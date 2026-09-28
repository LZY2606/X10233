//! SQLite 事件溯源存储。原始层与事件是真相来源；派生 run 是缓存，可随时从事件重建。

use crate::domain::engine::{self, Candidate, DetectorParams, Resolution, RunResult};
use crate::domain::events::{Event, EventPayload};
use crate::domain::model::{ClockEventInput, SatelliteStateInput};
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
use std::sync::Mutex;

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  station TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS branches (
  project_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  name TEXT NOT NULL,
  parent_branch_id TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, branch_id)
);
CREATE TABLE IF NOT EXISTS events (
  project_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  PRIMARY KEY (project_id, branch_id, seq)
);
CREATE TABLE IF NOT EXISTS runs (
  project_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  at_seq INTEGER NOT NULL,
  run_json TEXT NOT NULL,
  PRIMARY KEY (project_id, branch_id, run_id)
);
"#;

pub struct Store {
    conn: Mutex<Connection>,
}

pub trait Clock: Send + Sync {
    fn now_iso(&self) -> String;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_iso(&self) -> String {
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }
}
pub struct VirtualClock {
    pub iso: std::sync::Mutex<String>,
}
impl VirtualClock {
    pub fn new(iso: &str) -> Self {
        Self { iso: std::sync::Mutex::new(iso.to_string()) }
    }
    pub fn set(&self, iso: &str) {
        *self.iso.lock().unwrap() = iso.to_string();
    }
    pub fn advance_secs(&self, secs: i64) {
        let mut g = self.iso.lock().unwrap();
        let dt = chrono::NaiveDateTime::parse_from_str(&g, "%Y-%m-%dT%H:%M:%S%.3fZ")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(&g, "%Y-%m-%dT%H:%M:%S%.3f"))
            .unwrap();
        *g = (dt + chrono::Duration::seconds(secs)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    }
}
impl Clock for VirtualClock {
    fn now_iso(&self) -> String {
        self.iso.lock().unwrap().clone()
    }
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn conn_lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    pub fn create_project(&self, id: &str, name: &str, station: &str, clock: &dyn Clock) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO projects(id,name,station,created_at) VALUES(?1,?2,?3,?4)",
            params![id, name, station, clock.now_iso()],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO branches(project_id,branch_id,name,parent_branch_id,created_at) VALUES(?1,'main','main',NULL,?2)",
            params![id, clock.now_iso()],
        )?;
        Ok(())
    }

    pub fn project_station(&self, id: &str) -> rusqlite::Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT name, station FROM projects WHERE id=?1")?;
        let mut rows = st.query(params![id])?;
        if let Some(r) = rows.next()? {
            Ok(Some((r.get(0)?, r.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn list_projects(&self) -> rusqlite::Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT id,name,station FROM projects ORDER BY created_at,id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    pub fn list_branches(&self, project_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT branch_id FROM branches WHERE project_id=?1 ORDER BY created_at,branch_id")?;
        let rows = st.query_map(params![project_id], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    pub fn create_branch(&self, project_id: &str, branch_id: &str, name: &str, clock: &dyn Clock) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branches(project_id,branch_id,name,parent_branch_id,created_at) VALUES(?1,?2,?3,'main',?4)",
            params![project_id, branch_id, name, clock.now_iso()],
        )?;
        Ok(())
    }

    /// 从 main 分叉：复制截至 from_seq 的事件。
    pub fn fork_branch(&self, project_id: &str, new_branch: &str, name: &str, from_seq: i64, clock: &dyn Clock) -> rusqlite::Result<Vec<Event>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO branches(project_id,branch_id,name,parent_branch_id,created_at) VALUES(?1,?2,?3,'main',?4)",
            params![project_id, new_branch, name, clock.now_iso()],
        )?;
        let mut st = tx.prepare(
            "SELECT seq,id,created_at,payload_json FROM events WHERE project_id=?1 AND branch_id='main' AND seq<=?2 ORDER BY seq",
        )?;
        let rows = st.query_map(params![project_id, from_seq], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut copied = Vec::new();
        for row in rows {
            let (seq, id, created_at, payload_json) = row?;
            tx.execute(
                "INSERT INTO events(project_id,branch_id,seq,id,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",
                params![project_id, new_branch, seq, id, created_at, payload_json],
            )?;
            copied.push(Event {
                seq,
                id,
                branch_id: new_branch.to_string(),
                created_at,
                payload: serde_json::from_str(&payload_json).unwrap(),
            });
        }
        tx.commit()?;
        Ok(copied)
    }

    pub fn append_event(&self, project_id: &str, branch_id: &str, payload: EventPayload, clock: &dyn Clock) -> rusqlite::Result<Event> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let next: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE project_id=?1 AND branch_id=?2",
                params![project_id, branch_id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let payload_json = serde_json::to_string(&payload).unwrap();
        let id = format!("evt:{}:{}:{}", project_id, branch_id, crate::infra::hash::short_hash(&format!("{next}:{payload_json}")));
        let created_at = clock.now_iso();
        tx.execute(
            "INSERT INTO events(project_id,branch_id,seq,id,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![project_id, branch_id, next, id, created_at, payload_json],
        )?;
        tx.commit()?;
        Ok(Event { seq: next, id, branch_id: branch_id.to_string(), created_at, payload })
    }

    pub fn events(&self, project_id: &str, branch_id: &str) -> rusqlite::Result<Vec<Event>> {
        self.events_until(project_id, branch_id, i64::MAX)
    }

    pub fn events_until(&self, project_id: &str, branch_id: &str, until_seq: i64) -> rusqlite::Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT seq,id,created_at,payload_json FROM events WHERE project_id=?1 AND branch_id=?2 AND seq<=?3 ORDER BY seq",
        )?;
        let rows = st.query_map(params![project_id, branch_id, until_seq], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, id, created_at, payload_json) = row?;
            out.push(Event { seq, id, branch_id: branch_id.to_string(), created_at, payload: serde_json::from_str(&payload_json).unwrap() });
        }
        Ok(out)
    }

    pub fn save_run(&self, project_id: &str, branch_id: &str, at_seq: i64, run: &RunResult) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO runs(project_id,branch_id,run_id,at_seq,run_json) VALUES(?1,?2,?3,?4,?5)",
            params![project_id, branch_id, run.run_id, at_seq, serde_json::to_string(run).unwrap()],
        )?;
        Ok(())
    }

    pub fn load_run(&self, project_id: &str, branch_id: &str, run_id: &str) -> rusqlite::Result<Option<(i64, RunResult)>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT at_seq,run_json FROM runs WHERE project_id=?1 AND branch_id=?2 AND run_id=?3")?;
        let mut rows = st.query(params![project_id, branch_id, run_id])?;
        if let Some(r) = rows.next()? {
            let at: i64 = r.get(0)?;
            let j: String = r.get(1)?;
            Ok(Some((at, serde_json::from_str(&j).unwrap())))
        } else {
            Ok(None)
        }
    }
}

/// 回放某分支（截至可选 seq）得到的纯函数状态。
pub struct ReplayState {
    pub project_name: String,
    pub station: String,
    pub commits: Vec<(String, engine::ParsedImport)>,
    pub clock_events: Vec<ClockEventInput>,
    pub sat_states: Vec<SatelliteStateInput>,
    pub params: DetectorParams,
    pub params_version: u64,
    pub current_run: Option<String>,
    pub decisions: Vec<DecisionRec>,
}

#[derive(Clone, Debug)]
pub struct DecisionRec {
    pub run_id: String,
    pub candidate_id: String,
    pub action: String,
    pub b1: Option<i64>,
    pub b2: Option<i64>,
    pub note: String,
    pub at_seq: i64,
}

pub fn replay(project_name: &str, station: &str, events: &[Event]) -> ReplayState {
    let mut st = ReplayState {
        project_name: project_name.to_string(),
        station: station.to_string(),
        commits: vec![],
        clock_events: vec![],
        sat_states: vec![],
        params: DetectorParams::default(),
        params_version: 0,
        current_run: None,
        decisions: vec![],
    };
    for e in events {
        match &e.payload {
            EventPayload::ProjectCreated { name, station: sname } => {
                st.project_name = name.clone();
                st.station = sname.clone();
            }
            EventPayload::ObservationsCommitted { commit_id, parsed, .. } => {
                st.commits.push((commit_id.clone(), parsed.clone()));
            }
            EventPayload::ClockEventsCommitted { events: evs } => st.clock_events.extend(evs.iter().cloned()),
            EventPayload::SatStatesCommitted { states } => st.sat_states.extend(states.iter().cloned()),
            EventPayload::ParamsRevised { version, params, .. } => {
                st.params_version = *version;
                st.params = params.clone();
            }
            EventPayload::Computed { run_id, .. } => st.current_run = Some(run_id.clone()),
            EventPayload::DecisionMade { run_id, candidate_id, action, chosen_offset_b1, chosen_offset_b2, note, .. } => {
                st.decisions.push(DecisionRec {
                    run_id: run_id.clone(),
                    candidate_id: candidate_id.clone(),
                    action: action.clone(),
                    b1: *chosen_offset_b1,
                    b2: *chosen_offset_b2,
                    note: note.clone(),
                    at_seq: e.seq,
                });
            }
            EventPayload::BranchCreated { .. } => {}
        }
    }
    st
}

/// 把决定应用到 run 的候选（返回应用后的候选；run 本身不被改写，修复只存为偏移层决定）。
pub fn apply_decisions(run: &mut RunResult, decisions: &[DecisionRec]) {
    let by: BTreeMap<String, &DecisionRec> = decisions
        .iter()
        .filter(|d| d.run_id == run.run_id)
        .map(|d| (d.candidate_id.clone(), d))
        .collect();
    for c in run.candidates.iter_mut() {
        if let Some(d) = by.get(&c.id) {
            c.resolution = match d.action.as_str() {
                "accept" | "hypothesis_selected" => Resolution::Accepted,
                "reject" => Resolution::RejectedByHuman,
                "reset" => Resolution::Pending,
                _ => Resolution::Pending,
            };
        }
    }
}
