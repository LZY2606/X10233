//! SQLite persistence. The store is a single local file; raw commits and events
//! are append-only. All ids are supplied by the domain layer (content UUIDv5),
//! so export/import preserves identity, order and provenance.

use crate::model::*;
use rusqlite::{params, Connection};
use std::path::Path;

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let mut conn = if path == ":memory:" {
        Connection::open_in_memory()?
    } else {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Connection::open(path)?
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch(r#"
    CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS projects (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL,
        counterexample_key TEXT
    );
    CREATE TABLE IF NOT EXISTS raw_commits (
        id TEXT PRIMARY KEY, seq INTEGER NOT NULL UNIQUE,
        parent_id TEXT, kind TEXT NOT NULL, filename TEXT NOT NULL,
        raw_text TEXT NOT NULL, sha256 TEXT NOT NULL, imported_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS diagnostics (
        id TEXT PRIMARY KEY, commit_id TEXT NOT NULL, line_no INTEGER,
        severity TEXT NOT NULL, code TEXT NOT NULL, message TEXT NOT NULL, raw TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS epochs (
        id TEXT PRIMARY KEY, ord INTEGER NOT NULL, time TEXT NOT NULL,
        time_raw TEXT NOT NULL, flag TEXT NOT NULL,
        clock_offset_raw TEXT, clock_offset_s REAL
    );
    CREATE TABLE IF NOT EXISTS observations (
        id TEXT PRIMARY KEY, epoch_id TEXT NOT NULL, sat TEXT NOT NULL,
        code TEXT NOT NULL, band TEXT NOT NULL, kind TEXT NOT NULL,
        raw TEXT NOT NULL, v REAL, unit TEXT, lli TEXT, ssi TEXT,
        diagnostic TEXT, effective_commit_id TEXT
    );
    CREATE TABLE IF NOT EXISTS nav_records (
        id TEXT PRIMARY KEY, commit_id TEXT NOT NULL, sat TEXT NOT NULL,
        time TEXT NOT NULL, time_raw TEXT NOT NULL, kind TEXT NOT NULL,
        frequency_number INTEGER, values_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS clock_ledger (
        id TEXT PRIMARY KEY, commit_id TEXT NOT NULL, time TEXT NOT NULL,
        source TEXT NOT NULL, magnitude_raw TEXT NOT NULL, magnitude_s REAL,
        unit TEXT NOT NULL, note TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS params_versions (
        id TEXT PRIMARY KEY, seq INTEGER NOT NULL UNIQUE, parent_id TEXT,
        fingerprint TEXT NOT NULL, json TEXT NOT NULL,
        created_at TEXT NOT NULL, note TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS branches (
        id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
        parent_branch_id TEXT, forked_from_event_seq INTEGER,
        created_at TEXT NOT NULL, note TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS events (
        seq INTEGER NOT NULL, branch_id TEXT NOT NULL,
        kind TEXT NOT NULL, payload_json TEXT NOT NULL, at TEXT NOT NULL,
        PRIMARY KEY(seq, branch_id)
    );
    CREATE TABLE IF NOT EXISTS runs (
        id TEXT PRIMARY KEY, basis_commit_id TEXT NOT NULL,
        params_id TEXT NOT NULL, algo_version TEXT NOT NULL,
        fingerprint TEXT NOT NULL, created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS findings (
        id TEXT PRIMARY KEY, run_id TEXT NOT NULL, scope TEXT NOT NULL,
        sat TEXT, band TEXT, epoch_id TEXT NOT NULL, detector TEXT NOT NULL,
        status TEXT NOT NULL, qty REAL, qty_unit TEXT, n1 INTEGER, n2 INTEGER,
        distance REAL, message TEXT NOT NULL, fkey TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS candidates (
        id TEXT PRIMARY KEY, run_id TEXT NOT NULL, sat TEXT NOT NULL,
        epoch_id TEXT NOT NULL, n1 INTEGER NOT NULL, n2 INTEGER NOT NULL,
        score REAL NOT NULL, votes_json TEXT NOT NULL, status TEXT NOT NULL,
        ckey TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS decisions (
        id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, at_event_seq INTEGER NOT NULL,
        target_kind TEXT NOT NULL, target_id TEXT NOT NULL, target_run_id TEXT NOT NULL,
        action TEXT NOT NULL, n1 INTEGER, n2 INTEGER, params_id TEXT,
        note TEXT NOT NULL, created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS repair_offsets (
        id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, sat TEXT NOT NULL,
        band TEXT NOT NULL, from_epoch_id TEXT NOT NULL, cycles INTEGER NOT NULL,
        layer TEXT NOT NULL, decision_id TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS arc_revisions (
        id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, sat TEXT NOT NULL,
        band TEXT NOT NULL, at_event_seq INTEGER NOT NULL,
        arc_before_id TEXT NOT NULL, arc_after_id TEXT NOT NULL,
        decision_id TEXT NOT NULL, reason TEXT NOT NULL
    );
    "#)
}

macro_rules! ins {
    ($c:expr,$sql:expr,$p:expr) => {{ $c.execute($sql, params_from_iter($p))?; }};
}

fn params_from_iter(v: &[&dyn rusqlite::ToSql]) -> Vec<&dyn rusqlite::ToSql> { v.to_vec() }

pub fn put_project(c: &Connection, x: &Project) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO projects VALUES (?1,?2,?3,?4)",
        params![x.id, x.name, x.created_at, x.counterexample_key])?; Ok(())
}

pub fn put_raw_commit(c: &Connection, x: &RawCommit) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO raw_commits VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![x.id, x.seq, x.parent_id, x.kind, x.filename, x.raw_text, x.sha256, x.imported_at])?;
    Ok(())
}

pub fn put_diagnostic(c: &Connection, x: &DiagnosticRec) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO diagnostics VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![x.id, x.commit_id, x.line_no, x.severity, x.code, x.message, x.raw])?;
    Ok(())
}

pub fn put_epoch(c: &Connection, x: &EpochRec) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO epochs VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![x.id, x.ord, x.time, x.time_raw, x.flag, x.clock_offset_raw, x.clock_offset_s])?;
    Ok(())
}

pub fn put_obs(c: &Connection, x: &ObsRec) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO observations VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![x.id, x.epoch_id, x.sat, x.code, x.band, x.kind, x.raw, x.v,
                x.unit, x.lli, x.ssi, x.diagnostic, x.effective_commit_id])?;
    Ok(())
}

pub fn set_effective_commit(c: &Connection, obs_id: &str, commit_id: &str) -> rusqlite::Result<()> {
    c.execute("UPDATE observations SET effective_commit_id=?2 WHERE id=?1",
        params![obs_id, commit_id])?; Ok(())
}

pub fn put_nav(c: &Connection, x: &NavRecRow) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO nav_records VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![x.id, x.commit_id, x.sat, x.time, x.time_raw, x.kind,
                x.frequency_number, x.values_json])?;
    Ok(())
}

pub fn put_clock_ledger(c: &Connection, x: &ClockLedgerRow) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO clock_ledger VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![x.id, x.commit_id, x.time, x.source, x.magnitude_raw,
                x.magnitude_s, x.unit, x.note])?;
    Ok(())
}

pub fn put_params(c: &Connection, x: &Params) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO params_versions VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![x.id, x.seq, x.parent_id, x.fingerprint, x.json, x.created_at, x.note])?;
    Ok(())
}

pub fn put_branch(c: &Connection, x: &Branch) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO branches VALUES (?1,?2,?3,?4,?5,?6)",
        params![x.id, x.name, x.parent_branch_id, x.forked_from_event_seq, x.created_at, x.note])?;
    Ok(())
}

pub fn append_event(c: &Connection, branch_id: &str, seq: i64, kind: &str,
                    payload_json: &str, at: &str) -> rusqlite::Result<()> {
    c.execute("INSERT INTO events VALUES (?1,?2,?3,?4,?5)",
        params![seq, branch_id, kind, payload_json, at])?;
    Ok(())
}

pub fn put_run(c: &Connection, x: &PipelineRun) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO runs VALUES (?1,?2,?3,?4,?5)",
        params![x.id, x.basis_commit_id, x.params_id, x.algo_version, x.fingerprint, x.created_at])?;
    Ok(())
}

pub fn put_finding(c: &Connection, run_id: &str, f: &crate::engine::FFinding) -> rusqlite::Result<()> {
    let id = crate::ids::id("find", &[run_id, &f.key]);
    c.execute("INSERT OR REPLACE INTO findings VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![id, run_id, f.scope, f.sat, f.band, f.epoch_id, f.detector, f.status,
                f.qty, f.qty_unit, f.n1, f.n2, f.distance, f.message, f.key])?;
    Ok(())
}

pub fn put_candidate(c: &Connection, run_id: &str, x: &crate::engine::FCandidate) -> rusqlite::Result<()> {
    let id = crate::ids::id("candrow", &[run_id, &x.key]);
    c.execute("INSERT OR REPLACE INTO candidates VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![id, run_id, x.sat, x.epoch_id, x.n1, x.n2, x.score,
                x.votes_json, "open", x.key])?;
    Ok(())
}

pub fn put_decision(c: &Connection, x: &Decision) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO decisions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![x.id, x.branch_id, x.at_event_seq, x.target_kind, x.target_id,
                x.target_run_id, x.action, x.n1, x.n2, x.params_id, x.note, x.created_at])?;
    Ok(())
}

pub fn put_repair(c: &Connection, x: &RepairOffset) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO repair_offsets VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![x.id, x.branch_id, x.sat, x.band, x.from_epoch_id, x.cycles, x.layer, x.decision_id])?;
    Ok(())
}

pub fn put_arc_revision(c: &Connection, x: &ArcRevision) -> rusqlite::Result<()> {
    c.execute("INSERT OR REPLACE INTO arc_revisions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![x.id, x.branch_id, x.sat, x.band, x.at_event_seq, x.arc_before_id,
                x.arc_after_id, x.decision_id, x.reason])?;
    Ok(())
}

#[allow(dead_code)]
pub fn ensure_placeholder() { let _ = ins!; }
