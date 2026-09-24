//! SQLite 持久化层。所有身份为内容寻址；派生结果带 result_set/context，
//! 局部输入变化只使依赖结果过期，过期结果保留可查但不冒充当前结果。

use crate::engine::{param_fingerprint, Computation, FoldedRecord, OffsetLayer, ALGO_VERSION};
use crate::events::{Event, EventKind, Rationale};
use crate::hash::{sha256_hex, stable_id};
use crate::model::*;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::BTreeMap;

pub struct Db {
    pub conn: Connection,
}

/// 项目
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub created_unix_ms: i64,
    pub head_branch: String,
}

/// 一次输入提交（含父提交链）
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitRow {
    pub id: String,
    pub project_id: String,
    pub seq: i64,
    pub parent_commit_id: Option<String>,
    pub kind: String, // pack | rinex | import-bundle
    pub payload_json: String,
    pub raw_blob: String,
    pub created_unix_ms: i64,
}

/// 原始记录版本（同一槽位多版本）
#[derive(Debug, Clone, serde::Serialize)]
pub struct RawRow {
    pub id: String,
    pub commit_id: String,
    pub project_id: String,
    pub seq: i64,
    pub slot_key: String,
    pub version: i64,
    pub deleted: i64,
    pub obs_json: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DerivedRow {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub result_set_id: String,
    pub context_hash: String,
    pub kind: String, // boundary | arc | clock
    pub payload_json: String,
    pub source_records_json: String,
    pub algo_version: String,
    pub param_fingerprint: String,
    pub seq_in_set: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResultSetRow {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub head_event_id: String,
    pub context_hash: String,
    pub computation_json: String,
    pub created_unix_ms: i64,
}

const SCHEMA: &str = include_str!("schema.sql");

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn })
    }

    /// 内存库（测试）
    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn })
    }

    pub fn create_project(&self, name: &str, now_ms: i64) -> rusqlite::Result<ProjectRow> {
        let id = stable_id("proj", &format!("{name}|{now_ms}"));
        self.conn.execute(
            "INSERT INTO project(id,name,created_unix_ms,head_branch) VALUES(?1,?2,?3,'main')",
            params![id, name, now_ms],
        )?;
        self.conn.execute(
            "INSERT INTO branch(project_id,name,parent_branch,fork_point_event_id,head_event_id,created_unix_ms) \
             VALUES(?1,'main',NULL,NULL,NULL,?2)",
            params![id, now_ms],
        )?;
        Ok(ProjectRow { id, name: name.to_string(), created_unix_ms: now_ms, head_branch: "main".into() })
    }

    pub fn get_project(&self, id: &str) -> rusqlite::Result<ProjectRow> {
        self.conn.query_row(
            "SELECT id,name,created_unix_ms,head_branch FROM project WHERE id=?1",
            params![id],
            |r| Ok(ProjectRow { id: r.get(0)?, name: r.get(1)?, created_unix_ms: r.get(2)?, head_branch: r.get(3)? }),
        )
    }

    pub fn list_projects(&self) -> rusqlite::Result<Vec<ProjectRow>> {
        let mut stmt = self.conn.prepare("SELECT id,name,created_unix_ms,head_branch FROM project ORDER BY created_unix_ms,id")?;
        let rows = stmt.query_map([], |r| {
            Ok(ProjectRow { id: r.get(0)?, name: r.get(1)?, created_unix_ms: r.get(2)?, head_branch: r.get(3)? })
        })?;
        rows.collect()
    }

    pub fn set_head(&self, project_id: &str, branch: &str, head_event_id: &str) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE branch SET head_event_id=?1 WHERE project_id=?2 AND name=?3", params![
            head_event_id, project_id, branch
        ])?;
        Ok(())
    }

    pub fn branch_head(&self, project_id: &str, branch: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row(
            "SELECT head_event_id FROM branch WHERE project_id=?1 AND name=?2",
            params![project_id, branch],
            |r| r.get::<_, Option<String>>(0),
        )
    }

    pub fn list_branches(&self, project_id: &str) -> rusqlite::Result<Vec<BranchInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT name,parent_branch,fork_point_event_id,head_event_id,created_unix_ms FROM branch WHERE project_id=?1 ORDER BY created_unix_ms,name",
        )?;
        let rows = stmt.query_map(params![project_id], |r| {
            Ok(BranchInfo {
                name: r.get(0)?,
                parent_branch: r.get(1)?,
                fork_point_event_id: r.get(2)?,
                head_event_id: r.get(3)?,
                created_unix_ms: r.get(4)?,
            })
        })?;
        rows.collect()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub parent_branch: Option<String>,
    pub fork_point_event_id: Option<String>,
    pub head_event_id: Option<String>,
    pub created_unix_ms: i64,
}
