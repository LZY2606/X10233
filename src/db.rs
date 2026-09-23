use rusqlite::{params, Connection, Row};
use std::sync::Mutex;

/// 逻辑时钟：所有变更共享一个单调递增的 seq，即“虚拟时间”。
/// 任何一行派生数据都记录创建它的 seq；过期用 invalidations 追加表达，从不改写。
pub struct Db {
    pub conn: Mutex<Connection>,
}

#[derive(Clone, Debug)]
pub struct ImportRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub label: String,
    pub format: String,
    pub raw_text: String,
    pub seq: i64,
    pub diag: String,
}

#[derive(Clone, Debug)]
pub struct ObsRow {
    pub id: String,
    pub import_id: String,
    pub station: String,
    pub epoch_idx: i64,
    pub epoch_raw: String,
    pub epoch_s: Option<f64>,
    pub sat: String,
    pub freq_no: i64,
    pub obs_code: String,
    pub kind: String,
    pub value_raw: String,
    pub value: Option<f64>,
    pub unit: String,
    pub lli: i64,
    pub diag: Option<String>,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct ClockEventRow {
    pub id: String,
    pub import_id: String,
    pub station: String,
    pub epoch_idx: i64,
    pub offset_raw: String,
    pub offset_ms: Option<f64>,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct ParamsRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub json: String,
    pub fingerprint: String,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct ArcRow {
    pub id: String,
    pub import_id: String,
    pub station: String,
    pub sat: String,
    pub freq_no: i64,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub algo: String,
    pub param_fp: String,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct CandidateRow {
    pub id: String,
    pub arc_id: Option<String>,
    pub import_id: String,
    pub station: String,
    pub sat: String,
    pub freq_no: i64,
    pub epoch_idx: i64,
    pub detector: String,
    pub kind: String,
    pub est_cycles: Option<f64>,
    pub offset_cycles: Option<i64>,
    pub score: f64,
    pub ambiguity_group: Option<String>,
    pub note: String,
    pub algo: String,
    pub param_fp: String,
    pub sources: String,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct DecisionRow {
    pub id: String,
    pub candidate_id: String,
    pub action: String,
    pub note: String,
    pub actor: String,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct RepairRow {
    pub id: String,
    pub arc_id: String,
    pub station: String,
    pub sat: String,
    pub freq_no: i64,
    pub from_epoch: i64,
    pub offset_cycles: i64,
    pub candidate_id: String,
    pub decision_id: String,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct InvalidationRow {
    pub id: String,
    pub target_kind: String,
    pub target_id: String,
    pub reason: String,
    pub seq: i64,
}

#[derive(Clone, Debug)]
pub struct EventRow {
    pub seq: i64,
    pub kind: String,
    pub payload: String,
    pub actor: String,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS counters(name TEXT PRIMARY KEY, next INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS imports(
  id TEXT PRIMARY KEY, parent_id TEXT, label TEXT, format TEXT,
  raw_text TEXT, seq INTEGER, diag TEXT);
CREATE TABLE IF NOT EXISTS obs_rows(
  id TEXT PRIMARY KEY, import_id TEXT, station TEXT, epoch_idx INTEGER,
  epoch_raw TEXT, epoch_s REAL, sat TEXT, freq_no INTEGER, obs_code TEXT,
  kind TEXT, value_raw TEXT, value REAL, unit TEXT, lli INTEGER,
  diag TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS clock_events(
  id TEXT PRIMARY KEY, import_id TEXT, station TEXT, epoch_idx INTEGER,
  offset_raw TEXT, offset_ms REAL, seq INTEGER);
CREATE TABLE IF NOT EXISTS params(
  id TEXT PRIMARY KEY, parent_id TEXT, json TEXT, fingerprint TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS arcs(
  id TEXT PRIMARY KEY, import_id TEXT, station TEXT, sat TEXT, freq_no INTEGER,
  start_epoch INTEGER, end_epoch INTEGER, algo TEXT, param_fp TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS candidates(
  id TEXT PRIMARY KEY, arc_id TEXT, import_id TEXT, station TEXT, sat TEXT,
  freq_no INTEGER, epoch_idx INTEGER, detector TEXT, kind TEXT,
  est_cycles REAL, offset_cycles INTEGER, score REAL, ambiguity_group TEXT,
  note TEXT, algo TEXT, param_fp TEXT, sources TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS decisions(
  id TEXT PRIMARY KEY, candidate_id TEXT, action TEXT, note TEXT,
  actor TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS repairs(
  id TEXT PRIMARY KEY, arc_id TEXT, station TEXT, sat TEXT, freq_no INTEGER,
  from_epoch INTEGER, offset_cycles INTEGER, candidate_id TEXT,
  decision_id TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS invalidations(
  id TEXT PRIMARY KEY, target_kind TEXT, target_id TEXT, reason TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS events(
  seq INTEGER PRIMARY KEY, kind TEXT, payload TEXT, actor TEXT);
"#;

impl Db {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    pub fn memory() -> rusqlite::Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }
}

pub fn next_id(conn: &Connection, kind: &str) -> String {
    let next: i64 = conn
        .query_row(
            "INSERT INTO counters(name, next) VALUES(?1, 2)
             ON CONFLICT(name) DO UPDATE SET next = next + 1
             RETURNING next - 1",
            params![kind],
            |r| r.get(0),
        )
        .expect("counter");
    format!("{kind}-{next:06}")
}

pub fn next_seq(conn: &Connection) -> i64 {
    let cur: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='seq'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "0".to_string());
    let next: i64 = cur.parse::<i64>().unwrap_or(0) + 1;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('seq', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![next.to_string()],
    )
    .expect("seq");
    next
}

pub fn current_seq(conn: &Connection) -> i64 {
    conn.query_row("SELECT value FROM meta WHERE key='seq'", [], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}

pub fn emit_event(conn: &Connection, seq: i64, kind: &str, payload: &serde_json::Value, actor: &str) {
    conn.execute(
        "INSERT INTO events(seq, kind, payload, actor) VALUES(?1,?2,?3,?4)",
        params![seq, kind, payload.to_string(), actor],
    )
    .expect("event");
}

fn opt<T: rusqlite::types::FromSql>(row: &Row, i: usize) -> rusqlite::Result<Option<T>> {
    row.get(i)
}

pub fn map_import(row: &Row) -> rusqlite::Result<ImportRow> {
    Ok(ImportRow {
        id: row.get(0)?, parent_id: opt(row, 1)?, label: row.get(2)?,
        format: row.get(3)?, raw_text: row.get(4)?, seq: row.get(5)?, diag: row.get(6)?,
    })
}

pub fn map_obs(row: &Row) -> rusqlite::Result<ObsRow> {
    Ok(ObsRow {
        id: row.get(0)?, import_id: row.get(1)?, station: row.get(2)?,
        epoch_idx: row.get(3)?, epoch_raw: row.get(4)?, epoch_s: opt(row, 5)?,
        sat: row.get(6)?, freq_no: row.get(7)?, obs_code: row.get(8)?,
        kind: row.get(9)?, value_raw: row.get(10)?, value: opt(row, 11)?,
        unit: row.get(12)?, lli: row.get(13)?, diag: opt(row, 14)?, seq: row.get(15)?,
    })
}

pub fn map_clock(row: &Row) -> rusqlite::Result<ClockEventRow> {
    Ok(ClockEventRow {
        id: row.get(0)?, import_id: row.get(1)?, station: row.get(2)?,
        epoch_idx: row.get(3)?, offset_raw: row.get(4)?, offset_ms: opt(row, 5)?,
        seq: row.get(6)?,
    })
}

pub fn map_params(row: &Row) -> rusqlite::Result<ParamsRow> {
    Ok(ParamsRow {
        id: row.get(0)?, parent_id: opt(row, 1)?, json: row.get(2)?,
        fingerprint: row.get(3)?, seq: row.get(4)?,
    })
}

pub fn map_arc(row: &Row) -> rusqlite::Result<ArcRow> {
    Ok(ArcRow {
        id: row.get(0)?, import_id: row.get(1)?, station: row.get(2)?,
        sat: row.get(3)?, freq_no: row.get(4)?, start_epoch: row.get(5)?,
        end_epoch: row.get(6)?, algo: row.get(7)?, param_fp: row.get(8)?, seq: row.get(9)?,
    })
}

pub fn map_candidate(row: &Row) -> rusqlite::Result<CandidateRow> {
    Ok(CandidateRow {
        id: row.get(0)?, arc_id: opt(row, 1)?, import_id: row.get(2)?,
        station: row.get(3)?, sat: row.get(4)?, freq_no: row.get(5)?,
        epoch_idx: row.get(6)?, detector: row.get(7)?, kind: row.get(8)?,
        est_cycles: opt(row, 9)?, offset_cycles: opt(row, 10)?, score: row.get(11)?,
        ambiguity_group: opt(row, 12)?, note: row.get(13)?, algo: row.get(14)?,
        param_fp: row.get(15)?, sources: row.get(16)?, seq: row.get(17)?,
    })
}

pub fn map_decision(row: &Row) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        id: row.get(0)?, candidate_id: row.get(1)?, action: row.get(2)?,
        note: row.get(3)?, actor: row.get(4)?, seq: row.get(5)?,
    })
}

pub fn map_repair(row: &Row) -> rusqlite::Result<RepairRow> {
    Ok(RepairRow {
        id: row.get(0)?, arc_id: row.get(1)?, station: row.get(2)?,
        sat: row.get(3)?, freq_no: row.get(4)?, from_epoch: row.get(5)?,
        offset_cycles: row.get(6)?, candidate_id: row.get(7)?,
        decision_id: row.get(8)?, seq: row.get(9)?,
    })
}

pub fn map_invalidation(row: &Row) -> rusqlite::Result<InvalidationRow> {
    Ok(InvalidationRow {
        id: row.get(0)?, target_kind: row.get(1)?, target_id: row.get(2)?,
        reason: row.get(3)?, seq: row.get(4)?,
    })
}

pub fn map_event(row: &Row) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        seq: row.get(0)?, kind: row.get(1)?, payload: row.get(2)?, actor: row.get(3)?,
    })
}

pub const OBS_COLS: &str = "id,import_id,station,epoch_idx,epoch_raw,epoch_s,sat,freq_no,obs_code,kind,value_raw,value,unit,lli,diag,seq";
pub const ARC_COLS: &str = "id,import_id,station,sat,freq_no,start_epoch,end_epoch,algo,param_fp,seq";
pub const CAND_COLS: &str = "id,arc_id,import_id,station,sat,freq_no,epoch_idx,detector,kind,est_cycles,offset_cycles,score,ambiguity_group,note,algo,param_fp,sources,seq";

/// 候选在某 seq 时刻的状态：按决定折叠（undo 回到 open）。
pub fn candidate_status_at(conn: &Connection, candidate_id: &str, as_of: i64) -> String {
    let mut stmt = conn
        .prepare("SELECT action FROM decisions WHERE candidate_id=?1 AND seq<=?2 ORDER BY seq")
        .unwrap();
    let actions: Vec<String> = stmt
        .query_map(params![candidate_id, as_of], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    let mut status = "open".to_string();
    for a in actions {
        status = match a.as_str() {
            "undo" => "open".to_string(),
            other => other.to_string(),
        };
    }
    status
}

/// 目标在某 seq 时刻是否已过期（被作废）。
pub fn is_stale_at(conn: &Connection, target_kind: &str, target_id: &str, as_of: i64) -> Option<String> {
    conn.query_row(
        "SELECT reason FROM invalidations WHERE target_kind=?1 AND target_id=?2 AND seq<=?3 ORDER BY seq LIMIT 1",
        params![target_kind, target_id, as_of],
        |r| r.get(0),
    )
    .ok()
}
