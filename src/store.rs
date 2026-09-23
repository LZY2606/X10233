//! SQLite persistence.
//!
//! The store is local-first: one database file, no network dependency. All
//! natural identities are caller-supplied stable strings, so
//! export -> delete -> re-import preserves identity, order and provenance.

use crate::model::{Diagnostic, EpochRecord};
use crate::{clock::Clock, params::Params};
use rusqlite::Connection;
use std::sync::Mutex;

pub struct Store {
    pub conn: Mutex<Connection>,
    pub clock: Box<dyn Clock>,
}

pub const SCHEMA_VERSION: &str = "1";

pub fn open(path: &str, clock: Box<dyn Clock>) -> rusqlite::Result<Store> {
    let conn = if path == ":memory:" {
        Connection::open_in_memory()?
    } else {
        Connection::open(path)?
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    let store = Store {
        conn: Mutex::new(conn),
        clock,
    };
    store.init()?;
    Ok(store)
}

impl Store {
    fn init(&self) -> rusqlite::Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute_batch(include_str!("../sql/schema.sql"))?;
        Ok(())
    }
}

/// Raw epoch snapshot stored under an input version. Epochs are immutable once
/// written; a revision writes a brand-new snapshot with a parent link.
pub fn write_input_snapshot(
    c: &Connection,
    project_id: &str,
    input_version_id: &str,
    seq: i64,
    station: &str,
    payload_json: &str,
    epochs: &[EpochRecord],
    diagnostics: &[Diagnostic],
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO input_versions
            (id, project_id, seq, parent_id, station, raw_json, created_ms)
         VALUES (?1,?2,?3,NULL,?4,?5,0)",
        rusqlite::params![input_version_id, project_id, seq, station, payload_json],
    )?;
    for (idx, e) in epochs.iter().enumerate() {
        let eid = crate::id::id("ep", &[project_id, &e.time]);
        c.execute(
            "INSERT OR IGNORE INTO epochs
                (id, project_id, input_version_id, seq, time, flag, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                eid,
                project_id,
                input_version_id,
                idx as i64,
                e.time,
                e.flag,
                serde_json::to_string(e).unwrap()
            ],
        )?;
    }
    for d in diagnostics {
        let did = crate::id::id("diag", &[
            project_id,
            input_version_id,
            d.time.as_deref().unwrap_or(""),
            d.sat.as_deref().unwrap_or(""),
            &d.code,
            &d.message,
        ]);
        c.execute(
            "INSERT OR IGNORE INTO diagnostics
                (id, project_id, input_version_id, code, message, time, sat, obs_code)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                did,
                project_id,
                input_version_id,
                d.code,
                d.message,
                d.time,
                d.sat,
                d.obs_code
            ],
        )?;
    }
    Ok(())
}

/// Set a revision's parent after the fact (initial import stays NULL).
pub fn link_input_parent(
    c: &Connection,
    input_version_id: &str,
    parent_id: &str,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE input_versions SET parent_id=?2 WHERE id=?1",
        rusqlite::params![input_version_id, parent_id],
    )?;
    Ok(())
}

pub fn record_removed_epochs(
    c: &Connection,
    input_version_id: &str,
    removed_times: &[String],
) -> rusqlite::Result<()> {
    for t in removed_times {
        c.execute(
            "INSERT OR IGNORE INTO epoch_changes (input_version_id, epoch_time, change_kind)
             VALUES (?1,?2,'removed')",
            rusqlite::params![input_version_id, t],
        )?;
    }
    Ok(())
}

pub fn upsert_rule(
    c: &Connection,
    project_id: &str,
    rule_id: &str,
    seq: i64,
    params: &Params,
    note: &str,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO rules
            (id, project_id, seq, fingerprint, algorithm_version, params_json, note)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![
            rule_id,
            project_id,
            seq,
            params.fingerprint(),
            crate::params::ALGORITHM_VERSION,
            serde_json::to_string(params).unwrap(),
            note
        ],
    )?;
    Ok(())
}

pub fn next_seq(c: &Connection, project_id: &str, table: &str, col: &str) -> rusqlite::Result<i64> {
    let q = format!("SELECT COALESCE(MAX({col}),0)+1 FROM {table} WHERE project_id=?1");
    c.query_row(&q, rusqlite::params![project_id], |r| r.get::<_, i64>(0))
}
