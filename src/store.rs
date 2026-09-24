//! SQLite persistence. Event-sourced: raw inputs are immutable, every derived
//! row carries algorithm version / parameter fingerprint / direct sources, and
//! a monotonically increasing logical `seq` (virtual time) orders the journal.
use rusqlite::{params, Connection, Row};
use serde_json::{json, Map, Value};

use crate::ids;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS batch(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,            -- rinex_obs | obs_json | clock_events | sat_states | archive
  station TEXT,
  parent_id TEXT,                -- revision parent; raw inputs are never overwritten
  note TEXT,
  replaces_scope_json TEXT,      -- for revisions: [{station,sys,sat,from_ms,to_ms}]
  raw TEXT NOT NULL,             -- verbatim submission
  sha TEXT NOT NULL,
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS obs_epoch(
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sys TEXT NOT NULL,
  sat TEXT NOT NULL,
  epoch_ms INTEGER NOT NULL,
  seq INTEGER NOT NULL,          -- stable order within submission
  superseded INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS obs_value(
  id TEXT PRIMARY KEY,
  epoch_id TEXT NOT NULL,
  code TEXT NOT NULL,            -- observation code, e.g. C1C/L1C/D1C/S1C
  freq_num INTEGER,              -- frequency number, as imported
  value_raw TEXT NOT NULL,       -- raw representation, preserved verbatim
  unit_raw TEXT NOT NULL,        -- unit as imported/inferred
  lli INTEGER,
  ssi INTEGER,
  norm_value REAL,               -- derived layer: canonical-unit value
  norm_unit TEXT,                -- canonical unit
  norm_status TEXT NOT NULL,     -- ok | unconvertible | missing
  diagnostic TEXT
);
CREATE TABLE IF NOT EXISTS clock_event(
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  station TEXT NOT NULL,
  epoch_ms INTEGER NOT NULL,
  offset_s REAL NOT NULL,
  source TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sat_state(
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  sys TEXT NOT NULL,
  sat TEXT NOT NULL,
  epoch_ms INTEGER NOT NULL,
  x_m REAL, y_m REAL, z_m REAL, clock_s REAL,
  datum TEXT NOT NULL            -- coordinate datum, carried with the quantity
);
CREATE TABLE IF NOT EXISTS arc(
  id TEXT PRIMARY KEY,           -- hash(station,sys,sat,start_ms)
  station TEXT NOT NULL,
  sys TEXT NOT NULL,
  sat TEXT NOT NULL,
  start_ms INTEGER NOT NULL,
  end_ms INTEGER NOT NULL,
  arc_index INTEGER NOT NULL,
  source_fp TEXT NOT NULL,       -- fingerprint of the epochs this derives from
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS detector_run(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  algo_version TEXT NOT NULL,
  param_fp TEXT NOT NULL,
  params_json TEXT NOT NULL,
  scope_json TEXT NOT NULL,
  source_fp TEXT NOT NULL,       -- direct sources of this run
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS run_note(
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  arc_id TEXT,
  detector TEXT NOT NULL,
  status TEXT NOT NULL,          -- unavailable | error | ok
  reason TEXT NOT NULL,          -- e.g. missing_frequency:L2, doppler_missing
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS candidate(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  run_id TEXT NOT NULL,
  branch TEXT NOT NULL,
  arc_id TEXT NOT NULL,
  epoch_ms INTEGER NOT NULL,
  kind TEXT NOT NULL,
  detector TEXT NOT NULL,
  score REAL NOT NULL,
  slip_json TEXT NOT NULL,       -- SlipOption or [SlipOption] for ambiguous groups
  basis_json TEXT NOT NULL,      -- machine-readable judgement basis
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS decision(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  target_kind TEXT NOT NULL,     -- candidate | decision
  target_id TEXT NOT NULL,
  action TEXT NOT NULL,          -- accept | reject | undo | fork | adjust_params
  payload_json TEXT NOT NULL,    -- rationale code, chosen option, ...
  parent_id TEXT,
  author TEXT NOT NULL,
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS repair_offset(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  scope TEXT NOT NULL,           -- arc | receiver  (clock vs slip stay separate)
  station TEXT NOT NULL,
  sys TEXT NOT NULL,
  sat TEXT NOT NULL,
  from_ms INTEGER NOT NULL,
  freq_num INTEGER NOT NULL,     -- 0 => receiver clock layer
  cycles REAL NOT NULL,
  decision_id TEXT NOT NULL,     -- provenance: which decision created it
  active INTEGER NOT NULL DEFAULT 1,
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS invalidation(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  cause_batch_id TEXT NOT NULL,
  target_kind TEXT NOT NULL,     -- arc | candidate | run
  target_id TEXT NOT NULL,
  created_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS event_log(
  seq INTEGER PRIMARY KEY,
  id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
"#;

pub const TABLES: &[&str] = &[
    "batch", "obs_epoch", "obs_value", "clock_event", "sat_state", "arc",
    "detector_run", "run_note", "candidate", "decision", "repair_offset",
    "invalidation", "event_log",
];

pub struct Store {
    pub conn: Connection,
}

fn row_to_json(row: &Row, ncols: usize, names: &[String]) -> rusqlite::Result<Value> {
    let mut map = Map::new();
    for i in 0..ncols {
        let v: rusqlite::types::Value = row.get(i)?;
        let jv = match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(x) => json!(x),
            rusqlite::types::Value::Real(x) => json!(x),
            rusqlite::types::Value::Text(s) => json!(s),
            rusqlite::types::Value::Blob(b) => json!(hexdump(&b)),
        };
        map.insert(names[i].clone(), jv);
    }
    Ok(Value::Object(map))
}

fn hexdump(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

impl Store {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// Virtual time: a logical sequence counter, never wall-clock.
    pub fn next_seq(&self) -> rusqlite::Result<i64> {
        let cur: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0))
            .ok();
        let next = cur.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0) + 1;
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES('seq',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![next.to_string()],
        )?;
        Ok(next)
    }

    pub fn head_seq(&self) -> rusqlite::Result<i64> {
        let cur: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0))
            .ok();
        Ok(cur.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0))
    }

    pub fn set_head_seq(&self, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES('seq',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![seq.to_string()],
        )?;
        Ok(())
    }

    pub fn log_event(&self, kind: &str, payload: &Value) -> rusqlite::Result<i64> {
        let seq = self.next_seq()?;
        let id = ids::content_id("evt", &[&seq.to_string(), kind, &payload.to_string()]);
        self.conn.execute(
            "INSERT INTO event_log(seq,id,kind,payload_json) VALUES(?1,?2,?3,?4)",
            params![seq, id, kind, payload.to_string()],
        )?;
        Ok(seq)
    }

    pub fn export_all(&self) -> rusqlite::Result<Value> {
        let mut root = Map::new();
        root.insert("format".into(), json!("cycle-slip-review/archive/1"));
        root.insert("head_seq".into(), json!(self.head_seq()?));
        for table in TABLES {
            let order = if *table == "event_log" { "seq" } else { "rowid" };
            let mut stmt = self
                .conn
                .prepare(&format!("SELECT * FROM {} ORDER BY {}", table, order))?;
            let names: Vec<String> =
                stmt.column_names().iter().map(|s| s.to_string()).collect();
            let ncols = names.len();
            let rows = stmt.query_map([], |r| row_to_json(r, ncols, &names))?;
            let mut arr = Vec::new();
            for r in rows {
                arr.push(r?);
            }
            root.insert(table.to_string(), Value::Array(arr));
        }
        Ok(Value::Object(root))
    }

    /// Re-import an archive. Ids and seq are preserved verbatim; existing rows
    /// are skipped (idempotent), so identity, order and provenance survive an
    /// export / delete / import round-trip.
    pub fn import_all(&self, archive: &Value) -> rusqlite::Result<usize> {
        let mut inserted = 0usize;
        for table in TABLES {
            let Some(rows) = archive.get(*table).and_then(|v| v.as_array()) else {
                continue;
            };
            for row in rows {
                let Some(obj) = row.as_object() else { continue };
                let cols: Vec<&String> = obj.keys().collect();
                let col_list = cols
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let placeholders = (1..=cols.len())
                    .map(|i| format!("?{}", i))
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "INSERT OR IGNORE INTO {}({}) VALUES({})",
                    table, col_list, placeholders
                );
                let values: Vec<rusqlite::types::Value> = cols
                    .iter()
                    .map(|c| json_to_sql(&obj[*c]))
                    .collect();
                let mut stmt = self.conn.prepare(&sql)?;
                inserted += stmt.insert(rusqlite::params_from_iter(values)).unwrap_or(0);
            }
        }
        if let Some(h) = archive.get("head_seq").and_then(|v| v.as_i64()) {
            if h > self.head_seq()? {
                self.set_head_seq(h)?;
            }
        }
        Ok(inserted)
    }
}

fn json_to_sql(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else {
                rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}
