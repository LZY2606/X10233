pub const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Append-only event log.
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    version_id TEXT NOT NULL,
    parent_version_id TEXT NOT NULL,
    supersedes TEXT NOT NULL DEFAULT '',
    at_virtual TEXT NOT NULL,
    payload TEXT NOT NULL,
    sources TEXT NOT NULL,
    actor TEXT NOT NULL,
    note TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS versions (
    version_id TEXT PRIMARY KEY,
    parent_version_id TEXT NOT NULL,
    kind TEXT NOT NULL,           -- source | params | derived | head
    created_seq INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    payload TEXT NOT NULL
);

-- Immutable raw import batches.
CREATE TABLE IF NOT EXISTS source_batches (
    batch_id TEXT PRIMARY KEY,
    version_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    case_id TEXT NOT NULL,
    nominal_interval_s REAL NOT NULL,
    payload_json TEXT NOT NULL,
    UNIQUE(case_id, seq)
);

-- Raw station metadata per source version.
CREATE TABLE IF NOT EXISTS station_meta (
    batch_id TEXT NOT NULL,
    marker_name TEXT NOT NULL,
    marker_number TEXT NOT NULL,
    receiver TEXT NOT NULL,
    antenna TEXT NOT NULL,
    x_m REAL NOT NULL, y_m REAL NOT NULL, z_m REAL NOT NULL,
    PRIMARY KEY(batch_id)
);

-- Raw epochs, kept exactly as imported.
CREATE TABLE IF NOT EXISTS epochs_raw (
    batch_id TEXT NOT NULL,
    epoch_index INTEGER NOT NULL,
    label TEXT NOT NULL,
    timestamp_raw TEXT NOT NULL,
    tow_s REAL NOT NULL,
    irregular INTEGER NOT NULL,
    PRIMARY KEY(batch_id, epoch_index)
);

CREATE TABLE IF NOT EXISTS sats_raw (
    batch_id TEXT NOT NULL,
    epoch_index INTEGER NOT NULL,
    sat_key TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY(batch_id, epoch_index, sat_key)
);

-- One row per observation token. raw_text and lli/ss are immutable.
CREATE TABLE IF NOT EXISTS obs_raw (
    batch_id TEXT NOT NULL,
    epoch_index INTEGER NOT NULL,
    sat_key TEXT NOT NULL,
    signal_key TEXT NOT NULL,
    constellation TEXT NOT NULL,
    band_char TEXT NOT NULL,
    channel INTEGER NOT NULL,
    obs_code TEXT NOT NULL,
    obs_kind TEXT NOT NULL,
    raw_text TEXT NOT NULL,
    numeric_value REAL,
    lli_text TEXT NOT NULL,
    ss_text TEXT NOT NULL,
    lli_flags INTEGER NOT NULL,
    diagnostics TEXT NOT NULL,
    PRIMARY KEY(batch_id, epoch_index, sat_key, signal_key, obs_kind)
);

CREATE TABLE IF NOT EXISTS clock_events_raw (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL,
    at_epoch_label TEXT NOT NULL,
    jump_ms REAL NOT NULL,
    source TEXT NOT NULL,
    note TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS satellite_states_raw (
    batch_id TEXT NOT NULL,
    sat_key TEXT NOT NULL,
    x_m REAL NOT NULL, y_m REAL NOT NULL, z_m REAL NOT NULL,
    clock_bias_s REAL NOT NULL,
    datum TEXT NOT NULL,
    PRIMARY KEY(batch_id, sat_key)
);

-- Derived arcs per derived version.
CREATE TABLE IF NOT EXISTS arcs (
    derived_version TEXT NOT NULL,
    arc_id TEXT NOT NULL,
    sat_key TEXT NOT NULL,
    band TEXT NOT NULL,
    start_label TEXT NOT NULL,
    end_label TEXT NOT NULL,
    reason TEXT NOT NULL,
    PRIMARY KEY(derived_version, arc_id)
);

-- Candidate projections (both accepted, rejected and pending).
CREATE TABLE IF NOT EXISTS candidates (
    derived_version TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    sat_key TEXT NOT NULL,
    epoch_label TEXT NOT NULL,
    offsets_json TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    score REAL NOT NULL,
    tied INTEGER NOT NULL,
    disposition TEXT NOT NULL,
    ambiguity TEXT,
    reject_reason TEXT,
    algo_version TEXT NOT NULL,
    param_fingerprint TEXT NOT NULL,
    sources_json TEXT NOT NULL,
    PRIMARY KEY(derived_version, candidate_id)
);

-- Offset layer: only integer cycle corrections, never rewritten observations.
CREATE TABLE IF NOT EXISTS repair_offsets (
    id TEXT PRIMARY KEY,
    derived_version TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    sat_key TEXT NOT NULL,
    band TEXT NOT NULL,
    cycles INTEGER NOT NULL,
    accepted_event_id TEXT NOT NULL
);

-- Combination residual points for timeline/residual views.
CREATE TABLE IF NOT EXISTS residual_points (
    derived_version TEXT NOT NULL,
    sat_key TEXT NOT NULL,
    detector TEXT NOT NULL,
    epoch_label TEXT NOT NULL,
    value REAL,
    unit TEXT NOT NULL,
    threshold REAL NOT NULL,
    fired INTEGER NOT NULL,
    status TEXT NOT NULL,   -- current | stale | unknown
    PRIMARY KEY(derived_version, sat_key, detector, epoch_label)
);

-- Per derived object staleness, with the direct source that invalidated it.
CREATE TABLE IF NOT EXISTS stale_mark (
    derived_version TEXT NOT NULL,
    scope TEXT NOT NULL,          -- arc | candidate | residual
    object_id TEXT NOT NULL,
    because_event_id TEXT NOT NULL,
    because_source TEXT NOT NULL,
    marked_seq INTEGER NOT NULL,
    PRIMARY KEY(derived_version, scope, object_id)
);

-- Human decisions beyond accept/reject payloads (conflict resolution log).
CREATE TABLE IF NOT EXISTS decisions (
    event_id TEXT PRIMARY KEY,
    derived_version TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    conflicts_with TEXT NOT NULL,
    supersedes_decision TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_version ON events(version_id);
CREATE INDEX IF NOT EXISTS idx_cand_disp ON candidates(derived_version, disposition);
"#;
