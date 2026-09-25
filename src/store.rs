//! SQLite 持久化：事件日志为唯一事实来源，派生结果可随时重放。
//! 每个项目独立数据库文件（本地优先，无外部数据库依赖）。

use crate::decision::{Decision, Event, EventKind};
use crate::ids::content_id;
use crate::model::RawImport;
use crate::params::DetectorParams;
use crate::project::ParamsRevision;
use crate::state::fold_events;
use rusqlite::Connection;
use std::path::Path;

pub struct Store {
    pub conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  TEXT NOT NULL,
    event_id    TEXT NOT NULL,
    at          TEXT NOT NULL,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, seq);
"#;

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// 追加事件，返回带全局序号的事件（序号决定顺序，导出再导入保持）。
    pub fn append(&mut self, project_id: &str, kind: EventKind, at: &str, payload: serde_json::Value) -> rusqlite::Result<Event> {
        let material = format!("{project_id}\u{1f}{at}\u{1f}{:?}\u{1f}{payload}", kind);
        let event_id = content_id("evt", &material);
        self.conn.execute(
            "INSERT INTO events(project_id,event_id,at,kind,payload) VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![project_id, event_id, at, format!("{kind:?}"), payload.to_string()],
        )?;
        let seq = self.conn.last_insert_rowid() as u64;
        Ok(Event { id: event_id, seq, at: at.to_string(), kind, payload })
    }

    pub fn events(&self, project_id: &str) -> rusqlite::Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq,event_id,at,kind,payload FROM events WHERE project_id=?1 ORDER BY seq")?;
        let rows = stmt.query_map(rusqlite::params![project_id], |r| {
            let seq: i64 = r.get(0)?;
            let event_id: String = r.get(1)?;
            let at: String = r.get(2)?;
            let kind_s: String = r.get(3)?;
            let payload_s: String = r.get(4)?;
            let kind = match kind_s.as_str() {
                "ProjectCreated" => EventKind::ProjectCreated,
                "Imported" => EventKind::Imported,
                "ParamsRevised" => EventKind::ParamsRevised,
                "DecisionRecorded" => EventKind::DecisionRecorded,
                "ForkCreated" => EventKind::ForkCreated,
                other => panic!("unknown event kind {other}"),
            };
            let payload = serde_json::from_str(&payload_s).unwrap();
            Ok(Event { id: event_id, seq: seq as u64, at, kind, payload })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn project(&self, project_id: &str) -> rusqlite::Result<Option<crate::project::Project>> {
        let events = self.events(project_id)?;
        if events.is_empty() {
            return Ok(None);
        }
        Ok(Some(fold_events(project_id, &events)))
    }

    pub fn max_seq(&self, project_id: &str) -> rusqlite::Result<u64> {
        let v: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM events WHERE project_id=?1", rusqlite::params![project_id], |r| r.get(0))?;
        Ok(v.unwrap_or(0) as u64)
    }

    /// 导出项目为确定性 bundle（事件按 seq 排序，JSON 键排序）。
    pub fn export_bundle(&self, project_id: &str) -> rusqlite::Result<String> {
        let events = self.events(project_id)?;
        let bundle = serde_json::json!({
            "bundle_format": "carrier-break-station/v1",
            "project_id": project_id,
            "events": events,
        });
        // 稳定序列化：BTreeMap/Value 默认按 key 排序。
        let v = sort_json(bundle);
        Ok(serde_json::to_string_pretty(&v).unwrap())
    }

    /// 从 bundle 导入：保留原 seq/event_id（身份、顺序、来源关系不变）。
    pub fn import_bundle(&mut self, text: &str) -> rusqlite::Result<String> {
        let bundle: serde_json::Value =
            serde_json::from_str(text).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let project_id = bundle["project_id"].as_str().expect("project_id").to_string();
        let events = bundle["events"].as_array().expect("events array");
        let tx = self.conn.transaction()?;
        for ev in events {
            let seq = ev["seq"].as_u64().expect("seq") as i64;
            let event_id = ev["id"].as_str().expect("id");
            let at = ev["at"].as_str().expect("at");
            let kind = ev["kind"].as_str().expect("kind");
            let payload = ev["payload"].clone();
            // 幂等：同 project+seq 已存在则跳过。
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM events WHERE project_id=?1 AND seq=?2",
                rusqlite::params![project_id, seq],
                |r| r.get(0),
            )?;
            if exists == 0 {
                tx.execute(
                    "INSERT INTO events(seq,project_id,event_id,at,kind,payload) VALUES(?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![seq, project_id, event_id, at, kind, payload.to_string()],
                )?;
            }
        }
        tx.commit()?;
        Ok(project_id)
    }
}

fn sort_json(v: serde_json::Value) -> serde_json::Value {
    use serde_json::{Map, Value};
    match v {
        Value::Object(m) => {
            let mut sorted: Map<String, Value> = Map::new();
            let mut keys: Vec<String> = m.keys().cloned().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k, sort_json(m[&k].clone()));
            }
            Value::Object(sorted)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sort_json).collect()),
        other => other,
    }
}

/// 便捷的高层操作：创建项目 + 初始导入 + 初始参数。
pub fn bootstrap_project(
    store: &mut Store,
    project_id: &str,
    name: &str,
    imp: RawImport,
    params: &DetectorParams,
    clock: &mut crate::timeutil::VirtualClock,
) -> rusqlite::Result<()> {
    store.append(project_id, EventKind::ProjectCreated, &clock.tick(), serde_json::json!({"name": name}))?;
    store.append(project_id, EventKind::Imported, &clock.tick(), serde_json::json!({"import": imp}))?;
    let rev = ParamsRevision {
        fingerprint: params.fingerprint(),
        algo_version: params.algo_version.clone(),
        params: params.clone(),
        note: "baseline rule set".into(),
        created_at: clock.tick(),
    };
    store.append(project_id, EventKind::ParamsRevised, &rev.created_at, serde_json::to_value(&rev).unwrap())?;
    Ok(())
}

pub fn append_import(
    store: &mut Store,
    project_id: &str,
    imp: RawImport,
    clock: &mut crate::timeutil::VirtualClock,
) -> rusqlite::Result<Event> {
    store.append(project_id, EventKind::Imported, &clock.tick(), serde_json::json!({"import": imp}))
}

pub fn append_params(
    store: &mut Store,
    project_id: &str,
    params: DetectorParams,
    note: &str,
    clock: &mut crate::timeutil::VirtualClock,
) -> rusqlite::Result<Event> {
    let rev = ParamsRevision {
        fingerprint: params.fingerprint(),
        algo_version: params.algo_version.clone(),
        params,
        note: note.to_string(),
        created_at: clock.tick(),
    };
    store.append(project_id, EventKind::ParamsRevised, &rev.created_at, serde_json::to_value(&rev).unwrap())
}

#[allow(clippy::too_many_arguments)]
pub fn append_decision(
    store: &mut Store,
    project_id: &str,
    decision: Decision,
    clock: &mut crate::timeutil::VirtualClock,
) -> rusqlite::Result<Event> {
    store.append(project_id, EventKind::DecisionRecorded, &clock.tick(), serde_json::json!({"decision": decision}))
}
