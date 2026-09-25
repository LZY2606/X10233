pub mod schema;
mod ingest;
mod project;
mod decisions;
mod query;
mod projection;

pub use project::{Project, ProjectSummary};
pub use ingest::{IngestReport, IngestDiagnostic};

use crate::events::{DetectorParams, Event, EventKind, VirtualClock, ALGO_VERSION};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    pub conn: Mutex<Connection>,
    pub clock: Mutex<VirtualClock>,
    pub seed: u64,
}

impl Store {
    pub fn open(path: &Path, seed: u64) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(schema::SCHEMA)?;
        Ok(Store {
            conn: Mutex::new(conn),
            clock: Mutex::new(VirtualClock::new(seed)),
            seed,
        })
    }

    /// In-memory store used by tests.
    pub fn memory(seed: u64) -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.execute_batch(schema::SCHEMA)?;
        Ok(Store {
            conn: Mutex::new(conn),
            clock: Mutex::new(VirtualClock::new(seed)),
            seed,
        })
    }

    pub fn tick_iso(&self) -> String {
        self.clock.lock().unwrap().now_iso()
    }

    /// Append an immutable event and record the version pointer.
    pub fn append_event(
        &self,
        conn: &Connection,
        kind: EventKind,
        version_id: &str,
        parent_version_id: &str,
        supersedes: &str,
        payload: serde_json::Value,
        sources: Vec<String>,
        actor: &str,
        note: &str,
    ) -> rusqlite::Result<Event> {
        let seq: i64 = conn.query_row("SELECT COALESCE(MAX(seq)+1,0) FROM events", [], |r| r.get(0))?;
        let id = crate::ids::id_for(
            "evt",
            &format!("{seq}|{}|{version_id}|{parent_version_id}|{supersedes}|{actor}|{note}|{}", kind.as_str(), payload),
        );
        let at_virtual = self.tick_iso();
        conn.execute(
            "INSERT INTO events(seq,id,kind,version_id,parent_version_id,supersedes,at_virtual,payload,sources,actor,note)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                seq, id, kind.as_str(), version_id, parent_version_id, supersedes, at_virtual,
                payload.to_string(), serde_json::to_string(&sources).unwrap(), actor, note
            ],
        )?;
        Ok(Event {
            seq,
            id,
            kind: kind.as_str().to_string(),
            version_id: version_id.to_string(),
            parent_version_id: parent_version_id.to_string(),
            supersedes: supersedes.to_string(),
            at_virtual,
            payload,
            sources,
            actor: actor.to_string(),
            note: note.to_string(),
        })
    }

    pub fn head_version(&self, conn: &Connection) -> rusqlite::Result<String> {
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key='head'", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| "ROOT".to_string()))
    }

    pub fn set_head(&self, conn: &Connection, version: &str) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('head',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![version],
        )?;
        Ok(())
    }

    pub fn list_events(&self, conn: &Connection) -> rusqlite::Result<Vec<Event>> {
        let mut stmt = conn.prepare(
            "SELECT seq,id,kind,version_id,parent_version_id,supersedes,at_virtual,payload,sources,actor,note
             FROM events ORDER BY seq",
        )?;
        let rows = stmt.query_map([], event_from_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn current_params(&self, conn: &Connection) -> rusqlite::Result<(DetectorParams, String)> {
        let mut stmt = conn.prepare(
            "SELECT payload FROM versions WHERE kind='params' ORDER BY created_seq DESC LIMIT 1",
        )?;
        let res = stmt.query_row([], |r| r.get::<_, String>(0));
        match res {
            Ok(json) => {
                let p: DetectorParams = serde_json::from_str(&json).unwrap_or_default();
                let fp = p.fingerprint();
                Ok((p, fp))
            }
            Err(_) => Ok((DetectorParams::default(), DetectorParams::default().fingerprint())),
        }
    }

    pub fn algo_version(&self) -> &'static str {
        ALGO_VERSION
    }
}

pub(crate) fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let payload_s: String = row.get(7)?;
    let sources_s: String = row.get(8)?;
    Ok(Event {
        seq: row.get(0)?,
        id: row.get(1)?,
        kind: row.get(2)?,
        version_id: row.get(3)?,
        parent_version_id: row.get(4)?,
        supersedes: row.get(5)?,
        at_virtual: row.get(6)?,
        payload: serde_json::from_str(&payload_s).unwrap_or(serde_json::Value::Null),
        sources: serde_json::from_str(&sources_s).unwrap_or_default(),
        actor: row.get(9)?,
        note: row.get(10)?,
    })
}
