//! SQLite persistence. The event log is the source of truth; derived rows are
//! cached with their producing event and a stable id so local invalidation and
//! export/import round-trips preserve identity, order and provenance.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::event::Event;
use crate::model::Id;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_seq INTEGER NOT NULL,
            created_ts_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS events (
            seq INTEGER PRIMARY KEY,
            id TEXT NOT NULL UNIQUE,
            project_id TEXT NOT NULL,
            ts_ms INTEGER NOT NULL,
            kind TEXT NOT NULL,
            parent_event_id TEXT,
            payload_json TEXT NOT NULL,
            -- rows are physically appended; logical undo never deletes rows
            superseded_by TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, seq);

        -- cached derived snapshot rows (history-aware)
        CREATE TABLE IF NOT EXISTS derived_cache (
            id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            at_seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            json TEXT NOT NULL,
            PRIMARY KEY (project_id, kind, id, at_seq)
        );
        CREATE INDEX IF NOT EXISTS idx_cache_head ON derived_cache(project_id, at_seq);

        -- current head pointer per project
        CREATE TABLE IF NOT EXISTS heads (
            project_id TEXT PRIMARY KEY,
            head_seq INTEGER NOT NULL
        );
        "#,
    )?;
    Ok(())
}

pub fn create_project(conn: &Connection, id: &Id, name: &str, ts_ms: i64) -> rusqlite::Result<i64> {
    let seq = 0i64;
    conn.execute(
        "INSERT INTO projects (id, name, created_seq, created_ts_ms) VALUES (?1,?2,?3,?4)",
        params![id.0, name, seq, ts_ms],
    )?;
    conn.execute(
        "INSERT INTO heads (project_id, head_seq) VALUES (?1, ?2)",
        params![id.0, seq],
    )?;
    Ok(seq)
}

pub fn next_seq(conn: &Connection, project_id: &Id) -> rusqlite::Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE project_id = ?1",
        params![project_id.0],
        |r| r.get(0),
    )?;
    Ok(n)
}

pub fn append_event(conn: &Connection, event: &Event) -> rusqlite::Result<()> {
    let kind = format!("{:?}", event.kind);
    let payload = serde_json::to_string(&event.payload).expect("payload");
    conn.execute(
        "INSERT INTO events (seq, id, project_id, ts_ms, kind, parent_event_id, payload_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            event.seq,
            event.id.0,
            event.project_id.0,
            event.ts_ms,
            kind,
            event.parent_event_id.as_ref().map(|i| i.0.clone()),
            payload
        ],
    )?;
    conn.execute(
        "UPDATE heads SET head_seq = ?2 WHERE project_id = ?1",
        params![event.project_id.0, event.seq],
    )?;
    Ok(())
}

pub fn load_events(conn: &Connection, project_id: &Id) -> rusqlite::Result<Vec<Event>> {
    load_events_until(conn, project_id, None)
}

pub fn load_events_until(
    conn: &Connection,
    project_id: &Id,
    up_to_seq: Option<i64>,
) -> rusqlite::Result<Vec<Event>> {
    let mut sql = String::from(
        "SELECT seq,id,ts_ms,kind,parent_event_id,payload_json FROM events
         WHERE project_id = ?1 ",
    );
    if up_to_seq.is_some() {
        sql.push_str("AND seq <= ?2 ");
    }
    sql.push_str("ORDER BY seq ASC");
    let mut stmt = conn.prepare(&sql)?;
    let pid = project_id.clone();
    let rows: rusqlite::Result<Vec<Event>> = if let Some(up) = up_to_seq {
        let q = params![pid.0.clone(), up];
        stmt.query_map(q, move |r| row_to_event(r, &pid))?
            .collect()
    } else {
        let q = params![pid.0.clone()];
        stmt.query_map(q, move |r| row_to_event(r, &pid))?
            .collect()
    };
    rows
}

fn row_to_event(row: &rusqlite::Row<'_>, project_id: &Id) -> rusqlite::Result<Event> {
    let seq: i64 = row.get(0)?;
    let id: String = row.get(1)?;
    let ts_ms: i64 = row.get(2)?;
    let kind: String = row.get(3)?;
    let parent: Option<String> = row.get(4)?;
    let payload_json: String = row.get(5)?;
    let payload: crate::event::EventPayload =
        serde_json::from_str(&payload_json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))?;
    let kind = match kind.as_str() {
        "ProjectCreated" => crate::event::EventKind::ProjectCreated,
        "RawBatch" => crate::event::EventKind::RawBatch,
        "ParamRevision" => crate::event::EventKind::ParamRevision,
        "Decision" => crate::event::EventKind::Decision,
        "Conflict" => crate::event::EventKind::Conflict,
        "ConflictResolved" => crate::event::EventKind::ConflictResolved,
        "Undo" => crate::event::EventKind::Undo,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown event kind {other}"),
                )),
            ))
        }
    };
    Ok(Event {
        seq,
        id: Id(id),
        ts_ms,
        kind,
        project_id: project_id.clone(),
        parent_event_id: parent.map(Id),
        payload,
    })
}

pub fn head_seq(conn: &Connection, project_id: &Id) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT head_seq FROM heads WHERE project_id = ?1",
        params![project_id.0],
        |r| r.get(0),
    )
}

pub fn cache_derived(conn: &Connection, project_id: &Id, at_seq: i64, kind: &str, id: &str, json: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO derived_cache (id, project_id, at_seq, kind, json)
         VALUES (?1,?2,?3,?4,?5)",
        params![id, project_id.0, at_seq, kind, json],
    )?;
    Ok(())
}

pub fn list_projects(conn: &Connection) -> rusqlite::Result<Vec<(Id, String)>> {
    let mut stmt = conn.prepare("SELECT id, name FROM projects ORDER BY created_ts_ms")?;
    let rows = stmt.query_map([], |r| Ok((Id(r.get::<_, String>(0)?), r.get::<_, String>(1)?)))?;
    rows.collect()
}

/// Export the whole project (events in order) to a deterministic JSON bundle.
pub fn export_project(conn: &Connection, project_id: &Id) -> rusqlite::Result<String> {
    let events = load_events(conn, project_id)?;
    let bundle = serde_json::json!({
        "format": "gsb-cycle-slip-bundle/1",
        "project_id": project_id.0,
        "events": events.iter().map(ev_json).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&bundle).map_err(serde_to_sql)
}

fn ev_json(ev: &Event) -> serde_json::Value {
    serde_json::json!({
        "seq": ev.seq,
        "id": ev.id.0,
        "ts_ms": ev.ts_ms,
        "kind": format!("{:?}", ev.kind),
        "parent_event_id": ev.parent_event_id.as_ref().map(|i| i.0.clone()),
        "payload": ev.payload,
    })
}

/// Import an exported bundle into a (possibly fresh) database, rebuilding the
/// event log exactly. Ids/order are taken from the bundle, so a delete+import
/// round trip is identity-stable.
pub fn import_bundle(conn: &Connection, bundle_json: &str) -> rusqlite::Result<Id> {
    let v: serde_json::Value =
        serde_json::from_str(bundle_json).map_err(serde_to_sql)?;
    if v.get("format").and_then(|x| x.as_str()) != Some("gsb-cycle-slip-bundle/1") {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "not a gsb-cycle-slip-bundle/1 file",
            )),
        ));
    }
    let project_id = Id(v["project_id"].as_str().unwrap().to_string());
    let mut head_seq_val = 0i64;
    for item in v["events"].as_array().unwrap() {
        let seq = item["seq"].as_i64().unwrap();
        let id = Id(item["id"].as_str().unwrap().to_string());
        let ts_ms = item["ts_ms"].as_i64().unwrap();
        let kind_str = item["kind"].as_str().unwrap();
        let parent = item["parent_event_id"].as_str().map(|s| Id(s.to_string()));
        let payload: crate::event::EventPayload =
            serde_json::from_value(item["payload"].clone()).map_err(serde_to_sql)?;
        if matches!(payload, crate::event::EventPayload::ProjectCreated { .. }) {
            let name = match &payload {
                crate::event::EventPayload::ProjectCreated { name } => name.clone(),
                _ => unreachable!(),
            };
            conn.execute(
                "INSERT OR IGNORE INTO projects (id,name,created_seq,created_ts_ms)
                 VALUES (?1,?2,?3,?4)",
                params![project_id.0, name, seq, ts_ms],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO heads (project_id,head_seq) VALUES (?1,?2)",
                params![project_id.0, seq],
            )?;
        }
        let kind = kind_str;
        conn.execute(
            "INSERT OR REPLACE INTO events (seq,id,project_id,ts_ms,kind,parent_event_id,payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                seq, id.0, project_id.0, ts_ms, kind,
                parent.map(|p| p.0),
                serde_json::to_string(&payload).unwrap()
            ],
        )?;
        head_seq_val = head_seq_val.max(seq);
    }
    conn.execute(
        "UPDATE heads SET head_seq = ?2 WHERE project_id = ?1",
        params![project_id.0, head_seq_val],
    )?;
    Ok(project_id)
}

fn serde_to_sql(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    )
}
