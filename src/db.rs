use crate::error::{AppError, AppResult};
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_SQL: &str = include_str!("../sql/schema.sql");

pub fn open(path: &Path) -> AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

pub fn open_in_memory() -> AppResult<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

/// Strict integer primary key lookup helper.
pub fn require_single(conn: &Connection, sql: &str, params: &[&str]) -> AppResult<i64> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        r.get::<_, i64>(0)
    })?;
    match rows.next() {
        Some(v) => Ok(v?),
        None => Err(AppError::Integrity("expected single row".into())),
    }
}
