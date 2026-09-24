use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    init(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    init(&mut conn)?;
    Ok(conn)
}

fn init(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_seq INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS raws (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    parent_raw_id TEXT,
    supersedes_raw_id TEXT,
    filename TEXT NOT NULL,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
);

CREATE TABLE IF NOT EXISTS raw_observations (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    raw_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    epoch_idx INTEGER NOT NULL,
    tow_s REAL NOT NULL,
    epoch_text TEXT NOT NULL,
    epoch_flag TEXT NOT NULL,
    sat TEXT NOT NULL,
    code TEXT NOT NULL,
    band INTEGER,
    obs_class TEXT NOT NULL,
    value_raw TEXT NOT NULL,
    value REAL,
    lli INTEGER,
    ssi INTEGER,
    unit TEXT NOT NULL,
    datum TEXT NOT NULL,
    raw_line TEXT NOT NULL,
    diagnostics TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (project_id, id)
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    parents_json TEXT NOT NULL DEFAULT '[]',
    note TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (project_id, id)
);
CREATE INDEX IF NOT EXISTS events_branch ON events(project_id, branch, seq);
CREATE INDEX IF NOT EXISTS events_parent ON events(parents_json);

CREATE TABLE IF NOT EXISTS branches (
    project_id TEXT NOT NULL,
    name TEXT NOT NULL,
    head_event_id TEXT,
    head_seq INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, name)
);

CREATE TABLE IF NOT EXISTS params_versions (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    params_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_event INTEGER NOT NULL,
    PRIMARY KEY (project_id, id)
);

CREATE TABLE IF NOT EXISTS runs (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    algo_version TEXT NOT NULL,
    params_fingerprint TEXT NOT NULL,
    params_id TEXT NOT NULL,
    raw_ids_json TEXT NOT NULL,
    created_event INTEGER NOT NULL,
    stale INTEGER NOT NULL DEFAULT 0,
    stale_reason TEXT,
    PRIMARY KEY (project_id, id)
);

CREATE TABLE IF NOT EXISTS boundary_evals (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    bkey TEXT NOT NULL,
    station TEXT NOT NULL,
    sat TEXT NOT NULL,
    band INTEGER NOT NULL,
    code TEXT NOT NULL,
    arc INTEGER NOT NULL,
    boundary_epoch INTEGER NOT NULL,
    deps_fingerprint TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL,
    detectors_json TEXT NOT NULL,
    candidates_json TEXT NOT NULL,
    clock_jump INTEGER NOT NULL DEFAULT 0,
    lli_flag INTEGER NOT NULL DEFAULT 0,
    lli_verdict TEXT,
    note TEXT,
    PRIMARY KEY (project_id, id)
);
CREATE INDEX IF NOT EXISTS be_run ON boundary_evals(project_id, run_id);
CREATE INDEX IF NOT EXISTS be_bkey ON boundary_evals(project_id, branch, bkey);

CREATE TABLE IF NOT EXISTS decisions (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    boundary_key TEXT NOT NULL,
    action TEXT NOT NULL,
    rationale TEXT NOT NULL DEFAULT '',
    actor TEXT NOT NULL DEFAULT 'reviewer',
    created_event INTEGER NOT NULL,
    superseded_by TEXT,
    PRIMARY KEY (project_id, id)
);
CREATE INDEX IF NOT EXISTS decisions_bkey ON decisions(project_id, branch, boundary_key);

CREATE TABLE IF NOT EXISTS offsets (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    boundary_key TEXT NOT NULL,
    slip_cycles REAL NOT NULL,
    unit TEXT NOT NULL,
    source_decision TEXT NOT NULL,
    created_event INTEGER NOT NULL,
    PRIMARY KEY (project_id, id)
);
CREATE INDEX IF NOT EXISTS offsets_bkey ON offsets(project_id, branch, boundary_key);

CREATE TABLE IF NOT EXISTS clock_events (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    raw_id TEXT NOT NULL,
    tow_s REAL NOT NULL,
    kind TEXT NOT NULL,
    offset_value REAL NOT NULL,
    offset_unit TEXT NOT NULL,
    source TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
);

CREATE TABLE IF NOT EXISTS sat_states (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    raw_id TEXT NOT NULL,
    tow_s REAL NOT NULL,
    sat TEXT NOT NULL,
    x_m REAL, y_m REAL, z_m REAL,
    datum TEXT,
    elev_deg REAL,
    azim_deg REAL,
    payload_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ss_lookup ON sat_states(project_id, tow_s, sat);

CREATE TABLE IF NOT EXISTS arcs (
    project_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    station TEXT NOT NULL,
    sat TEXT NOT NULL,
    band INTEGER NOT NULL,
    code TEXT NOT NULL,
    arc INTEGER NOT NULL,
    start_epoch INTEGER NOT NULL,
    end_epoch INTEGER NOT NULL,
    start_tow_s REAL NOT NULL,
    end_tow_s REAL NOT NULL,
    n_epochs INTEGER NOT NULL,
    PRIMARY KEY (project_id, run_id, sat, band, code, arc)
);

CREATE TABLE IF NOT EXISTS combo_points (
    project_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    station TEXT NOT NULL,
    sat TEXT NOT NULL,
    band INTEGER NOT NULL,
    code TEXT NOT NULL,
    arc INTEGER NOT NULL,
    epoch_idx INTEGER NOT NULL,
    tow_s REAL NOT NULL,
    l4_m REAL, mw_m REAL, td_phase_m REAL, td_code_m REAL,
    elev_deg REAL
);
CREATE INDEX IF NOT EXISTS cp_run ON combo_points(project_id, run_id, sat, band, code, arc, epoch_idx);

CREATE TABLE IF NOT EXISTS run_evals (
    project_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    eval_id TEXT NOT NULL,
    PRIMARY KEY (project_id, run_id, eval_id)
);
"#,
    )?;
    Ok(())
}

pub fn max_revision(conn: &Connection, project_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(revision),0) FROM raws WHERE project_id=?1",
        params![project_id],
        |r| r.get(0),
    )
}

pub fn next_seq(conn: &Connection, project_id: &str, branch: &str) -> rusqlite::Result<i64> {
    let m: Option<i64> = conn
        .query_row(
            "SELECT MAX(seq) FROM events WHERE project_id=?1 AND branch=?2",
            params![project_id, branch],
            |r| r.get(0),
        )
        .optional()?;
    Ok(m.unwrap_or(0) + 1)
}
