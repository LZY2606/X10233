//! SQLite 持久化:原始输入不可覆盖(修订=新行+父版本),事件追加,派生值带
//! 算法版本/参数指纹/直接来源,过期仅打标不删除。

use rusqlite::{params, Connection};
use serde_json::Value;

pub struct Store {
    pub conn: Connection,
}

#[derive(Clone, Debug)]
pub struct InputRow {
    pub id: String,
    pub kind: String,
    pub parent_id: Option<String>,
    pub content: String,
    pub created_seq: i64,
}

#[derive(Clone, Debug)]
pub struct EventRow {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub branch: String,
    pub payload: Value,
    pub time_ms: i64,
}

#[derive(Clone, Debug)]
pub struct DerivedRow {
    pub id: String,
    pub kind: String,
    pub algo: String,
    pub param_fp: String,
    pub branch: String,
    pub source_ids: Vec<String>,
    pub payload: Value,
    pub created_seq: i64,
    pub stale_since_seq: Option<i64>,
    pub superseded_seq: Option<i64>,
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let s = Store { conn };
        s.init()?;
        Ok(s)
    }

    pub fn memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Store { conn };
        s.init()?;
        Ok(s)
    }

    fn init(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
             CREATE TABLE IF NOT EXISTS inputs(
               id TEXT PRIMARY KEY, kind TEXT, parent_id TEXT,
               content TEXT, created_seq INTEGER);
             CREATE TABLE IF NOT EXISTS events(
               seq INTEGER PRIMARY KEY, id TEXT, kind TEXT, branch TEXT,
               payload TEXT, time_ms INTEGER);
             CREATE TABLE IF NOT EXISTS derived(
               id TEXT, kind TEXT, algo TEXT, param_fp TEXT, branch TEXT,
               source_ids TEXT, payload TEXT, created_seq INTEGER,
               stale_since_seq INTEGER, superseded_seq INTEGER,
               PRIMARY KEY(id, created_seq));
             CREATE TABLE IF NOT EXISTS branches(
               id TEXT PRIMARY KEY, parent TEXT, fork_seq INTEGER, label TEXT);",
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO meta(k,v) VALUES('seq','0')",
            [],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO branches(id,parent,fork_seq,label) VALUES('main',NULL,0,'主线')",
            [],
        )
    }

    pub fn next_seq(&self) -> rusqlite::Result<i64> {
        let cur: i64 = self
            .conn
            .query_row("SELECT v FROM meta WHERE k='seq'", [], |r| r.get::<_, String>(0))?
            .parse()
            .unwrap();
        let nxt = cur + 1;
        self.conn
            .execute("UPDATE meta SET v=?1 WHERE k='seq'", params![nxt.to_string()])?;
        Ok(nxt)
    }

    pub fn new_id(&self, prefix: &str) -> rusqlite::Result<String> {
        Ok(format!("{}_{:06}", prefix, self.next_seq()?))
    }

    pub fn insert_input(
        &self,
        id: &str,
        kind: &str,
        parent: Option<&str>,
        content: &str,
        seq: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO inputs(id,kind,parent_id,content,created_seq) VALUES(?1,?2,?3,?4,?5)",
            params![id, kind, parent, content, seq],
        )?;
        Ok(())
    }

    pub fn insert_event(
        &self,
        seq: i64,
        id: &str,
        kind: &str,
        branch: &str,
        payload: &Value,
        time_ms: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO events(seq,id,kind,branch,payload,time_ms) VALUES(?1,?2,?3,?4,?5,?6)",
            params![seq, id, kind, branch, payload.to_string(), time_ms],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_derived(
        &self,
        id: &str,
        kind: &str,
        algo: &str,
        param_fp: &str,
        branch: &str,
        source_ids: &[String],
        payload: &Value,
        created_seq: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO derived(id,kind,algo,param_fp,branch,source_ids,payload,created_seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                kind,
                algo,
                param_fp,
                branch,
                serde_json::to_string(source_ids).unwrap(),
                payload.to_string(),
                created_seq
            ],
        )?;
        Ok(())
    }

    /// 把依赖某个来源的派生行标记为过期(仍可查看,但不能冒充当前结果)。
    pub fn mark_stale_by_source(&self, source_id: &str, seq: i64) -> rusqlite::Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT rowid, source_ids FROM derived WHERE stale_since_seq IS NULL AND superseded_seq IS NULL")?;
        let rows: Vec<(i64, Vec<String>)> = stmt
            .query_map([], |r| {
                let ids: String = r.get(1)?;
                Ok((r.get::<_, i64>(0)?, serde_json::from_str(&ids).unwrap_or_default()))
            })?
            .filter_map(|r| r.ok())
            .collect();
        let mut n = 0;
        for (rowid, ids) in rows {
            if ids.iter().any(|s| s == source_id) {
                self.conn.execute(
                    "UPDATE derived SET stale_since_seq=?1 WHERE rowid=?2",
                    params![seq, rowid],
                )?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// 用新计算替换某分支某类派生行(旧的标记 superseded,保留可查)。
    pub fn supersede_kind(&self, kind: &str, branch: &str, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE derived SET superseded_seq=?1 WHERE kind=?2 AND branch=?3 AND superseded_seq IS NULL",
            params![seq, kind, branch],
        )?;
        Ok(())
    }

    pub fn list_inputs(&self) -> rusqlite::Result<Vec<InputRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,kind,parent_id,content,created_seq FROM inputs ORDER BY created_seq")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(InputRow {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    parent_id: r.get(2)?,
                    content: r.get(3)?,
                    created_seq: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_events(&self) -> rusqlite::Result<Vec<EventRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq,id,kind,branch,payload,time_ms FROM events ORDER BY seq")?;
        let rows = stmt
            .query_map([], |r| {
                let payload: String = r.get(4)?;
                Ok(EventRow {
                    seq: r.get(0)?,
                    id: r.get(1)?,
                    kind: r.get(2)?,
                    branch: r.get(3)?,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    time_ms: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_derived(&self) -> rusqlite::Result<Vec<DerivedRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,kind,algo,param_fp,branch,source_ids,payload,created_seq,stale_since_seq,superseded_seq
             FROM derived ORDER BY created_seq, id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let src: String = r.get(5)?;
                let payload: String = r.get(6)?;
                Ok(DerivedRow {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    algo: r.get(2)?,
                    param_fp: r.get(3)?,
                    branch: r.get(4)?,
                    source_ids: serde_json::from_str(&src).unwrap_or_default(),
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    created_seq: r.get(7)?,
                    stale_since_seq: r.get(8)?,
                    superseded_seq: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn add_branch(&self, id: &str, parent: &str, fork_seq: i64, label: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO branches(id,parent,fork_seq,label) VALUES(?1,?2,?3,?4)",
            params![id, parent, fork_seq, label],
        )?;
        Ok(())
    }

    pub fn list_branches(&self) -> rusqlite::Result<Vec<(String, Option<String>, i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,parent,fork_seq,label FROM branches ORDER BY rowid")?;
        Ok(stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)) )?
            .filter_map(|r| r.ok())
            .collect())
    }

    pub fn clear_all(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM inputs; DELETE FROM events; DELETE FROM derived;
             DELETE FROM branches; UPDATE meta SET v='0' WHERE k='seq';
             INSERT OR IGNORE INTO branches(id,parent,fork_seq,label) VALUES('main',NULL,0,'主线');",
        )
    }
}
