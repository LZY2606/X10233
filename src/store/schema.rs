use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 1;

pub fn init(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    marker TEXT NOT NULL DEFAULT '',
    coord_datum TEXT NOT NULL DEFAULT '',
    coord_unit TEXT NOT NULL DEFAULT '',
    created_event_id TEXT NOT NULL DEFAULT '',
    head_event_id TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS batches (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    raw_sha TEXT NOT NULL,
    parent_batch_id TEXT,
    imported_at_ms INTEGER NOT NULL,
    raw_text TEXT NOT NULL,
    UNIQUE(project_id, seq)
);
CREATE TABLE IF NOT EXISTS bands (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    band TEXT NOT NULL,
    phase_code TEXT NOT NULL,
    pseudo_code TEXT NOT NULL,
    doppler_code TEXT NOT NULL,
    snr_code TEXT NOT NULL,
    frequency_raw TEXT NOT NULL,
    frequency_hz REAL,
    wavelength_raw TEXT NOT NULL,
    wavelength_m REAL,
    unit TEXT NOT NULL,
    datum TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_obs (
    natural_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    line_no INTEGER NOT NULL,
    epoch_raw TEXT NOT NULL,
    epoch_secs REAL,
    epoch_datum TEXT NOT NULL,
    sat TEXT NOT NULL,
    code TEXT NOT NULL,
    kind TEXT NOT NULL,
    band TEXT NOT NULL,
    raw_value TEXT NOT NULL,
    raw_unit TEXT NOT NULL,
    value REAL,
    diagnostic TEXT,
    PRIMARY KEY (batch_id, natural_id)
);
CREATE TABLE IF NOT EXISTS raw_clock (
    natural_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    line_no INTEGER NOT NULL,
    epoch_raw TEXT NOT NULL,
    epoch_secs REAL,
    epoch_datum TEXT NOT NULL,
    raw_delta TEXT NOT NULL,
    raw_unit TEXT NOT NULL,
    delta_ns REAL,
    diagnostic TEXT,
    PRIMARY KEY (batch_id, natural_id)
);
CREATE TABLE IF NOT EXISTS raw_sat_state (
    natural_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    line_no INTEGER NOT NULL,
    epoch_raw TEXT NOT NULL,
    epoch_secs REAL,
    epoch_datum TEXT NOT NULL,
    sat TEXT NOT NULL,
    az_raw TEXT NOT NULL,
    el_raw TEXT NOT NULL,
    health_raw TEXT NOT NULL,
    datum TEXT NOT NULL,
    az_deg REAL,
    el_deg REAL,
    diagnostic TEXT,
    PRIMARY KEY (batch_id, natural_id)
);
CREATE TABLE IF NOT EXISTS import_diagnostics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL REFERENCES batches(id),
    line_no INTEGER NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    severity TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    parent_id TEXT,
    branch TEXT NOT NULL DEFAULT 'main',
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    body_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    batch_id TEXT,
    param_fp TEXT
);
CREATE INDEX IF NOT EXISTS events_project ON events(project_id, seq);
CREATE TABLE IF NOT EXISTS derived_statistics (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    viewpoint_event_id TEXT NOT NULL,
    detector TEXT NOT NULL,
    alg_version TEXT NOT NULL,
    param_fp TEXT NOT NULL,
    sat TEXT NOT NULL,
    band TEXT NOT NULL,
    at_seq INTEGER NOT NULL,
    at_epoch TEXT NOT NULL,
    dt_secs REAL NOT NULL,
    value REAL,
    value_unit TEXT NOT NULL,
    flag TEXT NOT NULL,
    sources_json TEXT NOT NULL,
    diagnostic TEXT,
    PRIMARY KEY (viewpoint_event_id, id)
);
CREATE TABLE IF NOT EXISTS derived_candidates (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    viewpoint_event_id TEXT NOT NULL,
    ambiguity_group TEXT NOT NULL,
    ambiguity_n INTEGER NOT NULL,
    detector TEXT NOT NULL,
    alg_version TEXT NOT NULL,
    param_fp TEXT NOT NULL,
    sat TEXT NOT NULL,
    band TEXT NOT NULL,
    band_b TEXT,
    at_seq INTEGER NOT NULL,
    at_epoch TEXT NOT NULL,
    offset_cycles REAL NOT NULL,
    offset_b_cycles REAL,
    offset_unknown INTEGER NOT NULL,
    score REAL NOT NULL,
    evidence_json TEXT NOT NULL,
    proposals_json TEXT NOT NULL,
    suggest_reject INTEGER NOT NULL,
    status TEXT NOT NULL,
    status_reason TEXT,
    offset_ids_json TEXT NOT NULL,
    sources_json TEXT NOT NULL,
    PRIMARY KEY (viewpoint_event_id, id)
);
CREATE TABLE IF NOT EXISTS derived_arcs (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    viewpoint_event_id TEXT NOT NULL,
    sat TEXT NOT NULL,
    band TEXT NOT NULL,
    start_seq INTEGER NOT NULL,
    end_seq INTEGER,
    start_epoch TEXT NOT NULL,
    end_epoch TEXT,
    state TEXT NOT NULL,
    close_reason TEXT,
    boundary_candidates_json TEXT NOT NULL,
    PRIMARY KEY (viewpoint_event_id, id)
);
CREATE TABLE IF NOT EXISTS derived_offsets (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    viewpoint_event_id TEXT NOT NULL,
    sat TEXT NOT NULL,
    band TEXT NOT NULL,
    at_seq INTEGER NOT NULL,
    at_epoch TEXT NOT NULL,
    cycles REAL NOT NULL,
    unit TEXT NOT NULL,
    decision_event_id TEXT NOT NULL,
    rationale TEXT NOT NULL,
    PRIMARY KEY (viewpoint_event_id, id)
);
CREATE TABLE IF NOT EXISTS derived_rejected (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    viewpoint_event_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    reason TEXT NOT NULL,
    decision_event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS materialized_view (
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    stable_id TEXT NOT NULL,
    viewpoint_event_id TEXT NOT NULL,
    freshness TEXT NOT NULL,
    PRIMARY KEY (project_id, kind, stable_id)
);
        "#,
    )?;
    Ok(())
}
