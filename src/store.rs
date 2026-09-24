//! SQLite persistence with explicit event lineage.
//!
//! Every mutation appends an immutable event with a parent event id (forming
//! a tree of branches). Raw inputs are never overwritten: each import is a new
//! input version pointing at its parent. Derived findings are cached and
//! marked superseded only when their direct source keys change, so local
//! input changes invalidate only dependent results.
use crate::engine::{self, DecisionState, ProjectView};
use crate::hash::{canonical_json, sha256_short};
use crate::model::{ImportedFile, Params};
use crate::parser;
use crate::vtime::VClock;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub struct Store {
    pub conn: Connection,
    pub clock: VClock,
}

#[derive(Debug, Clone)]
pub struct EventInfo {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub branch: String,
    pub kind: String,
    pub label: String,
    pub payload: Value,
    pub created_at: String,
    pub input_version_id: Option<String>,
    pub param_version_id: Option<String>,
    pub project_id: String,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub head_event_id: String,
    pub event: EventInfo,
    pub input_version_id: String,
    pub param_version_id: String,
    pub file: ImportedFile,
    pub params: Params,
    pub decisions: DecisionState,
    pub view: ProjectView,
}

#[derive(Debug, Clone, Default)]
pub struct DecisionRecord {
    pub id: String,
    pub event_id: String,
    pub project_id: String,
    pub subject_key: String,
    pub action: String,
    pub n1: Option<i64>,
    pub n2: Option<i64>,
    pub rationale: String,
    pub superseded: i64,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  head_event_id TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  project_id TEXT NOT NULL,
  parent_id TEXT,
  branch TEXT NOT NULL,
  kind TEXT NOT NULL,
  label TEXT NOT NULL,
  payload TEXT NOT NULL,
  input_version_id TEXT,
  param_version_id TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS input_versions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  version_no INTEGER NOT NULL,
  parent_id TEXT,
  raw_text TEXT NOT NULL,
  raw_sha256 TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_epochs (
  id TEXT PRIMARY KEY,
  input_version_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  time_raw TEXT NOT NULL,
  time_s REAL,
  json_lines TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_clock_events (
  id TEXT PRIMARY KEY,
  input_version_id TEXT NOT NULL,
  epoch_seq INTEGER NOT NULL,
  time_raw TEXT NOT NULL,
  jump_us REAL,
  source TEXT,
  raw TEXT,
  unit TEXT,
  diagnostic TEXT
);
CREATE TABLE IF NOT EXISTS raw_measurements (
  id TEXT PRIMARY KEY,
  input_version_id TEXT NOT NULL,
  epoch_seq INTEGER NOT NULL,
  time_raw TEXT NOT NULL,
  satellite TEXT NOT NULL,
  obs_code TEXT NOT NULL,
  kind TEXT NOT NULL,
  band TEXT NOT NULL,
  value_f64 REAL,
  value_int INTEGER,
  missing INTEGER NOT NULL,
  raw TEXT NOT NULL,
  unit TEXT NOT NULL,
  diagnostic TEXT
);
CREATE TABLE IF NOT EXISTS raw_frequencies (
  id TEXT PRIMARY KEY,
  input_version_id TEXT NOT NULL,
  epoch_seq INTEGER NOT NULL,
  satellite TEXT NOT NULL,
  entries_json TEXT NOT NULL,
  source TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_satstates (
  id TEXT PRIMARY KEY,
  input_version_id TEXT NOT NULL,
  epoch_seq INTEGER NOT NULL,
  satellite TEXT NOT NULL,
  entries_json TEXT NOT NULL,
  iono_l1_m REAL,
  clk_bias_s REAL
);
CREATE TABLE IF NOT EXISTS parse_diagnostics (
  id TEXT PRIMARY KEY,
  input_version_id TEXT NOT NULL,
  line_no INTEGER NOT NULL,
  severity TEXT NOT NULL,
  code TEXT NOT NULL,
  message TEXT NOT NULL,
  raw_line TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS param_versions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  version_no INTEGER NOT NULL,
  parent_id TEXT,
  params_json TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS decisions (
  id TEXT PRIMARY KEY,
  event_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  subject_key TEXT NOT NULL,
  action TEXT NOT NULL,
  n1 INTEGER,
  n2 INTEGER,
  rationale TEXT NOT NULL,
  superseded INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS finding_rows (
  row_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  input_version_id TEXT NOT NULL,
  param_version_id TEXT NOT NULL,
  subject_key TEXT NOT NULL,
  source_keys_json TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  superseded INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (row_id)
);
CREATE TABLE IF NOT EXISTS repair_offsets (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  subject_key TEXT NOT NULL,
  band TEXT NOT NULL,
  cycles INTEGER NOT NULL,
  event_id TEXT NOT NULL,
  layer TEXT NOT NULL DEFAULT 'offset'
);
"#;

impl Store {
    pub fn open(path: &str, clock: VClock) -> rusqlite::Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            if let Some(parent) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            Connection::open(path)?
        };
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn, clock })
    }

    pub fn reset(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM repair_offsets; DELETE FROM finding_rows; DELETE FROM decisions;
             DELETE FROM param_versions; DELETE FROM parse_diagnostics; DELETE FROM raw_satstates;
             DELETE FROM raw_frequencies; DELETE FROM raw_measurements; DELETE FROM raw_clock_events;
             DELETE FROM raw_epochs; DELETE FROM input_versions; DELETE FROM events; DELETE FROM projects;",
        )
    }

    pub fn create_project(&mut self, id: &str, name: &str) -> rusqlite::Result<String> {
        let exists: Option<i64> = self
            .conn
            .query_row("SELECT 1 FROM projects WHERE id=?1", params![id], |_| Ok(1))
            .optional()?;
        if exists.is_some() {
            return Ok(id.into());
        }
        let now = self.clock.now_iso();
        self.conn.execute(
            "INSERT INTO projects(id,name,head_event_id,created_at) VALUES(?1,?2,NULL,?3)",
            params![id, name, now],
        )?;
        Ok(id.into())
    }

    pub fn list_projects(&self) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,head_event_id,created_at FROM projects ORDER BY created_at,id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "head_event_id": r.get::<_, Option<String>>(2)?,
                "created_at": r.get::<_, String>(3)?,
            }))
        })?;
        let mut out = vec![];
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn next_event_seq(&self, project: &str) -> rusqlite::Result<i64> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE project_id=?1",
                params![project],
                |r| r.get(0),
            )
            .unwrap_or(1);
        Ok(n)
    }

    fn head_id(&self, project: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row(
            "SELECT head_event_id FROM projects WHERE id=?1",
            params![project],
            |r| r.get(0),
        )
    }

    fn set_head(&mut self, project: &str, event_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE projects SET head_event_id=?1 WHERE id=?2",
            params![event_id, project],
        )?;
        Ok(())
    }

    /// Append a new immutable input version. Raw text is stored verbatim and
    /// parsed into typed rows; nothing from the previous version is mutated.
    pub fn import_input(
        &mut self,
        project: &str,
        label: &str,
        raw_text: &str,
        branch: Option<String>,
    ) -> rusqlite::Result<EventInfo> {
        self.create_project(project, project)?;
        let parent = self.head_id(project)?;
        let file = parser::parse(raw_text);
        let raw_sha = crate::hash::sha256_hex(raw_text.as_bytes());
        let fp_input = sha256_short(raw_text.as_bytes());
        let version_no: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version_no),0)+1 FROM input_versions WHERE project_id=?1",
                params![project],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let now = self.clock.now_iso();
        let seq = self.next_event_seq(project)?;
        let input_id = format!("iv-{version_no}-{}", &raw_sha[..12]);
        let payload = serde_json::json!({
            "label": label,
            "version_no": version_no,
            "raw_sha256": raw_sha,
            "input_fingerprint": fp_input,
            "diagnostic_count": file.diagnostics.len(),
            "epoch_count": file.epochs.len(),
        });
        let event_id = make_event_id(project, seq, "input", &payload, &now);
        let br = branch.unwrap_or_else(|| "main".into());
        self.conn.execute(
            "INSERT INTO events(id,seq,project_id,parent_id,branch,kind,label,payload,input_version_id,param_version_id,created_at)
             VALUES(?1,?2,?3,?4,?5,'input',?6,?7,?8,NULL,?9)",
            params![
                event_id,
                seq,
                project,
                parent,
                br,
                label,
                payload.to_string(),
                input_id,
                now
            ],
        )?;
        self.conn.execute(
            "INSERT INTO input_versions(id,project_id,version_no,parent_id,raw_text,raw_sha256,fingerprint,created_event_id,created_at)
             VALUES(?1,?2,?3,(SELECT input_version_id FROM events WHERE id=?4),?5,?6,?7,?8,?9)",
            params![input_id, project, version_no, event_id, raw_text, raw_sha, fp_input, event_id, now],
        )?;
        self.store_typed_rows(&input_id, &file)?;
        self.set_head(project, &event_id)?;
        self.recompute_at_head(project)?;
        self.get_event(&event_id)
    }

    fn store_typed_rows(&mut self, input_id: &str, file: &ImportedFile) -> rusqlite::Result<()> {
        for e in &file.epochs {
            let epoch_row_id = format!("ep-{input_id}-{}", e.seq);
            self.conn.execute(
                "INSERT INTO raw_epochs(id,input_version_id,seq,time_raw,time_s,json_lines)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    epoch_row_id,
                    input_id,
                    e.seq,
                    e.time_raw,
                    e.time_s,
                    serde_json::to_string(&e.sats).unwrap_or_default()
                ],
            )?;
            for fr in &e.frequencies {
                let id = format!("fq-{input_id}-{}-{}", e.seq, fr.satellite);
                self.conn.execute(
                    "INSERT INTO raw_frequencies(id,input_version_id,epoch_seq,satellite,entries_json,source)
                     VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        id,
                        input_id,
                        e.seq,
                        fr.satellite,
                        serde_json::to_string(&fr.entries).unwrap_or_default(),
                        fr.source
                    ],
                )?;
            }
            for ce in &e.clock_events {
                let id = format!("cj-{input_id}-{}", e.seq);
                self.conn.execute(
                    "INSERT INTO raw_clock_events(id,input_version_id,epoch_seq,time_raw,jump_us,source,raw,unit,diagnostic)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        id,
                        input_id,
                        e.seq,
                        e.time_raw,
                        ce.jump_us,
                        ce.source,
                        ce.raw,
                        ce.unit,
                        ce.diagnostic
                    ],
                )?;
            }
            for ss in &e.sat_states {
                let id = format!("ss-{input_id}-{}-{}", e.seq, ss.satellite);
                self.conn.execute(
                    "INSERT INTO raw_satstates(id,input_version_id,epoch_seq,satellite,entries_json,iono_l1_m,clk_bias_s)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        id,
                        input_id,
                        e.seq,
                        ss.satellite,
                        serde_json::to_string(&ss.entries).unwrap_or_default(),
                        ss.iono_l1_m,
                        ss.clk_bias_s
                    ],
                )?;
            }
            for so in &e.sats {
                for m in &so.measurements {
                    let id = format!("m-{input_id}-{}-{}-{}", e.seq, so.satellite, m.obs_code);
                    self.conn.execute(
                        "INSERT INTO raw_measurements(id,input_version_id,epoch_seq,time_raw,satellite,obs_code,kind,band,value_f64,value_int,missing,raw,unit,diagnostic)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                        params![
                            id,
                            input_id,
                            e.seq,
                            e.time_raw,
                            so.satellite,
                            m.obs_code,
                            m.kind.as_str(),
                            m.band,
                            m.value_f64,
                            m.value_int,
                            m.missing as i64,
                            m.raw,
                            m.unit,
                            m.diagnostic
                        ],
                    )?;
                }
            }
        }
        for d in &file.diagnostics {
            let id = format!("dg-{input_id}-{}-{}", d.line_no, d.code);
            self.conn.execute(
                "INSERT INTO parse_diagnostics(id,input_version_id,line_no,severity,code,message,raw_line)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![id, input_id, d.line_no, d.severity, d.code, d.message, d.raw_line],
            )?;
        }
        Ok(())
    }

    /// Append a new parameter version (parent preserved; old contexts remain
    /// rewindable and recomputable).
    pub fn revise_params(
        &mut self,
        project: &str,
        label: &str,
        params: &Params,
        branch: Option<String>,
    ) -> rusqlite::Result<EventInfo> {
        self.create_project(project, project)?;
        let parent = self.head_id(project)?;
        let version_no: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version_no),0)+1 FROM param_versions WHERE project_id=?1",
                params![project],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let pval = serde_json::to_value(params).unwrap();
        let fp = crate::hash::fingerprint_value(&pval);
        let now = self.clock.now_iso();
        let seq = self.next_event_seq(project)?;
        let param_id = format!("pv-{version_no}-{fp}");
        let payload = serde_json::json!({
            "label": label,
            "version_no": version_no,
            "params": pval,
            "param_fingerprint": fp,
        });
        let event_id = make_event_id(project, seq, "params", &payload, &now);
        let br = branch.unwrap_or_else(|| "main".into());
        self.conn.execute(
            "INSERT INTO events(id,seq,project_id,parent_id,branch,kind,label,payload,input_version_id,param_version_id,created_at)
             VALUES(?1,?2,?3,?4,?5,'params',?6,?7,
                (SELECT input_version_id FROM events WHERE id=?4),?8,?9)",
            params![event_id, seq, project, parent, br, label, payload.to_string(), param_id, now],
        )?;
        self.conn.execute(
            "INSERT INTO param_versions(id,project_id,version_no,parent_id,params_json,fingerprint,created_event_id,created_at)
             VALUES(?1,?2,?3,(SELECT param_version_id FROM events WHERE id=?4),?5,?6,?7,?8)",
            params![param_id, project, version_no, event_id, pval.to_string(), fp, event_id, now],
        )?;
        self.set_head(project, &event_id)?;
        self.recompute_at_head(project)?;
        self.get_event(&event_id)
    }

    /// Record accept/reject/undo/acknowledge. Accepting a conflicting offset
    /// on an existing subject forks a new branch unless `branch` is supplied.
    pub fn record_decision(
        &mut self,
        project: &str,
        subject_key: &str,
        action: &str,
        n1: Option<i64>,
        n2: Option<i64>,
        rationale: &str,
        branch: Option<String>,
    ) -> rusqlite::Result<EventInfo> {
        let parent = self
            .head_id(project)?
            .ok_or_else(|| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other("project has no events"))))?;
        let parent_ev = self.get_event(&parent)?;
        let prior = self.decision_for_subject_at(&parent, subject_key)?;
        let mut br = branch.unwrap_or_else(|| parent_ev.branch.clone());
        if action == "accept" {
            if let Some(pr) = &prior {
                if pr.action == "accept" && (pr.n1, pr.n2) != (n1, n2) {
                    // conflicting human decision: fork explicitly
                    br = next_branch_name(&self.conn, project)?;
                }
            }
        }
        let now = self.clock.now_iso();
        let seq = self.next_event_seq(project)?;
        let payload = serde_json::json!({
            "subject_key": subject_key,
            "action": action,
            "n1": n1,
            "n2": n2,
            "rationale": rationale,
            "forked_from": if br != parent_ev.branch { Some(parent.clone()) } else { None },
        });
        let event_id = make_event_id(project, seq, "decision", &payload, &now);
        self.conn.execute(
            "INSERT INTO events(id,seq,project_id,parent_id,branch,kind,label,payload,input_version_id,param_version_id,created_at)
             VALUES(?1,?2,?3,?4,?5,'decision',?6,?7,?8,?9,?10)",
            params![
                event_id,
                seq,
                project,
                parent,
                br,
                format!("{action} {subject_key}"),
                payload.to_string(),
                parent_ev.input_version_id,
                parent_ev.param_version_id,
                now
            ],
        )?;
        let did = format!("dec-{event_id}");
        self.conn.execute(
            "INSERT INTO decisions(id,event_id,project_id,subject_key,action,n1,n2,rationale,superseded)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
            params![did, event_id, project, subject_key, action, n1, n2, rationale],
        )?;
        if let Some(pr) = &prior {
            self.conn.execute(
                "UPDATE decisions SET superseded=1 WHERE id=?1",
                params![pr.id],
            )?;
        }
        if action == "accept" {
            if let (Some(a), Some(b)) = (n1, n2) {
                for (band, cycles) in [("L1", a), ("L2", b)] {
                    let off_id = format!("off-{event_id}-{band}");
                    self.conn.execute(
                        "INSERT INTO repair_offsets(id,project_id,subject_key,band,cycles,event_id,layer)
                         VALUES(?1,?2,?3,?4,?5,?6,'offset')",
                        params![off_id, project, subject_key, band, cycles, event_id],
                    )?;
                }
            }
        }
        self.set_head(project, &event_id)?;
        self.recompute_at_head(project)?;
        self.get_event(&event_id)
    }

    pub fn rewind_head(&mut self, project: &str, event_id: &str) -> rusqlite::Result<()> {
        // View-only move: does not delete or mutate the latest branch.
        self.get_event(event_id)?;
        self.set_head(project, event_id)
    }

    pub fn get_event(&self, id: &str) -> rusqlite::Result<EventInfo> {
        self.conn.query_row(
            "SELECT id,seq,parent_id,branch,kind,label,payload,created_at,input_version_id,param_version_id,project_id
             FROM events WHERE id=?1",
            params![id],
            |r| {
                let payload: String = r.get(6)?;
                Ok(EventInfo {
                    id: r.get(0)?,
                    seq: r.get(1)?,
                    parent_id: r.get(2)?,
                    branch: r.get(3)?,
                    kind: r.get(4)?,
                    label: r.get(5)?,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    created_at: r.get(7)?,
                    input_version_id: r.get(8)?,
                    param_version_id: r.get(9)?,
                    project_id: r.get(10)?,
                })
            },
        )
    }

    /// Walk parent links from an event back to the root.
    fn lineage(&self, event_id: &str) -> rusqlite::Result<Vec<EventInfo>> {
        let mut chain = vec![];
        let mut cur = Some(event_id.to_string());
        while let Some(id) = cur {
            let ev = self.get_event(&id)?;
            cur = ev.parent_id.clone();
            chain.push(ev);
        }
        chain.reverse();
        Ok(chain)
    }

    fn decision_state_at(&self, event_id: &str) -> rusqlite::Result<DecisionState> {
        let chain = self.lineage(event_id)?;
        let mut st = DecisionState::new();
        for ev in &chain {
            if ev.kind != "decision" {
                continue;
            }
            let sub = ev
                .payload
                .get("subject_key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let action = ev
                .payload
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match action {
                "accept" => {
                    let n1 = ev.payload.get("n1").and_then(|v| v.as_i64());
                    let n2 = ev.payload.get("n2").and_then(|v| v.as_i64());
                    st.rejected.remove(&sub);
                    if let (Some(a), Some(b)) = (n1, n2) {
                        st.accepted.insert(sub, (a, b));
                    }
                }
                "reject" => {
                    st.accepted.remove(&sub);
                    st.rejected.insert(sub);
                }
                "undo" => {
                    st.accepted.remove(&sub);
                    st.rejected.remove(&sub);
                }
                "acknowledge_clock" | "acknowledge_lli" => {
                    st.acknowledged.insert(sub);
                }
                _ => {}
            }
        }
        Ok(st)
    }

    fn decision_for_subject_at(
        &self,
        event_id: &str,
        subject: &str,
    ) -> rusqlite::Result<Option<DecisionRecord>> {
        let chain = self.lineage(event_id)?;
        let mut recs: BTreeMap<String, DecisionRecord> = BTreeMap::new();
        for ev in &chain {
            if ev.kind != "decision" {
                continue;
            }
            let sub = ev
                .payload
                .get("subject_key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sub != subject {
                continue;
            }
            recs.insert(
                ev.id.clone(),
                DecisionRecord {
                    id: format!("dec-{}", ev.id),
                    event_id: ev.id.clone(),
                    project_id: ev.project_id.clone(),
                    subject_key: sub,
                    action: ev.payload.get("action").and_then(|v| v.as_str()).unwrap_or("").into(),
                    n1: ev.payload.get("n1").and_then(|v| v.as_i64()),
                    n2: ev.payload.get("n2").and_then(|v| v.as_i64()),
                    rationale: ev
                        .payload
                        .get("rationale")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    superseded: 0,
                },
            );
        }
        Ok(recs.into_values().last())
    }

    fn load_input(&self, input_id: &str) -> rusqlite::Result<ImportedFile> {
        let raw: String = self.conn.query_row(
            "SELECT raw_text FROM input_versions WHERE id=?1",
            params![input_id],
            |r| r.get(0),
        )?;
        Ok(parser::parse(&raw))
    }

    fn default_params(&self, _project: &str) -> rusqlite::Result<(String, Params)> {
        let params = Params::default();
        let pval = serde_json::to_value(&params).unwrap();
        let fp = crate::hash::fingerprint_value(&pval);
        Ok((format!("pv-default-{fp}"), params))
    }

    fn load_params(&self, param_id: Option<&str>, project: &str) -> rusqlite::Result<(String, Params)> {
        if let Some(pid) = param_id {
            let json: String = self.conn.query_row(
                "SELECT params_json FROM param_versions WHERE id=?1",
                params![pid],
                |r| r.get(0),
            )?;
            let p: Params = serde_json::from_str(&json).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(format!("params: {e}"))))
            })?;
            return Ok((pid.to_string(), p));
        }
        self.default_params(project)
    }

    fn input_fingerprint(&self, input_id: &str) -> rusqlite::Result<String> {
        self.conn.query_row(
            "SELECT fingerprint FROM input_versions WHERE id=?1",
            params![input_id],
            |r| r.get(0),
        )
    }

    /// Full context at any event (used for history rewind and normal views).
    pub fn context_at(&self, event_id: &str) -> rusqlite::Result<Context> {
        let ev = self.get_event(event_id)?;
        let input_id = ev
            .input_version_id
            .clone()
            .ok_or_else(|| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other("event without input"))))?;
        let (param_used, params) = self.load_params(ev.param_version_id.as_deref(), &ev.project_id)?;
        let file = self.load_input(&input_id)?;
        let fp = self.input_fingerprint(&input_id)?;
        let decisions = self.decision_state_at(event_id)?;
        let view = engine::compute(engine::ComputeInput {
            file: &file,
            params: &params,
            input_fingerprint: fp,
            decisions: &decisions,
        });
        Ok(Context {
            head_event_id: event_id.into(),
            event: ev,
            input_version_id: input_id,
            param_version_id: param_used,
            file,
            params,
            decisions,
            view,
        })
    }

    pub fn head_context(&self, project: &str) -> rusqlite::Result<Option<Context>> {
        match self.head_id(project)? {
            Some(id) => Ok(Some(self.context_at(&id)?)),
            None => Ok(None),
        }
    }

    pub fn list_events(&self, project: &str) -> rusqlite::Result<Vec<EventInfo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,seq,parent_id,branch,kind,label,payload,created_at,input_version_id,param_version_id,project_id
                      FROM events WHERE project_id=?1 ORDER BY seq,id")?;
        let rows = stmt.query_map(params![project], map_event)?;
        let mut out = vec![];
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Recompute findings at the current head and cache them. Partial
    /// invalidation: a cached finding from the previous event is carried over
    /// unchanged iff the event did not change any of its direct source keys;
    /// otherwise the old row is marked superseded and a fresh one inserted.
    /// Stale rows remain readable and never overwrite history.
    pub fn recompute_at_head(&mut self, project: &str) -> rusqlite::Result<()> {
        let Some(head) = self.head_id(project)? else {
            return Ok(());
        };
        let head_ev = self.get_event(&head)?;
        let Some(input_id) = head_ev.input_version_id.clone() else {
            return Ok(());
        };
        let (param_id, params) = self.load_params(head_ev.param_version_id.as_deref(), project)?;
        let file = self.load_input(&input_id)?;
        let fp = self.input_fingerprint(&input_id)?;
        let decisions = self.decision_state_at(&head)?;
        let view = engine::compute(engine::ComputeInput {
            file: &file,
            params: &params,
            input_fingerprint: fp.clone(),
            decisions: &decisions,
        });

        // Previous current rows (latest event carrying each subject).
        let prev_event = head_ev.parent_id.clone();
        let prev_rows = self.current_finding_rows(project, prev_event.as_deref())?;
        let prev_by_subject: BTreeMap<String, FindingRowCache> =
            prev_rows.into_iter().map(|r| (r.subject_key.clone(), r)).collect();

        // Current findings by subject.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for f in &view.findings {
            seen.insert(f.subject_key.clone());
            let payload = serde_json::to_value(f).unwrap();
            let source_keys: BTreeSet<String> = f.source_direct.iter().cloned().collect();
            let row_id = format!("frow-{}-{}", head, f.id);
            if let Some(old) = prev_by_subject.get(&f.subject_key) {
                let old_sources: BTreeSet<String> =
                    serde_json::from_str(&old.source_keys_json).unwrap_or_default();
                let sources_unchanged = old_sources == source_keys;
                let params_unchanged = old.param_version_id == param_id;
                let input_unchanged = old.input_version_id == input_id;
                if sources_unchanged && params_unchanged && input_unchanged {
                    // carry-over: local change did not touch this subject and
                    // rules/input identity match. Insert an identical row but
                    // anchored at the new event so head views are complete.
                    self.conn.execute(
                        "INSERT INTO finding_rows(row_id,finding_id,project_id,event_id,input_version_id,param_version_id,subject_key,source_keys_json,payload_json,superseded)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0)",
                        params![
                            row_id,
                            f.id,
                            project,
                            head,
                            input_id,
                            param_id,
                            f.subject_key,
                            serde_json::to_string(&source_keys).unwrap(),
                            payload.to_string()
                        ],
                    )?;
                    continue;
                } else {
                    // only this subject's old row becomes stale
                    self.conn.execute(
                        "UPDATE finding_rows SET superseded=1 WHERE row_id=?1",
                        params![old.row_id],
                    )?;
                }
            }
            self.conn.execute(
                "INSERT INTO finding_rows(row_id,finding_id,project_id,event_id,input_version_id,param_version_id,subject_key,source_keys_json,payload_json,superseded)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0)",
                params![
                    row_id,
                    f.id,
                    project,
                    head,
                    input_id,
                    param_id,
                    f.subject_key,
                    serde_json::to_string(&source_keys).unwrap(),
                    payload.to_string()
                ],
            )?;
        }

        // Subjects that disappeared at head (e.g. missing fixed): supersede.
        for (subject, old) in prev_by_subject.iter() {
            if !seen.contains(subject) {
                self.conn.execute(
                    "UPDATE finding_rows SET superseded=1 WHERE row_id=?1",
                    params![old.row_id],
                )?;
            }
        }
        Ok(())
    }

    fn current_finding_rows(
        &self,
        project: &str,
        at_event: Option<&str>,
    ) -> rusqlite::Result<Vec<FindingRowCache>> {
        let anchor = match at_event {
            Some(e) => e.to_string(),
            None => return Ok(vec![]),
        };
        let mut stmt = self.conn.prepare(
            "SELECT row_id,finding_id,event_id,input_version_id,param_version_id,subject_key,source_keys_json
             FROM finding_rows WHERE project_id=?1 AND event_id=?2 AND superseded=0",
        )?;
        let rows = stmt.query_map(params![project, anchor], |r| {
            Ok(FindingRowCache {
                row_id: r.get(0)?,
                finding_id: r.get(1)?,
                event_id: r.get(2)?,
                input_version_id: r.get(3)?,
                param_version_id: r.get(4)?,
                subject_key: r.get(5)?,
                source_keys_json: r.get(6)?,
            })
        })?;
        let mut out = vec![];
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Merge all rows from another (fresh or deleted-then-recreated) store via
    /// the JSON export bundle. IDs are preserved exactly, including order via
    /// event `seq`.
    pub fn import_bundle(&mut self, bundle: &Value) -> rusqlite::Result<Vec<String>> {
        let mut imported_projects = vec![];
        let arr = bundle
            .get("projects")
            .and_then(|v| v.as_array())
            .ok_or_else(|| bad("bundle missing projects"))?;
        for p in arr {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("demo").to_string();
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            let head = p.get("head_event_id").and_then(|v| v.as_str());
            let created = p.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
            self.create_project(&id, &name)?;
            self.conn.execute(
                "INSERT OR REPLACE INTO projects(id,name,head_event_id,created_at) VALUES(?1,?2,?3,?4)",
                params![id, name, head, created],
            )?;
            imported_projects.push(id.clone());
            for table in [
                "events",
                "input_versions",
                "param_versions",
                "decisions",
                "finding_rows",
                "repair_offsets",
                "raw_epochs",
                "raw_clock_events",
                "raw_measurements",
                "raw_frequencies",
                "raw_satstates",
                "parse_diagnostics",
            ] {
                self.import_table(&id, table, p.get(table).and_then(|v| v.as_array()))?;
            }
        }
        Ok(imported_projects)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn import_table(
        &mut self,
        _project: &str,
        table: &str,
        rows: Option<&Vec<Value>>,
    ) -> rusqlite::Result<()> {
        let Some(rows) = rows else { return Ok(()) };
        for row in rows {
            let cols: Vec<String> = row
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let col_sql = cols.join(",");
            let placeholders = cols.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("INSERT OR REPLACE INTO {table}({col_sql}) VALUES({placeholders})");
            let mut ps = self.conn.prepare(&sql)?;
            let values: Vec<Box<dyn rusqlite::types::ToSql>> = cols
                .iter()
                .map(|c| {
                    let v = &row[c];
                    match v {
                        Value::Null => Box::new(Option::<String>::None) as Box<dyn rusqlite::types::ToSql>,
                        Value::Bool(b) => Box::new(*b as i64),
                        Value::Number(n) => {
                            let b: Box<dyn rusqlite::types::ToSql> = if let Some(i) = n.as_i64() {
                                Box::new(i)
                            } else if let Some(f) = n.as_f64() {
                                Box::new(f)
                            } else {
                                Box::new(n.to_string())
                            };
                            b
                        }
                        Value::String(s) => Box::new(s.clone()),
                        _ => Box::new(v.to_string()),
                    }
                })
                .collect();
            let refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|b| b.as_ref()).collect();
            ps.execute(refs.as_slice())?;
        }
        Ok(())
    }

    pub fn export_bundle(&self, project: &str) -> rusqlite::Result<Value> {
        let project_row: Value = self.conn.query_row(
            "SELECT id,name,head_event_id,created_at FROM projects WHERE id=?1",
            params![project],
            |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_,String>(0)?,
                    "name": r.get::<_,String>(1)?,
                    "head_event_id": r.get::<_,Option<String>>(2)?,
                    "created_at": r.get::<_,String>(3)?,
                }))
            },
        )?;
        let mut obj = project_row.as_object().unwrap().clone();
        for table in [
            "events",
            "input_versions",
            "param_versions",
            "decisions",
            "finding_rows",
            "repair_offsets",
            "raw_epochs",
            "raw_clock_events",
            "raw_measurements",
            "raw_frequencies",
            "raw_satstates",
            "parse_diagnostics",
        ] {
            let arr = self.dump_table(project, table)?;
            obj.insert(table.to_string(), Value::Array(arr));
        }
        Ok(serde_json::json!({
            "bundle_format": "csbench-bundle/1",
            "exported_at": self.clock.now_iso(),
            "projects": [Value::Object(obj)],
        }))
    }

    fn dump_table(&self, project: &str, table: &str) -> rusqlite::Result<Vec<Value>> {
        let has_project = match table {
            "events" | "input_versions" | "param_versions" | "decisions" | "finding_rows"
            | "repair_offsets" => true,
            _ => false,
        };
        let (sql, count) = if has_project {
            (
                format!("SELECT * FROM {table} WHERE project_id=?1 ORDER BY 1"),
                1,
            )
        } else {
            (
                format!(
                    "SELECT t.* FROM {table} t JOIN input_versions iv ON t.input_version_id=iv.id
                     WHERE iv.project_id=?1 ORDER BY 1"
                ),
                1,
            )
        };
        let _ = count;
        let mut stmt = self.conn.prepare(&sql)?;
        let col_count = stmt.column_count();
        let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt.query(params![project])?;
        let mut out = vec![];
        for row in rows.mapped(|r| {
            let mut o = serde_json::Map::new();
            for i in 0..col_count {
                let val: Value = match r.get_ref(i) {
                    Ok(rusqlite::types::ValueRef::Null) => Value::Null,
                    Ok(rusqlite::types::ValueRef::Integer(n)) => serde_json::json!(n),
                    Ok(rusqlite::types::ValueRef::Real(x)) => serde_json::json!(x),
                    Ok(rusqlite::types::ValueRef::Text(t)) => {
                        Value::String(String::from_utf8_lossy(t).to_string())
                    }
                    Ok(rusqlite::types::ValueRef::Blob(b)) => {
                        Value::String(String::from_utf8_lossy(b).to_string())
                    }
                    Err(_) => Value::Null,
                };
                o.insert(names[i].clone(), val);
            }
            Ok(Value::Object(o))
        }) {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn branches(&self, project: &str) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT branch, MAX(seq), id FROM events WHERE project_id=?1 GROUP BY branch ORDER BY branch")?;
        let mut out = vec![];
        for r in stmt.query_map(params![project], |r| {
            Ok(serde_json::json!({"branch": r.get::<_,String>(0)?, "tip_seq": r.get::<_,i64>(1)?}))
        })? {
            out.push(r?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct FindingRowCache {
    row_id: String,
    finding_id: String,
    event_id: String,
    input_version_id: String,
    param_version_id: String,
    subject_key: String,
    source_keys_json: String,
}

fn map_event(r: &rusqlite::Row) -> rusqlite::Result<EventInfo> {
    let payload: String = r.get(6)?;
    Ok(EventInfo {
        id: r.get(0)?,
        seq: r.get(1)?,
        parent_id: r.get(2)?,
        branch: r.get(3)?,
        kind: r.get(4)?,
        label: r.get(5)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        created_at: r.get(7)?,
        input_version_id: r.get(8)?,
        param_version_id: r.get(9)?,
        project_id: r.get(10)?,
    })
}

fn next_branch_name(conn: &Connection, project: &str) -> rusqlite::Result<String> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT branch) FROM events WHERE project_id=?1 AND branch<>'main'",
        params![project],
        |r| r.get(0),
    )?;
    Ok(format!("alt-{}", n + 1))
}

fn make_event_id(project: &str, seq: i64, kind: &str, payload: &Value, now: &str) -> String {
    let canonical = canonical_json(payload);
    let h = sha256_short(format!("{project}|{seq}|{kind}|{canonical}|{now}").as_bytes());
    format!("ev-{seq}-{h}")
}

fn bad(msg: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(msg.to_string())))
}
