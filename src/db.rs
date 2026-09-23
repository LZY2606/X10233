//! SQLite storage. Identity is content-derived and stable across export /
//! delete / re-import; the event log is an append-only DAG.

use rusqlite::Connection;

pub const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  head_event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  parent_id TEXT,
  kind TEXT NOT NULL,
  occurred_ns INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  UNIQUE(project_id, seq, parent_id)
);
CREATE INDEX IF NOT EXISTS events_project ON events(project_id, seq);
CREATE TABLE IF NOT EXISTS input_versions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  filename TEXT NOT NULL,
  kind TEXT NOT NULL,             -- RINEX | STATE
  amendment_of TEXT,
  sha256 TEXT NOT NULL,
  imported_event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_lines (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  input_id TEXT NOT NULL,
  line_no INTEGER NOT NULL,
  raw_text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_obs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  input_id TEXT NOT NULL,
  gps_ns INTEGER NOT NULL,
  epoch_line_no INTEGER NOT NULL,
  sat TEXT NOT NULL,
  code TEXT NOT NULL,
  kind TEXT NOT NULL,
  raw_text TEXT NOT NULL,
  value REAL,
  lli INTEGER NOT NULL,
  signal_strength TEXT NOT NULL,
  unit TEXT NOT NULL,
  diagnostic TEXT
);
CREATE TABLE IF NOT EXISTS clock_events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  input_id TEXT NOT NULL,
  at_second REAL NOT NULL,
  magnitude_s REAL NOT NULL,
  scope TEXT NOT NULL,
  raw_text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS frequencies (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  input_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  band TEXT NOT NULL,
  freq_hz REAL NOT NULL,
  freq_number INTEGER NOT NULL,
  wavelength_m REAL NOT NULL,
  raw_text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS import_diagnostics (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  input_id TEXT NOT NULL,
  line_no INTEGER NOT NULL,
  severity TEXT NOT NULL,
  code TEXT NOT NULL,
  message TEXT NOT NULL,
  raw_text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS param_versions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  params_json TEXT NOT NULL,
  event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS arcs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  code TEXT NOT NULL,
  start_ns INTEGER NOT NULL,
  end_ns INTEGER,
  source_key TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  params_fp TEXT NOT NULL,
  input_fp TEXT NOT NULL,
  decision_ref TEXT,
  status TEXT NOT NULL,
  fresh TEXT NOT NULL DEFAULT 'CURRENT',
  created_event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS pair_points (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  gps_ns INTEGER NOT NULL,
  dt_s REAL NOT NULL,
  gf_m REAL,
  gf_jump_m REAL,
  mw_wl REAL,
  mw_jump_wl REAL,
  td1_res_cyc REAL,
  td2_res_cyc REAL,
  lli1 INTEGER NOT NULL,
  lli2 INTEGER NOT NULL,
  status TEXT NOT NULL,
  reason TEXT,
  source_key TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  params_fp TEXT NOT NULL,
  input_fp TEXT NOT NULL,
  fresh TEXT NOT NULL DEFAULT 'CURRENT',
  created_event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS candidates (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  gps_ns INTEGER NOT NULL,
  detector TEXT NOT NULL,
  n1 INTEGER NOT NULL,
  n2 INTEGER NOT NULL,
  residual REAL NOT NULL,
  residual_unit TEXT NOT NULL,
  score REAL NOT NULL,
  rejected_reason TEXT,
  pair_point_id TEXT,
  source_key TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  params_fp TEXT NOT NULL,
  input_fp TEXT NOT NULL,
  fresh TEXT NOT NULL DEFAULT 'CURRENT',
  created_event_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS decisions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  verdict TEXT NOT NULL,            -- ACCEPTED | REJECTED
  offset1 INTEGER,
  offset2 INTEGER,
  event_id TEXT NOT NULL,
  supersedes_decision_id TEXT,
  note TEXT
);
CREATE TABLE IF NOT EXISTS offset_layer (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  code TEXT NOT NULL,
  from_ns INTEGER NOT NULL,
  cycles INTEGER NOT NULL,
  decision_id TEXT NOT NULL
);
"#;

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}
