//! SQLite persistence.
//!
//! Layering is enforced by the schema:
//! * `raw_*` tables are append-only. A revision is a new batch linked through
//!   `parent_id`; old rows are never overwritten.
//! * `*_derived` rows carry `rule_id`, direct source ids and a
//!   current/stale status. Local source changes expire only dependent rows.
//! * `events` is the append-only decision log used for replay and branching.

use rusqlite::Connection;

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    seed TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS batches (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    seq INTEGER NOT NULL,
    parent_id TEXT,
    kind TEXT NOT NULL,            -- OBS_FULL | OBS_PATCH | CLOCK | SAT_STATE
    original_text TEXT NOT NULL,   -- submitted bytes, immutable
    content_hash TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    UNIQUE (project_id, seq)
);

CREATE TABLE IF NOT EXISTS raw_epochs (
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    epoch TEXT NOT NULL,
    recv_clock_offset_s TEXT NOT NULL,
    PRIMARY KEY (project_id, batch_id, seq)
);

CREATE TABLE IF NOT EXISTS raw_obs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    epoch_seq INTEGER NOT NULL,
    epoch TEXT NOT NULL,
    sat TEXT NOT NULL,
    code TEXT NOT NULL,
    band TEXT NOT NULL,
    freq_hz INTEGER NOT NULL,
    raw_field TEXT NOT NULL,
    raw_value TEXT NOT NULL,
    lli INTEGER,
    ssi_raw TEXT,
    kind TEXT NOT NULL,
    diagnostic_code TEXT,
    diagnostic_message TEXT
);

CREATE TABLE IF NOT EXISTS raw_clock_events (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    epoch TEXT NOT NULL,
    event TEXT NOT NULL,
    jump_s REAL,
    source TEXT NOT NULL,
    detail_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS raw_sat_states (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    epoch TEXT NOT NULL,
    sat TEXT NOT NULL,
    pos_x_m REAL NOT NULL,
    pos_y_m REAL NOT NULL,
    pos_z_m REAL NOT NULL,
    datum TEXT NOT NULL,
    health TEXT NOT NULL,
    source TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS import_diagnostics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    severity TEXT NOT NULL,
    code TEXT NOT NULL,
    line INTEGER,
    message TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS rule_versions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    parent_id TEXT,
    fingerprint TEXT NOT NULL,
    params_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at_ms INTEGER NOT NULL,
    UNIQUE (project_id, seq)
);

CREATE TABLE IF NOT EXISTS norm_obs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    obs_id TEXT NOT NULL,
    epoch_seq INTEGER NOT NULL,
    epoch TEXT NOT NULL,
    sat TEXT NOT NULL,
    code TEXT NOT NULL,
    band TEXT NOT NULL,
    quantity_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'current',  -- current | stale
    UNIQUE(project_id, id)
);

CREATE TABLE IF NOT EXISTS clock_findings (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    epoch_seq INTEGER NOT NULL,
    epoch TEXT NOT NULL,
    jump_s REAL NOT NULL,
    mode TEXT NOT NULL,              -- COMMON_MODE
    affected_sats_json TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    source_event_ids_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'current'
);

CREATE TABLE IF NOT EXISTS arcs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    sat TEXT NOT NULL,
    code TEXT NOT NULL,
    start_epoch_seq INTEGER NOT NULL,
    end_epoch_seq INTEGER NOT NULL,
    start_epoch TEXT NOT NULL,
    end_epoch TEXT NOT NULL,
    end_reason TEXT NOT NULL,        -- OPEN | GAP | MISSING_FREQUENCY | SLIP | CLOCK_JUMP
    status TEXT NOT NULL DEFAULT 'current'
);

CREATE TABLE IF NOT EXISTS pair_results (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    sat TEXT NOT NULL,
    epoch_seq_a INTEGER NOT NULL,
    epoch_seq_b INTEGER NOT NULL,
    dt_s REAL NOT NULL,
    gap_flag TEXT NOT NULL,          -- REGULAR | LONG_GAP
    detector TEXT NOT NULL,          -- GF | MW | TD_L1 | TD_L2
    view_json TEXT NOT NULL,         -- {status: ok|unknown|..., quantities + units}
    source_obs_ids_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'current'
);

CREATE TABLE IF NOT EXISTS candidates (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    sat TEXT NOT NULL,
    epoch_seq INTEGER NOT NULL,
    epoch TEXT NOT NULL,
    proposals_json TEXT NOT NULL,    -- all legal hypotheses, never collapsed
    detectors_json TEXT NOT NULL,    -- per-detector independent opinions
    lli_flags_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'current'
);

CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    action TEXT NOT NULL,            -- ACCEPT | REJECT | PARAM_REVISION is separate
    proposal_json TEXT,
    reason TEXT NOT NULL,
    rule_id_at_time TEXT NOT NULL,
    event_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    undone_by_event_id TEXT
);

CREATE TABLE IF NOT EXISTS repair_offsets (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    decision_id TEXT NOT NULL,
    sat TEXT NOT NULL,
    band TEXT NOT NULL,
    cycles REAL NOT NULL,
    unit TEXT NOT NULL DEFAULT 'cycle',
    status TEXT NOT NULL DEFAULT 'current'  -- current | superseded
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    UNIQUE (project_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_raw_obs_key ON raw_obs(project_id, epoch_seq, sat, code);
CREATE INDEX IF NOT EXISTS idx_derived_status ON candidates(project_id, status);
CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, seq);
"#;

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}
