//! 导出 / 再导入：完整事件日志、原始批次、世代与派生行随包携带。
//!
//! 因为所有身份（project/branch/event/batch/row）都是内容哈希，
//! 删掉本地数据库后再导入，身份、顺序与来源关系逐字节保持。

use crate::storage::Store;
use crate::time::Clock;
use rusqlite::params;
use serde_json::{json, Value};

pub fn export_project(store: &Store, project: &str) -> rusqlite::Result<Value> {
    let proj = store.conn.query_row(
        "SELECT project_uid, name, seed_ms, created_at, created_ms FROM projects WHERE project_uid=?1",
        params![project],
        |r| {
            Ok(json!({
                "project_uid": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "seed_ms": r.get::<_, i64>(2)?,
                "created_at": r.get::<_, String>(3)?,
                "created_ms": r.get::<_, i64>(4)?,
            }))
        },
    )?;

    fn dump(store: &Store, sql: &str, p: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = store.conn.prepare(sql)?;
        let n = stmt.column_count();
        let names: Vec<String> =
            (0..n).map(|i| stmt.column_name(i).unwrap().to_string()).collect();
        let rows = stmt.query_map(p, |r| {
            let mut obj = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                let v: rusqlite::types::Value = r.get(i)?;
                obj.insert(name.clone(), sql_value_to_json(v));
            }
            Ok(Value::Object(obj))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }

    let branches = dump(
        store,
        "SELECT project_uid, branch_uid, name, parent_branch_uid, created_event_uid, created_at
         FROM branches WHERE project_uid=?1 ORDER BY name",
        &[project],
    )?;
    let events = dump(
        store,
        "SELECT project_uid, branch_uid, seq, event_uid, kind, payload, recorded_at, recorded_ms
         FROM events WHERE project_uid=?1 ORDER BY branch_uid, seq",
        &[project],
    )?;
    let batches = dump(
        store,
        "SELECT project_uid, branch_uid, seq, batch_uid, kind, filename, parent_batch_uid,
                content_text, accepted, diagnostics_json, committed_at, committed_ms
         FROM raw_batches WHERE project_uid=?1 ORDER BY branch_uid, seq",
        &[project],
    )?;
    let observations = dump(
        store,
        "SELECT * FROM raw_observations WHERE project_uid=?1",
        &[project],
    )?;
    let sat_states = dump(
        store,
        "SELECT * FROM raw_sat_states WHERE project_uid=?1",
        &[project],
    )?;
    let clock_events = dump(
        store,
        "SELECT * FROM raw_clock_events WHERE project_uid=?1",
        &[project],
    )?;
    let generations = dump(
        store,
        "SELECT project_uid, branch_uid, gen_no, event_uid, note
         FROM generations WHERE project_uid=?1 ORDER BY branch_uid, gen_no",
        &[project],
    )?;
    let derived = dump(
        store,
        "SELECT project_uid, branch_uid, row_uid, kind, sort_key, dep_json, data_json,
                first_gen, last_gen FROM derived_rows WHERE project_uid=?1
         ORDER BY branch_uid, kind, sort_key, first_gen",
        &[project],
    )?;

    Ok(json!({
        "bundle_version": crate::domain::BUNDLE_VERSION,
        "project": proj,
        "branches": branches,
        "events": events,
        "batches": batches,
        "raw_observations": observations,
        "raw_sat_states": sat_states,
        "raw_clock_events": clock_events,
        "generations": generations,
        "derived_rows": derived,
    }))
}

fn sql_value_to_json(v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => Value::Null,
        V::Integer(i) => json!(i),
        V::Real(f) => json!(f),
        V::Text(s) => {
            // 载荷列存的是 JSON，透传解析以保留结构；其它文本保持字符串。
            serde_json::from_str(&s).unwrap_or(Value::String(s))
        }
        V::Blob(b) => Value::String(format!("<blob {} bytes>", b.len())),
    }
}

/// 再导入。`clock` 仅用于必要的时间戳列；历史 created_at/recorded_at 以包为准。
pub fn import_bundle(store: &mut Store, bundle: &Value, _clock: &dyn Clock) -> rusqlite::Result<String> {
    let project = bundle["project"]["project_uid"].as_str().unwrap().to_string();
    // 若已存在同 uid 项目，要求状态完全一致（重复导入幂等），否则报错而不是覆盖。
    let exists: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM projects WHERE project_uid=?1", params![project], |r| r.get(0))?;
    if exists == 0 {
        let p = &bundle["project"];
        store.conn.execute(
            "INSERT INTO projects VALUES (?,?,?,?,?)",
            params![
                p["project_uid"].as_str().unwrap(),
                p["name"].as_str().unwrap(),
                p["seed_ms"].as_i64().unwrap(),
                p["created_at"].as_str().unwrap(),
                p["created_ms"].as_i64().unwrap(),
            ],
        )?;
    }

    load_table(&mut store.conn, "branches", &bundle["branches"],
        &["project_uid","branch_uid","name","parent_branch_uid","created_event_uid","created_at"])?;
    load_table(&mut store.conn, "events", &bundle["events"],
        &["project_uid","branch_uid","seq","event_uid","kind","payload","recorded_at","recorded_ms"])?;
    load_table(&mut store.conn, "raw_batches", &bundle["batches"],
        &["project_uid","branch_uid","seq","batch_uid","kind","filename","parent_batch_uid",
          "content_text","accepted","diagnostics_json","committed_at","committed_ms"])?;
    load_table(&mut store.conn, "raw_observations", &bundle["raw_observations"],
        &["project_uid","batch_uid","ordinal","station","sat","system","freq_num","code",
          "epoch_tag","epoch_offset","pseudorange_m","pseudorange_raw","phase_cycles","phase_raw",
          "doppler_hz","doppler_raw","lli","lli_raw","snr_db_hz","snr_raw","raw_line"])?;
    load_table(&mut store.conn, "raw_sat_states", &bundle["raw_sat_states"],
        &["project_uid","batch_uid","ordinal","sat","epoch_tag","epoch_offset",
          "health_raw","tokens_json","raw_line"])?;
    load_table(&mut store.conn, "raw_clock_events", &bundle["raw_clock_events"],
        &["project_uid","batch_uid","ordinal","applies_to","epoch_tag","epoch_offset",
          "jump_ms","jump_ms_raw","raw_line"])?;
    load_table(&mut store.conn, "generations", &bundle["generations"],
        &["project_uid","branch_uid","gen_no","event_uid","note"])?;
    load_table(&mut store.conn, "derived_rows", &bundle["derived_rows"],
        &["project_uid","branch_uid","row_uid","kind","sort_key","dep_json","data_json",
          "first_gen","last_gen"])?;
    Ok(project)
}

fn load_table(
    conn: &mut rusqlite::Connection,
    table: &str,
    rows: &Value,
    columns: &[&str],
) -> rusqlite::Result<()> {
    let placeholders = columns.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("INSERT OR IGNORE INTO {table} VALUES ({placeholders})");
    if let Value::Array(rows) = rows {
        for row in rows {
            let mut vals: Vec<rusqlite::types::Value> = Vec::new();
            for c in columns {
                vals.push(json_to_sql(&row[c]));
            }
            conn.execute(&sql, rusqlite::params_from_iter(vals))?;
        }
    }
    Ok(())
}

fn json_to_sql(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else {
                rusqlite::types::Value::Real(n.as_f64().unwrap())
            }
        }
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}
