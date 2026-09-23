//! SQLite 持久化：事件追加表 + 原始输入表 + 派生缓存表。
//! 导出/导入为 JSON 档案；身份由内容哈希决定，重导后保持不变。
use crate::events::{Event, EventPayload};
use crate::ids::{hash_hex, stable_id};
use crate::model::RawBatch;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

pub struct Store {
    pub conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Archive {
    pub format: String,
    pub events: Vec<Event>,
    pub raw_inputs: Vec<RawBatch>,
    pub head: i64,
}

pub const ARCHIVE_FORMAT: &str = "carrier-breakpoint-station/archive/1";

impl Store {
    pub fn open(path: &str) -> Result<Store, String> {
        let conn = Connection::open(path).map_err(|e| format!("打开数据库失败 {path}: {e}"))?;
        let s = Store { conn };
        s.init()?;
        Ok(s)
    }
    pub fn open_memory() -> Result<Store, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let s = Store { conn };
        s.init()?;
        Ok(s)
    }
    fn init(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS events(
                   id INTEGER PRIMARY KEY, parent INTEGER, logical_time INTEGER NOT NULL,
                   payload TEXT NOT NULL, hash TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS raw_inputs(
                   id TEXT PRIMARY KEY, batch_json TEXT NOT NULL, content_sha TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS derived(
                   event_id INTEGER NOT NULL, key TEXT NOT NULL, payload TEXT NOT NULL,
                   PRIMARY KEY(event_id, key));",
            )
            .map_err(|e| e.to_string())
    }

    pub fn get_meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| r.get(0))
            .ok()
    }
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn head(&self) -> i64 {
        self.get_meta("head").and_then(|v| v.parse().ok()).unwrap_or(0)
    }
    pub fn set_head(&self, id: i64) -> Result<(), String> {
        self.set_meta("head", &id.to_string())
    }

    pub fn append_event(&mut self, payload: EventPayload, logical_time: i64) -> Result<Event, String> {
        let head = self.head();
        let parent = if head == 0 { None } else { Some(head) };
        let parent_hash = if head == 0 {
            "genesis".to_string()
        } else {
            self.get_event(head)?.hash
        };
        let payload_json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        let hash = stable_id("ev", &[&parent_hash, &logical_time.to_string(), &payload_json]);
        let id = head + 1;
        self.conn
            .execute(
                "INSERT INTO events(id,parent,logical_time,payload,hash) VALUES(?1,?2,?3,?4,?5)",
                params![id, parent, logical_time, payload_json, hash],
            )
            .map_err(|e| e.to_string())?;
        self.set_head(id)?;
        Ok(Event { id, parent, logical_time, payload, hash })
    }

    /// 分叉追加：parent 指向任意旧事件，head 移到新事件。
    pub fn fork_event(&mut self, parent: i64, payload: EventPayload, logical_time: i64) -> Result<Event, String> {
        let parent_hash = if parent == 0 {
            "genesis".to_string()
        } else {
            self.get_event(parent)?.hash
        };
        let payload_json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        let hash = stable_id("ev", &[&parent_hash, &logical_time.to_string(), &payload_json]);
        let id: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(id),0)+1 FROM events", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "INSERT INTO events(id,parent,logical_time,payload,hash) VALUES(?1,?2,?3,?4,?5)",
                params![id, Some(parent).filter(|p| *p > 0), logical_time, payload_json, hash],
            )
            .map_err(|e| e.to_string())?;
        self.set_head(id)?;
        Ok(Event { id, parent: Some(parent).filter(|p| *p > 0), logical_time, payload, hash })
    }

    pub fn get_event(&self, id: i64) -> Result<Event, String> {
        self.conn
            .query_row(
                "SELECT id,parent,logical_time,payload,hash FROM events WHERE id=?1",
                params![id],
                |r| {
                    let payload_json: String = r.get(3)?;
                    Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, i64>(2)?, payload_json, r.get::<_, String>(4)?))
                },
            )
            .map_err(|e| format!("事件 {id} 不存在: {e}"))
            .and_then(|(id, parent, lt, pj, hash)| {
                let payload: EventPayload =
                    serde_json::from_str(&pj).map_err(|e| format!("事件 {id} 负载损坏: {e}"))?;
                Ok(Event { id, parent, logical_time: lt, payload, hash })
            })
    }

    pub fn list_events(&self) -> Result<Vec<Event>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM events ORDER BY id")
            .map_err(|e| e.to_string())?;
        let ids: Vec<i64> = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        ids.into_iter().map(|id| self.get_event(id)).collect()
    }

    /// 从 genesis 到 `to` 的事件链（沿父指针回溯后正序）。
    pub fn chain_to(&self, to: i64) -> Result<Vec<Event>, String> {
        let mut chain = Vec::new();
        let mut cur = to;
        while cur > 0 {
            let ev = self.get_event(cur)?;
            chain.push(ev.clone());
            cur = ev.parent.unwrap_or(0);
        }
        chain.reverse();
        Ok(chain)
    }

    pub fn put_raw(&self, batch: &RawBatch) -> Result<(), String> {
        let json = serde_json::to_string(batch).map_err(|e| e.to_string())?;
        let sha = hash_hex(json.as_bytes());
        if sha != batch.content_sha {
            return Err(format!("批次 {} 内容哈希不一致", batch.id));
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO raw_inputs(id,batch_json,content_sha) VALUES(?1,?2,?3)",
                params![batch.id, json, sha],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_raw(&self, id: &str) -> Result<RawBatch, String> {
        let json: String = self
            .conn
            .query_row("SELECT batch_json FROM raw_inputs WHERE id=?1", params![id], |r| r.get(0))
            .map_err(|e| format!("原始批次 {id} 不存在: {e}"))?;
        serde_json::from_str(&json).map_err(|e| format!("原始批次 {id} 反序列化失败: {e}"))
    }

    pub fn put_derived(&self, event_id: i64, key: &str, payload_json: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO derived(event_id,key,payload) VALUES(?1,?2,?3)",
                params![event_id, key, payload_json],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn get_derived(&self, event_id: i64, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT payload FROM derived WHERE event_id=?1 AND key=?2",
                params![event_id, key],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn export_archive(&self) -> Result<Archive, String> {
        let events = self.list_events()?;
        let mut raw_inputs = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM raw_inputs ORDER BY rowid")
            .map_err(|e| e.to_string())?;
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for id in ids {
            raw_inputs.push(self.get_raw(&id)?);
        }
        Ok(Archive { format: ARCHIVE_FORMAT.to_string(), events, raw_inputs, head: self.head() })
    }

    /// 导入档案：要求当前库无事件（典型场景：删除本地数据库后再导入）。
    /// 身份、顺序、来源关系完全由档案内容决定。
    pub fn import_archive(&mut self, archive: &Archive) -> Result<(), String> {
        if archive.format != ARCHIVE_FORMAT {
            return Err(format!("档案格式不支持：{}", archive.format));
        }
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if n > 0 {
            return Err("数据库非空：请先删除本地数据库再导入档案".to_string());
        }
        for batch in &archive.raw_inputs {
            self.put_raw(batch)?;
        }
        for ev in &archive.events {
            let payload_json = serde_json::to_string(&ev.payload).map_err(|e| e.to_string())?;
            let expect = stable_id(
                "ev",
                &[
                    &match ev.parent {
                        None => "genesis".to_string(),
                        Some(p) => self.get_event(p)?.hash,
                    },
                    &ev.logical_time.to_string(),
                    &payload_json,
                ],
            );
            if expect != ev.hash {
                return Err(format!("事件 {} 哈希链校验失败", ev.id));
            }
            self.conn
                .execute(
                    "INSERT INTO events(id,parent,logical_time,payload,hash) VALUES(?1,?2,?3,?4,?5)",
                    params![ev.id, ev.parent, ev.logical_time, payload_json, ev.hash],
                )
                .map_err(|e| e.to_string())?;
        }
        self.set_head(archive.head)?;
        Ok(())
    }
}
