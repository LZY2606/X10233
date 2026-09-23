//! SQLite store.
//!
//! Layering:
//!   * raw layer: projects, batches, epochs, observations, clock events,
//!     frequencies — immutable once committed (new batches express revisions
//!     through parent relationships);
//!   * event log: append-only decisions/imports/parameter revisions, with a
//!     branch pointer each; branching re-points the head to an ancestor event;
//!   * derived layer: produced nodes (arcs, boundary candidate sets, repair
//!     overlays) carrying algorithm version, parameter fingerprint and direct
//!     sources. They are never deleted: stale nodes remain viewable.

use crate::engine::{self, DetParams};
use crate::model::*;
use crate::parser;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub struct Store {
    pub conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub station_id: Option<String>,
    pub station_datum: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionPayload {
    pub boundary_slot: String,
    pub action: String, // accept | reject
    pub candidate_id: Option<String>,
    pub offset_b1_cycles: Option<i64>,
    pub offset_b2_cycles: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamsPayload {
    pub params: DetParams,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPayload {
    pub batch_id: String,
    pub parent_batch_id: Option<String>,
    pub report: ImportReport,
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let mut s = Store { conn };
        s.init()?;
        Ok(s)
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut s = Store { conn };
        s.init()?;
        Ok(s)
    }

    fn init(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch(SCHEMA)
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  station_id TEXT,
  station_datum TEXT
);
CREATE TABLE IF NOT EXISTS batches (
  id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  parent_batch_id TEXT,
  ord INTEGER NOT NULL,
  raw_text TEXT NOT NULL,
  imported_by_event TEXT,
  PRIMARY KEY (project_id, id)
);
CREATE TABLE IF NOT EXISTS frequencies (
  id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  system TEXT NOT NULL,
  band TEXT NOT NULL,
  freq_hz REAL NOT NULL,
  code TEXT NOT NULL,
  PRIMARY KEY (project_id, id, batch_id)
);
CREATE TABLE IF NOT EXISTS epochs (
  id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  ord INTEGER NOT NULL,
  t0 TEXT NOT NULL,
  offset_s REAL NOT NULL,
  PRIMARY KEY (project_id, id, batch_id)
);
CREATE TABLE IF NOT EXISTS observations (
  id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  epoch_id TEXT NOT NULL,
  ord INTEGER NOT NULL,
  satellite TEXT NOT NULL,
  tokens_json TEXT NOT NULL,
  PRIMARY KEY (project_id, id, batch_id)
);
CREATE TABLE IF NOT EXISTS clock_events (
  id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  epoch_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  delta_s REAL NOT NULL,
  source TEXT NOT NULL,
  raw TEXT NOT NULL,
  PRIMARY KEY (project_id, id, batch_id)
);
CREATE TABLE IF NOT EXISTS events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  parent_event_id TEXT
);
CREATE TABLE IF NOT EXISTS branches (
  project_id TEXT NOT NULL,
  name TEXT NOT NULL,
  head_event_id TEXT NOT NULL,
  head_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, name)
);
CREATE TABLE IF NOT EXISTS nodes (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  slot TEXT NOT NULL,
  kind TEXT NOT NULL,
  producing_event TEXT NOT NULL,
  param_fp TEXT NOT NULL,
  direct_sources_json TEXT NOT NULL,
  body_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nodes_slot ON nodes(project_id, slot);
CREATE INDEX IF NOT EXISTS idx_nodes_event ON nodes(project_id, producing_event);
"#;
