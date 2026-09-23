//! SQLite 持久化。原始层 append-only；派生层带生命周期状态与依赖指纹。

use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Db {
    pub conn: Mutex<Connection>,
}

pub const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS branches (
  name TEXT PRIMARY KEY,
  forked_from_event TEXT,
  created_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
  event_id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  parent_event_id TEXT,
  kind TEXT NOT NULL,
  virtual_ts TEXT NOT NULL,
  summary TEXT NOT NULL,
  payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_branch_seq ON events(branch, seq);

CREATE TABLE IF NOT EXISTS raw_obs (
  raw_id TEXT PRIMARY KEY,
  introduced_event TEXT NOT NULL,
  superseded_event TEXT,
  station TEXT NOT NULL, sat TEXT NOT NULL, epoch REAL NOT NULL,
  band TEXT NOT NULL, code TEXT NOT NULL,
  json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_raw_key ON raw_obs(station, sat, epoch, band);

CREATE TABLE IF NOT EXISTS raw_other (
  raw_id TEXT PRIMARY KEY,
  introduced_event TEXT NOT NULL,
  superseded_event TEXT,
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS diagnostics (
  diag_id TEXT PRIMARY KEY,
  introduced_event TEXT NOT NULL,
  kind TEXT NOT NULL,
  line_no INTEGER,
  message TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS derived (
  derived_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  branch TEXT NOT NULL,
  created_event TEXT NOT NULL,
  superseded_event TEXT,
  state TEXT NOT NULL,
  station TEXT, sat TEXT, epoch REAL, band TEXT,
  score REAL,
  sources TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  param_fp TEXT NOT NULL,
  json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_derived_lookup ON derived(kind, state, station, sat, epoch);
"#;

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.execute(
        "INSERT OR IGNORE INTO branches(name, forked_from_event, created_seq) VALUES ('main', NULL, 0)",
        [],
    )?;
    Ok(())
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        init(&conn)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    /// 内存数据库（测试用）。
    pub fn open_memory() -> rusqlite::Result<Db> {
        let conn = Connection::open_in_memory()?;
        init(&conn)?;
        Ok(Db { conn: Mutex::new(conn) })
    }
}
