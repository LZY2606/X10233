use rusqlite::Connection;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(
  k TEXT PRIMARY KEY,
  v TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS inputs(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL,
  parent_id TEXT,
  raw TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  t_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS obs(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  input_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  epoch_ms INTEGER NOT NULL,
  freq INTEGER NOT NULL,
  code TEXT NOT NULL,
  p_raw TEXT NOT NULL,
  l_raw TEXT NOT NULL,
  d_raw TEXT NOT NULL,
  lli_raw TEXT NOT NULL,
  s_raw TEXT NOT NULL,
  p REAL, l REAL, d REAL, lli INTEGER, s REAL,
  diag TEXT
);
CREATE TABLE IF NOT EXISTS clkjumps(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  input_id TEXT NOT NULL,
  station TEXT NOT NULL,
  epoch_ms INTEGER NOT NULL,
  offset_raw TEXT NOT NULL,
  offset_s REAL,
  diag TEXT
);
CREATE TABLE IF NOT EXISTS params(
  id TEXT PRIMARY KEY,
  fp TEXT NOT NULL,
  json TEXT NOT NULL,
  t_ms INTEGER NOT NULL,
  note TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS arcs(
  id TEXT PRIMARY KEY,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  freq INTEGER NOT NULL,
  code TEXT NOT NULL,
  start_ms INTEGER NOT NULL,
  end_ms INTEGER NOT NULL,
  input_id TEXT NOT NULL,
  params_fp TEXT NOT NULL,
  stale INTEGER NOT NULL DEFAULT 0,
  stale_reason TEXT
);
CREATE TABLE IF NOT EXISTS arc_obs(
  arc_id TEXT NOT NULL,
  obs_id INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS candidates(
  id TEXT PRIMARY KEY,
  arc_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  freq INTEGER NOT NULL,
  epoch_ms INTEGER NOT NULL,
  detector TEXT NOT NULL,
  class TEXT NOT NULL,
  dn1 REAL, dn2 REAL, residual REAL,
  score REAL NOT NULL,
  status TEXT NOT NULL,
  reason TEXT,
  group_id TEXT,
  params_fp TEXT NOT NULL,
  input_id TEXT NOT NULL,
  stale INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS offsets(
  id TEXT PRIMARY KEY,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  freq INTEGER NOT NULL,
  from_ms INTEGER NOT NULL,
  dn REAL NOT NULL,
  event_id TEXT NOT NULL,
  revoked INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS events(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  parent_id TEXT,
  t_ms INTEGER NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL
);
";

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Allocate the next stable identity for a table, e.g. `arc_0007`.
/// Counters live in `meta` so exported archives re-import with identities intact.
pub fn next_id(conn: &Connection, key: &str, prefix: &str) -> rusqlite::Result<(String, i64)> {
    conn.execute(
        "INSERT INTO meta(k,v) VALUES(?1,'0') ON CONFLICT(k) DO NOTHING",
        [key],
    )?;
    conn.execute(
        "UPDATE meta SET v=CAST(CAST(v AS INTEGER)+1 AS TEXT) WHERE k=?1",
        [key],
    )?;
    let n: i64 = conn.query_row("SELECT CAST(v AS INTEGER) FROM meta WHERE k=?1", [key], |r| {
        r.get(0)
    })?;
    Ok((format!("{prefix}_{n:04}"), n))
}
