//! SQLite persistence. Raw submissions are immutable; every derived row
//! carries algorithm version, parameter fingerprint and direct sources.
use rusqlite::{params, Connection};
use std::path::Path;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS imports(
  id TEXT PRIMARY KEY,
  parent_id TEXT,
  label TEXT NOT NULL,
  raw_text TEXT NOT NULL,
  seq INTEGER NOT NULL,
  diag TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS observations(
  id TEXT PRIMARY KEY,
  import_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  epoch_label TEXT NOT NULL,
  epoch_secs REAL,
  freq_number INTEGER NOT NULL,
  obs_code TEXT NOT NULL,
  pr_raw TEXT NOT NULL, pr_m REAL,
  cp_raw TEXT NOT NULL, cp_cycles REAL,
  dp_raw TEXT NOT NULL, dp_hz REAL,
  lli_raw TEXT NOT NULL, lli INTEGER,
  snr_raw TEXT NOT NULL, snr REAL,
  clock_jump_raw TEXT, clock_jump_s REAL,
  diag TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS arcs(
  id TEXT PRIMARY KEY,
  station TEXT NOT NULL, sat TEXT NOT NULL, freq_number INTEGER NOT NULL,
  start_secs REAL NOT NULL, end_secs REAL NOT NULL,
  obs_ids TEXT NOT NULL, boundary TEXT NOT NULL,
  algo_version TEXT NOT NULL, param_fp TEXT NOT NULL,
  diag TEXT NOT NULL DEFAULT '[]', stale INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS candidates(
  id TEXT PRIMARY KEY,
  arc_id TEXT NOT NULL,
  detector TEXT NOT NULL,
  epoch_label TEXT NOT NULL, epoch_secs REAL NOT NULL,
  slip_cycles TEXT NOT NULL,
  score REAL NOT NULL,
  status TEXT NOT NULL,
  reason TEXT,
  algo_version TEXT NOT NULL, param_fp TEXT NOT NULL,
  source_ids TEXT NOT NULL, stale INTEGER NOT NULL DEFAULT 0,
  seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS clock_events(
  id TEXT PRIMARY KEY,
  station TEXT NOT NULL,
  epoch_label TEXT NOT NULL, epoch_secs REAL NOT NULL,
  kind TEXT NOT NULL, sat TEXT, magnitude_m REAL NOT NULL,
  detector TEXT NOT NULL,
  algo_version TEXT NOT NULL, param_fp TEXT NOT NULL,
  source_ids TEXT NOT NULL, stale INTEGER NOT NULL DEFAULT 0,
  seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS repairs(
  id TEXT PRIMARY KEY,
  arc_id TEXT NOT NULL, from_epoch_secs REAL NOT NULL,
  offsets TEXT NOT NULL, decision_id TEXT NOT NULL,
  algo_version TEXT NOT NULL, param_fp TEXT NOT NULL,
  source_ids TEXT NOT NULL, stale INTEGER NOT NULL DEFAULT 0,
  seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS decisions(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  parent_id TEXT,
  kind TEXT NOT NULL,
  target_id TEXT,
  payload TEXT NOT NULL,
  undone INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS events(
  seq INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS params(
  id TEXT PRIMARY KEY,
  version INTEGER NOT NULL,
  json TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  seq INTEGER NOT NULL
);
"#;

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let s = Store { conn };
        s.init()?;
        Ok(s)
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Store { conn };
        s.init()?;
        Ok(s)
    }

    fn init(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        self.conn.execute(
            "INSERT INTO meta(k,v) VALUES('seq','0') ON CONFLICT(k) DO NOTHING",
            [],
        )?;
        Ok(())
    }

    /// Logical (virtual) clock: strictly monotone, no wall time involved.
    pub fn next_seq(&self) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "UPDATE meta SET v = CAST(CAST(v AS INTEGER) + 1 AS TEXT) WHERE k='seq' RETURNING CAST(v AS INTEGER)",
            [],
            |r| r.get(0),
        )
    }

    pub fn current_seq(&self) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "SELECT CAST(v AS INTEGER) FROM meta WHERE k='seq'",
            [],
            |r| r.get(0),
        )
    }

    pub fn set_seq_at_least(&self, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE meta SET v = CAST(?1 AS TEXT) WHERE k='seq' AND CAST(v AS INTEGER) < ?1",
            params![seq],
        )?;
        Ok(())
    }

    /// Append a machine-readable event to the judgment log.
    pub fn emit(&self, kind: &str, payload: &serde_json::Value) -> rusqlite::Result<i64> {
        let seq = self.next_seq()?;
        self.conn.execute(
            "INSERT INTO events(seq, kind, payload) VALUES(?1, ?2, ?3)",
            params![seq, kind, payload.to_string()],
        )?;
        Ok(seq)
    }
}
