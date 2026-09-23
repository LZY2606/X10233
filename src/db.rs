// SQLite persistence: append-only event ledger + versioned raw layer +
// scoped derived layer. Derived rows are never updated in place.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Db {
    pub conn: Connection,
}

pub const SCHEMA_VERSION: &str = "1";

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn memory() -> rusqlite::Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        let v: String = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
            .unwrap_or_else(|_| "0".to_string());
        if v != SCHEMA_VERSION {
            self.conn.execute(
                "INSERT INTO meta(key,value) VALUES('schema_version',?1)
                 ON CONFLICT(key) DO UPDATE SET value=?1",
                params![SCHEMA_VERSION],
            )?;
        }
        Ok(())
    }

    pub fn tx<T, F: FnOnce(&Connection) -> rusqlite::Result<T>>(&mut self, f: F) -> rusqlite::Result<T> {
        let tx = self.conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

pub fn project_exists(conn: &Connection, project_id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT 1 FROM project WHERE id=?1", params![project_id], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
}

pub fn next_seq(conn: &Connection, project_id: &str, table: &str) -> rusqlite::Result<i64> {
    // All seq counters share the event-style monotonic per-project ordering.
    let sql = format!(
        "SELECT COALESCE(MAX(seq),0)+1 FROM {} WHERE project_id=?1",
        table
    );
    conn.query_row(&sql, params![project_id], |r| r.get(0))
}

pub fn json_get<T: serde::de::DeserializeOwned>(s: Option<String>) -> T {
    serde_json::from_str(&s.expect("json column")).expect("stored json")
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS project(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  note TEXT NOT NULL DEFAULT ''
);

-- Append-only ledger. parents is a JSON array; one parent normally, two on merge.
CREATE TABLE IF NOT EXISTS event(
  id TEXT NOT NULL,
  project_id TEXT NOT NULL REFERENCES project(id),
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,
  parents TEXT NOT NULL,          -- JSON array of event ids
  payload TEXT NOT NULL,         -- JSON, machine readable
  created_ms INTEGER NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(project_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS event_seq ON event(project_id, seq);

CREATE TABLE IF NOT EXISTS branch(
  project_id TEXT NOT NULL REFERENCES project(id),
  name TEXT NOT NULL,
  head_event_id TEXT NOT NULL,
  is_default INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(project_id, name)
);

-- Raw layer versions. Each import is immutable; later versions overlay earlier.
CREATE TABLE IF NOT EXISTS import(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES project(id),
  parent_import_id TEXT,
  seq INTEGER NOT NULL,
  label TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  created_by_event TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS import_seq ON import(project_id, seq);

CREATE TABLE IF NOT EXISTS raw_epoch(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  import_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  iso TEXT NOT NULL,
  epoch_s REAL NOT NULL,
  system TEXT NOT NULL,
  original_line TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS raw_epoch_lookup ON raw_epoch(project_id, seq);

CREATE TABLE IF NOT EXISTS raw_obs(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  import_id TEXT NOT NULL,
  epoch_seq INTEGER NOT NULL,
  sat TEXT NOT NULL,
  code TEXT NOT NULL,
  freq_idx INTEGER,
  original TEXT NOT NULL,
  value REAL,
  unit TEXT,
  lli TEXT,
  diagnostic TEXT
);
CREATE INDEX IF NOT EXISTS raw_obs_lookup ON raw_obs(project_id, epoch_seq, sat);

CREATE TABLE IF NOT EXISTS raw_clock(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  import_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  iso TEXT NOT NULL,
  epoch_s REAL NOT NULL,
  scope TEXT NOT NULL,
  delta_text TEXT NOT NULL,
  delta_m REAL,
  unit TEXT NOT NULL,
  kind TEXT NOT NULL,
  note TEXT NOT NULL,
  diagnostic TEXT
);

CREATE TABLE IF NOT EXISTS raw_satstate(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  import_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  sat TEXT NOT NULL,
  iso TEXT NOT NULL,
  epoch_s REAL NOT NULL,
  health TEXT NOT NULL,
  elevation_deg REAL,
  azimuth_deg REAL,
  datum TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS params(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES project(id),
  seq INTEGER NOT NULL,
  parent_id TEXT,
  created_by_event TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  json TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS params_seq ON params(project_id, seq);

-- A run is one deterministic recomputation. scope=NULL means full project.
CREATE TABLE IF NOT EXISTS run(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES project(id),
  seq INTEGER NOT NULL,
  parent_run_id TEXT,
  created_by_event TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  param_id TEXT NOT NULL,
  scope TEXT,                      -- JSON array of dep scopes or NULL for full
  source_imports TEXT NOT NULL,   -- JSON array of import ids used
  label TEXT NOT NULL DEFAULT '',
  created_ms INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS run_seq ON run(project_id, seq);

CREATE TABLE IF NOT EXISTS norm_obs(
  project_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  epoch_seq INTEGER NOT NULL,
  code TEXT NOT NULL,
  freq_idx INTEGER,
  value REAL,
  unit TEXT,
  lli TEXT,
  status TEXT NOT NULL,          -- ok | missing | unconvertible
  diagnostic TEXT,
  source_record_id TEXT NOT NULL,
  source_import_id TEXT NOT NULL,
  source_version_seq INTEGER NOT NULL,
  PRIMARY KEY(project_id, run_id, sat, epoch_seq, code)
);

CREATE TABLE IF NOT EXISTS combo(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  seq INTEGER NOT NULL,
  epoch_s REAL NOT NULL,
  gf_m REAL,
  mw_cycle REAL,
  gf_status TEXT NOT NULL,
  mw_status TEXT NOT NULL,
  diagnostic TEXT,
  dep_scope TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS arc(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  sat TEXT NOT NULL,
  start_seq INTEGER NOT NULL,
  end_seq INTEGER NOT NULL,
  reason_open TEXT NOT NULL,
  reason_close TEXT,
  missing_seqs TEXT NOT NULL,
  note TEXT,
  dep_scope TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS candidate(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  boundary_seq INTEGER NOT NULL,
  sat TEXT NOT NULL,
  kind TEXT NOT NULL,
  from_epoch_s REAL NOT NULL,
  to_epoch_s REAL NOT NULL,
  dt_s REAL NOT NULL,
  triggered TEXT NOT NULL,
  status TEXT NOT NULL,
  best_n1 INTEGER,
  best_n2 INTEGER,
  tie_group TEXT,
  body TEXT NOT NULL,
  dep_scope TEXT NOT NULL,
  note TEXT
);
CREATE INDEX IF NOT EXISTS cand_lookup ON candidate(project_id, run_id, boundary_seq, sat);

CREATE TABLE IF NOT EXISTS clock_candidate(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  boundary_seq INTEGER NOT NULL,
  from_epoch_s REAL NOT NULL,
  to_epoch_s REAL NOT NULL,
  dt_s REAL NOT NULL,
  step_m REAL NOT NULL,
  agreeing_sats INTEGER NOT NULL,
  total_sats INTEGER NOT NULL,
  declared_clock_id TEXT,
  kind TEXT NOT NULL,
  dep_scope TEXT NOT NULL
);
"#;
