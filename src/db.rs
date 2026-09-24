//! SQLite persistence (event sourcing + materialized derived rows).
//!
//! Raw inputs are immutable rows; revisions are new batches with a parent
//! relation. Derived rows carry algorithm version, parameter fingerprint and
//! direct sources. Export writes every row verbatim, and re-import restores
//! identity, order and source relations exactly.

use crate::clock::Clock;
use crate::identity::fid;
use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::sync::Mutex;

pub const ALGORITHM_VERSION: &str = "carrier-break-station/algo-1.0.0";

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub created_iso: String,
    pub clock_basis: String,
    pub algo_version: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventRec {
    pub id: String,
    pub branch: String,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub parent_event_id: Option<String>,
    pub at_iso: String,
    pub clock_basis: String,
}

pub struct Db {
    pub conn: Mutex<Connection>,
}

pub fn open(path: &str) -> rusqlite::Result<Db> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    let db = Db { conn: Mutex::new(conn) };
    db.migrate()?;
    Ok(db)
}

pub fn open_memory() -> rusqlite::Result<Db> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let db = Db { conn: Mutex::new(conn) };
    db.migrate()?;
    Ok(db)
}

impl Db {
    fn migrate(&self) -> rusqlite::Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute_batch(SCHEMA)?;
        Ok(())
    }

    pub fn create_project(&self, name: &str, clock: &dyn Clock) -> rusqlite::Result<ProjectRow> {
        let created = clock.now().to_rfc3339();
        let basis = clock.basis();
        let id = fid(&["project", name, &created]);
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO projects(id,name,created_iso,clock_basis,algo_version) VALUES(?1,?2,?3,?4,?5)",
            params![id, name, created, basis, ALGORITHM_VERSION],
        )?;
        c.execute(
            "INSERT INTO branches(project_id,name,head_event_seq,created_iso) VALUES(?1,'main',0,?2)",
            params![id, created],
        )?;
        let p = Params::default();
        c.execute(
            "INSERT INTO params_versions(project_id,version,parent_version,fingerprint,json,created_event_id) VALUES(?1,1,NULL,?2,?3,NULL)",
            params![id, p.fingerprint, serde_json::to_string(&p).unwrap()],
        )?;
        Ok(ProjectRow {
            id,
            name: name.into(),
            created_iso: created,
            clock_basis: basis,
            algo_version: ALGORITHM_VERSION.into(),
        })
    }

    pub fn list_projects(&self) -> rusqlite::Result<Vec<ProjectRow>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id,name,created_iso,clock_basis,algo_version FROM projects ORDER BY created_iso,id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(ProjectRow {
                id: r.get(0)?,
                name: r.get(1)?,
                created_iso: r.get(2)?,
                clock_basis: r.get(3)?,
                algo_version: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn project_exists(&self, id: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT 1 FROM projects WHERE id=?1", params![id], |_| Ok(()))
            .optional()
            .unwrap_or(None)
            .is_some()
    }

    pub fn get_project(&self, id: &str) -> Option<ProjectRow> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,name,created_iso,clock_basis,algo_version FROM projects WHERE id=?1",
                params![id],
                |r| {
                    Ok(ProjectRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        created_iso: r.get(2)?,
                        clock_basis: r.get(3)?,
                        algo_version: r.get(4)?,
                    })
                },
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn branch_head(&self, project_id: &str, branch: &str) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT head_event_seq FROM branches WHERE project_id=?1 AND name=?2",
                params![project_id, branch],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None)
            .unwrap_or(0)
    }

    pub fn list_branches(&self, project_id: &str) -> Vec<(String, i64, Option<String>)> {
        let c = self.conn.lock().unwrap();
        let mut st = c
            .prepare("SELECT name,head_event_seq,origin_event_id FROM branches WHERE project_id=?1 ORDER BY name")
            .unwrap();
        st.query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .filter_map(|x| x.ok())
            .collect()
    }

    pub fn create_branch(&self, project_id: &str, name: &str, origin_event_id: &str, clock: &dyn Clock) -> rusqlite::Result<()> {
        let now = clock.now().to_rfc3339();
        self.conn.lock().unwrap().execute(
            "INSERT INTO branches(project_id,name,head_event_seq,created_iso,origin_event_id) VALUES(?1,?2,0,?3,?4)",
            params![project_id, name, now, origin_event_id],
        )?;
        Ok(())
    }

    pub fn params_at(&self, project_id: &str, version: i64) -> Option<Params> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT json FROM params_versions WHERE project_id=?1 AND version=?2",
                params![project_id, version],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap_or(None)
            .and_then(|j| serde_json::from_str(&j).ok())
    }

    pub fn params_versions(&self, project_id: &str) -> Vec<(i64, Option<i64>, String, Option<String>)> {
        let c = self.conn.lock().unwrap();
        let mut st = c
            .prepare("SELECT version,parent_version,fingerprint,created_event_id FROM params_versions WHERE project_id=?1 ORDER BY version")
            .unwrap();
        st.query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .filter_map(|x| x.ok())
            .collect()
    }

    pub fn add_params_version(&self, project_id: &str, p: &Params, event_id: &str) -> rusqlite::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO params_versions(project_id,version,parent_version,fingerprint,json,created_event_id) VALUES(?1,?2,?3,?4,?5,?6)",
            params![project_id, p.version, p.parent_version, p.fingerprint, serde_json::to_string(p).unwrap(), event_id],
        )?;
        Ok(())
    }

    pub fn event_by_id(&self, id: &str) -> Option<EventRec> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,branch,seq,kind,payload,parent_event_id,at_iso,clock_basis FROM events WHERE id=?1",
                params![id],
                |r| {
                    Ok(EventRec {
                        id: r.get(0)?,
                        branch: r.get(1)?,
                        seq: r.get(2)?,
                        kind: r.get(3)?,
                        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                        parent_event_id: r.get(5)?,
                        at_iso: r.get(6)?,
                        clock_basis: r.get(7)?,
                    })
                },
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn event_at(&self, project_id: &str, branch: &str, seq: i64) -> Option<EventRec> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,branch,seq,kind,payload,parent_event_id,at_iso,clock_basis FROM events WHERE project_id=?1 AND branch=?2 AND seq=?3",
                params![project_id, branch, seq],
                |r| {
                    Ok(EventRec {
                        id: r.get(0)?,
                        branch: r.get(1)?,
                        seq: r.get(2)?,
                        kind: r.get(3)?,
                        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                        parent_event_id: r.get(5)?,
                        at_iso: r.get(6)?,
                        clock_basis: r.get(7)?,
                    })
                },
            )
            .optional()
            .unwrap_or(None)
    }
}

/// Append a branch event and advance the branch head.
pub fn append_event(
    db: &Db,
    project_id: &str,
    branch: &str,
    kind: &str,
    payload: &Value,
    parent_event_id: Option<&str>,
    clock: &dyn Clock,
) -> rusqlite::Result<(String, i64)> {
    let mut c = db.conn.lock().unwrap();
    let tx = c.transaction()?;
    let seq: i64 = tx
        .query_row(
            "SELECT head_event_seq+1 FROM branches WHERE project_id=?1 AND name=?2",
            params![project_id, branch],
            |r| r.get(0),
        )
        .unwrap_or(1);
    let id = fid(&["evt", project_id, branch, &seq.to_string()]);
    let now = clock.now().to_rfc3339();
    let basis = clock.basis();
    tx.execute(
        "INSERT INTO events(id,project_id,branch,seq,kind,payload,parent_event_id,at_iso,clock_basis)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            id,
            project_id,
            branch,
            seq,
            kind,
            serde_json::to_string(payload).unwrap(),
            parent_event_id,
            now,
            basis
        ],
    )?;
    tx.execute(
        "UPDATE branches SET head_event_seq=?1 WHERE project_id=?2 AND name=?3",
        params![seq, project_id, branch],
    )?;
    tx.commit()?;
    Ok((id, seq))
}

/// Full causal ancestry of a branch tip (or an arbitrary seq), following
/// fork-origin events across branches. Returned in causal order.
pub fn branch_ancestry(db: &Db, project_id: &str, branch: &str, at_seq: Option<i64>) -> Vec<EventRec> {
    let tip = at_seq.unwrap_or_else(|| db.branch_head(project_id, branch));
    let mut ordered: Vec<EventRec> = Vec::new();
    let mut cursor: Option<(String, i64)> = Some((branch.to_string(), tip));
    let mut guard = 0usize;
    while let Some((b, s)) = cursor {
        guard += 1;
        if guard > 100_000 || s <= 0 {
            break;
        }
        let Some(ev) = db.event_at(project_id, &b, s) else { break };
        let parent = ev.parent_event_id.clone();
        ordered.push(ev);
        cursor = match parent {
            None => {
                if s > 1 {
                    Some((b, s - 1))
                } else {
                    None
                }
            }
            Some(pid) => {
                if let Some(origin) = pid.strip_prefix("event:") {
                    if let Some(orig) = db.event_by_id(origin) {
                        Some((orig.branch, orig.seq))
                    } else if s > 1 {
                        Some((b, s - 1))
                    } else {
                        None
                    }
                } else if s > 1 {
                    Some((b, s - 1))
                } else {
                    None
                }
            }
        };
    }
    ordered.reverse();
    ordered
}

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  created_iso TEXT NOT NULL,
  clock_basis TEXT NOT NULL,
  algo_version TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS params_versions (
  project_id TEXT NOT NULL REFERENCES projects(id),
  version INTEGER NOT NULL,
  parent_version INTEGER,
  fingerprint TEXT NOT NULL,
  json TEXT NOT NULL,
  created_event_id TEXT,
  PRIMARY KEY(project_id, version)
);
CREATE TABLE IF NOT EXISTS branches (
  project_id TEXT NOT NULL REFERENCES projects(id),
  name TEXT NOT NULL,
  head_event_seq INTEGER NOT NULL,
  created_iso TEXT NOT NULL,
  origin_event_id TEXT,
  PRIMARY KEY(project_id, name)
);
CREATE TABLE IF NOT EXISTS events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  branch TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  parent_event_id TEXT,
  at_iso TEXT NOT NULL,
  clock_basis TEXT NOT NULL,
  UNIQUE(project_id, branch, seq)
);
CREATE TABLE IF NOT EXISTS source_batches (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  label TEXT NOT NULL,
  version INTEGER NOT NULL,
  kind TEXT NOT NULL,
  file_name TEXT NOT NULL,
  content TEXT NOT NULL,
  format TEXT NOT NULL,
  parent_batch_id TEXT,
  created_event_id TEXT NOT NULL,
  created_iso TEXT NOT NULL,
  UNIQUE(project_id, label, version)
);
CREATE TABLE IF NOT EXISTS raw_epochs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  batch_id TEXT NOT NULL REFERENCES source_batches(id),
  station TEXT NOT NULL,
  epoch_iso TEXT NOT NULL,
  ord INTEGER NOT NULL,
  epoch_s INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_obs (
  id TEXT NOT NULL PRIMARY KEY,
  natural_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  epoch_id TEXT NOT NULL,
  epoch_iso TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  band_num INTEGER NOT NULL,
  code TEXT NOT NULL,
  c_raw TEXT, l_raw TEXT, d_raw TEXT, s_raw TEXT, lli_raw TEXT,
  c_val DOUBLE, l_val DOUBLE, d_val DOUBLE, s_val DOUBLE, lli_val INTEGER,
  diag TEXT,
  ord INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_states (
  id TEXT NOT NULL PRIMARY KEY,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  station TEXT NOT NULL,
  epoch_iso TEXT NOT NULL,
  sat TEXT NOT NULL,
  elev_deg DOUBLE, az_deg DOUBLE, health TEXT,
  raw_line TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_clock_events (
  id TEXT NOT NULL PRIMARY KEY,
  project_id TEXT NOT NULL,
  batch_id TEXT NOT NULL,
  station TEXT NOT NULL,
  epoch_iso TEXT NOT NULL,
  kind TEXT NOT NULL,
  note TEXT NOT NULL,
  raw_line TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  branch TEXT NOT NULL,
  seq INTEGER NOT NULL,
  algorithm_version TEXT NOT NULL,
  params_version INTEGER NOT NULL,
  params_fp TEXT NOT NULL,
  basis_event_id TEXT NOT NULL,
  input_summary TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  reason TEXT NOT NULL,
  created_iso TEXT NOT NULL,
  UNIQUE(project_id, branch, seq)
);
CREATE TABLE IF NOT EXISTS arcs (
  id TEXT PRIMARY KEY,
  natural_id TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(id),
  project_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  band_num INTEGER NOT NULL,
  ord INTEGER NOT NULL,
  start_epoch_id TEXT NOT NULL,
  end_epoch_id TEXT NOT NULL,
  split_reason TEXT NOT NULL,
  n_epochs INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS detector_obs (
  id TEXT PRIMARY KEY,
  natural_id TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(id),
  arc_id TEXT,
  boundary_epoch_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  detector TEXT NOT NULL,
  band_num INTEGER,
  residual_cycles DOUBLE,
  unit TEXT NOT NULL,
  status TEXT NOT NULL,
  source_obs_ids TEXT NOT NULL,
  expected_cycles DOUBLE,
  dt_s DOUBLE
);
CREATE TABLE IF NOT EXISTS candidates (
  id TEXT PRIMARY KEY,
  natural_id TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(id),
  arc_id TEXT,
  boundary_epoch_id TEXT NOT NULL,
  station TEXT NOT NULL,
  sat TEXT NOT NULL,
  band_num INTEGER NOT NULL,
  candidate_kind TEXT NOT NULL,
  n1 INTEGER,
  n2 INTEGER,
  slip_cycles_l1 DOUBLE NOT NULL,
  score DOUBLE NOT NULL,
  status TEXT NOT NULL,
  supporting_detectors TEXT NOT NULL,
  reject_reason TEXT,
  superseded_by TEXT
);
CREATE TABLE IF NOT EXISTS decisions (
  id TEXT PRIMARY KEY,
  event_id TEXT NOT NULL,
  project_id TEXT NOT NULL REFERENCES projects(id),
  branch TEXT NOT NULL,
  run_id TEXT NOT NULL,
  candidate_natural_key TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  note TEXT NOT NULL,
  actor TEXT NOT NULL,
  at_iso TEXT NOT NULL,
  clock_basis TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS repairs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  branch TEXT NOT NULL,
  sat TEXT NOT NULL,
  band_num INTEGER NOT NULL,
  from_epoch_id TEXT NOT NULL,
  offset_cycles DOUBLE NOT NULL,
  decision_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS judge_events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  branch TEXT NOT NULL,
  run_id TEXT,
  kind TEXT NOT NULL,
  epoch_iso TEXT,
  sat TEXT,
  band_num INTEGER,
  payload TEXT NOT NULL,
  event_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_events_branch ON events(project_id, branch, seq);
CREATE INDEX IF NOT EXISTS idx_obs_natural ON raw_obs(project_id, epoch_iso, sat, band_num);
CREATE INDEX IF NOT EXISTS idx_arcs_run ON arcs(run_id);
CREATE INDEX IF NOT EXISTS idx_det_run ON detector_obs(run_id);
CREATE INDEX IF NOT EXISTS idx_cand_run ON candidates(run_id);
CREATE INDEX IF NOT EXISTS idx_judge_project ON judge_events(project_id, branch);
"#;
