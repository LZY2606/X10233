//! SQLite persistence. Everything needed to replay a project is stored
//! append-only; the database is a single local file in a temp/demo dir.

use crate::domain::*;
use crate::state::Snapshot;
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Store {
    pub conn: Mutex<Connection>,
}

pub fn init_schema(conn: &Connection) {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_ms INTEGER NOT NULL,
            head_event_id TEXT
        );
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            parent_event_id TEXT,
            branch TEXT NOT NULL,
            kind TEXT NOT NULL,
            virtual_ms INTEGER NOT NULL,
            actor TEXT NOT NULL,
            summary TEXT NOT NULL,
            payload TEXT NOT NULL,
            ord INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS events_project ON events(project_id, ord);
        CREATE TABLE IF NOT EXISTS stale_rows (
            project_id TEXT NOT NULL,
            row_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            superseded_by_event_id TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            ord INTEGER NOT NULL,
            PRIMARY KEY (project_id, row_id, ord)
        );
        "#,
    )
    .expect("schema migration");
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        init_schema(&conn);
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn);
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn create_project(&self, id: &str, name: &str, created_ms: i64, head_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO projects(id,name,created_ms,head_event_id) VALUES(?1,?2,?3,?4)",
            params![id, name, created_ms, head_id],
        )?;
        Ok(())
    }

    pub fn set_head(&self, project_id: &str, head_id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE projects SET head_event_id=?1 WHERE id=?2",
            params![head_id, project_id],
        );
    }

    pub fn project_exists(&self, id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT 1 FROM projects WHERE id=?1", params![id], |_| Ok(())).is_ok()
    }

    pub fn append_event(&self, project_id: &str, ev: &Event) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let payload = serde_json::to_string(&ev.payload).unwrap();
        conn.execute(
            "INSERT INTO events(id,project_id,seq,parent_event_id,branch,kind,virtual_ms,actor,summary,payload,ord)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                ev.id,
                project_id,
                ev.seq,
                ev.parent_event_id,
                ev.branch,
                ev.kind.as_str(),
                ev.virtual_ms,
                ev.actor,
                ev.summary,
                payload,
                ev.seq
            ],
        )?;
        Ok(())
    }

    pub fn list_events(&self, project_id: &str) -> rusqlite::Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,seq,parent_event_id,branch,kind,virtual_ms,actor,summary,payload FROM events
             WHERE project_id=?1 ORDER BY ord",
        )?;
        let rows = stmt.query_map(params![project_id], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn event_exists(&self, project_id: &str, id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM events WHERE project_id=?1 AND id=?2",
            params![project_id, id],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn list_projects(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id,name FROM projects ORDER BY created_ms")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete_project(&self, project_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM events WHERE project_id=?1", params![project_id])?;
        conn.execute("DELETE FROM stale_rows WHERE project_id=?1", params![project_id])?;
        conn.execute("DELETE FROM projects WHERE id=?1", params![project_id])?;
        Ok(())
    }

    /// Persist derived rows that are being superseded so they stay viewable
    /// after a local change (stale != current).
    pub fn archive_stale(&self, project_id: &str, superseded_by: &str, rows: Vec<serde_json::Value>) {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        for (i, row) in rows.into_iter().enumerate() {
            let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
            let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let _ = tx.execute(
                "INSERT OR REPLACE INTO stale_rows(project_id,row_id,kind,superseded_by_event_id,snapshot_json,ord)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![project_id, id, kind, superseded_by, serde_json::to_string(&row).unwrap(), i as i64],
            );
        }
        tx.commit().unwrap();
    }

    pub fn list_stale(&self, project_id: &str) -> Vec<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT snapshot_json FROM stale_rows WHERE project_id=?1 ORDER BY ord")
            .unwrap();
        let rows = stmt.query_map(params![project_id], |r| r.get::<_, String>(0)).unwrap();
        rows.flatten().filter_map(|s| serde_json::from_str(&s).ok()).collect()
    }

    pub fn export_all(&self) -> serde_json::Value {
        let projects = self.list_projects().unwrap_or_default();
        let mut projs = Vec::new();
        for (pid, name) in projects {
            let events = self.list_events(&pid).unwrap_or_default();
            let stale = self.list_stale(&pid);
            projs.push(serde_json::json!({
                "id": pid,
                "name": name,
                "events": events,
                "stale_rows": stale,
            }));
        }
        serde_json::json!({ "format": "carrier-break-station/export-1", "projects": projs })
    }

    pub fn import_all(&self, doc: &serde_json::Value, replace: bool) -> Result<usize, String> {
        let projects = doc.get("projects").and_then(|v| v.as_array()).ok_or("missing projects[]")?;
        let mut n = 0;
        for p in projects {
            let pid = p.get("id").and_then(|v| v.as_str()).ok_or("project missing id")?;
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("imported");
            if replace {
                let _ = self.delete_project(pid);
            } else if self.project_exists(pid) {
                return Err(format!("project {pid} already exists; delete or re-export first"));
            }
            let events_val = p.get("events").and_then(|v| v.as_array()).ok_or("missing events[]")?;
            if events_val.is_empty() {
                return Err(format!("project {pid} has no events"));
            }
            let mut events: Vec<Event> = Vec::new();
            for ev in events_val {
                events.push(serde_json::from_value(ev.clone()).map_err(|e| e.to_string())?);
            }
            let created = events.iter().map(|e| e.virtual_ms).min().unwrap_or(0);
            let head = events.first().map(|e| e.id.as_str()).unwrap_or("");
            self.create_project(pid, name, created, head).map_err(|e| e.to_string())?;
            for ev in &events {
                self.append_event(pid, ev).map_err(|e| e.to_string())?;
            }
            if let Some(h) = crate::state::head_event(&events) {
                self.set_head(pid, &h.id);
            }
            if let Some(stale) = p.get("stale_rows").and_then(|v| v.as_array()) {
                let vals: Vec<serde_json::Value> = stale.iter().cloned().collect();
                if !vals.is_empty() {
                    self.archive_stale(pid, "import", vals);
                }
            }
            n += 1;
        }
        Ok(n)
    }
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let kind_str: String = r.get(5)?;
    let payload_str: String = r.get(8)?;
    Ok(Event {
        id: r.get(0)?,
        seq: r.get(1)?,
        parent_event_id: r.get(2)?,
        branch: r.get(3)?,
        kind: EventKind::parse(&kind_str).ok_or_else(|| rusqlite::Error::InvalidColumnType(0, kind_str.clone(), rusqlite::types::Type::Text))?,
        virtual_ms: r.get(6)?,
        actor: r.get(7)?,
        summary: r.get(4)?,
        payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::json!({})),
    })
}

/// convenience: materialize current head snapshot
pub fn current_snapshot(store: &Store, project_id: &str) -> Result<Snapshot, String> {
    let events = store.list_events(project_id).map_err(|e| e.to_string())?;
    if events.is_empty() {
        return Err("project has no events".into());
    }
    Ok(crate::state::Engine::new(events).materialize(project_id, None))
}
