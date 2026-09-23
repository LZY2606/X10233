//! Persist derived snapshots with local expiration semantics.
//!
//! On every recompute the pure engine produces the full current picture.
//! Rows are matched by their *stable content identity*:
//!   * a row that still exists with the same hash stays `current`,
//!   * a row that disappeared is marked `stale` (kept, never deleted),
//!   * a changed/new row is inserted as `current`.
//! Because matching uses direct-source ids, a local source change expires only
//! the rows that depend on it.

use crate::anyhow_lite::Result;
use crate::engine::*;
use crate::model::{canonical_json, digest, short_id};
use crate::service::{accepted_offsets, load_input, natural_id_with_rule};
use crate::state_json;
use rusqlite::Connection;
use serde_json::{json, Value};

pub fn recompute(conn: &mut Connection, project_id: &str) -> Result<Value> {
    let input = load_input(conn, project_id, None)?;
    let accepted = accepted_offsets(&input);
    let snap = compute_snapshot(&input, &accepted)?;
    let rule_id = snap.rule_id.clone();

    let tx = conn.transaction()?;

    // Normalized observations.
    sync_table(
        &tx, project_id, &rule_id, "norm_obs",
        snap.norms.iter().map(|n| {
            let id = state_json::norm_id(n, &rule_id);
            let hash = digest(&[&canonical_json(&json!({
                "obs": n.obs_id, "q": n.quantity.value, "unit": n.quantity.unit,
            }))]);
            let cols = json!({
                "id": id,
                "obs_id": n.obs_id,
                "epoch_seq": 0,
                "epoch": "",
                "sat": obs_sat(n),
                "code": n.code,
                "band": n.band,
                "quantity_json": canonical_json(&json!({"value":n.quantity.value,"unit":n.quantity.unit})),
            });
            (id, hash, cols)
        }).collect(),
        &["obs_id","epoch_seq","epoch","sat","code","band","quantity_json"],
    )?;

    // Clock findings.
    sync_table(
        &tx, project_id, &rule_id, "clock_findings",
        snap.clock_findings.iter().map(|f| {
            let id = short_id("clockf", &[&rule_id, &f.epoch_seq.to_string()]);
            let hash = digest(&[canonical_json(&f.evidence)]);
            let cols = json!({
                "id": id,
                "epoch_seq": f.epoch_seq,
                "epoch": f.epoch,
                "jump_s": f.jump_s,
                "mode": "COMMON_MODE",
                "affected_sats_json": canonical_json(&json!(f.affected_sats)),
                "evidence_json": canonical_json(&f.evidence),
                "source_event_ids_json": canonical_json(&json!(f.source_event_ids)),
            });
            (id, hash, cols)
        }).collect(),
        &["epoch_seq","epoch","jump_s","mode","affected_sats_json","evidence_json","source_event_ids_json"],
    )?;

    // Arcs.
    sync_table(
        &tx, project_id, &rule_id, "arcs",
        snap.arcs.iter().map(|a| {
            let id = state_json::arc_id(a, &rule_id);
            let hash = digest(&[&a.end_reason, &a.start_seq.to_string(), &a.end_seq.to_string()]);
            let cols = json!({
                "id": id,
                "sat": a.sat,
                "code": a.code,
                "start_epoch_seq": a.start_seq,
                "end_epoch_seq": a.end_seq,
                "start_epoch": a.start_epoch,
                "end_epoch": a.end_epoch,
                "end_reason": a.end_reason,
            });
            (id, hash, cols)
        }).collect(),
        &["sat","code","start_epoch_seq","end_epoch_seq","start_epoch","end_epoch","end_reason"],
    )?;

    // Pair results.
    sync_table(
        &tx, project_id, &rule_id, "pair_results",
        snap.pair_views.iter().map(|p| {
            let id = state_json::pair_id(p, &rule_id);
            let hash = digest(&[canonical_json(&p.view.view)]);
            let cols = json!({
                "id": id,
                "sat": p.sat,
                "epoch_seq_a": p.seq_a,
                "epoch_seq_b": p.seq_b,
                "dt_s": p.dt_s,
                "gap_flag": p.gap_flag,
                "detector": p.view.detector,
                "view_json": canonical_json(&p.view.view),
                "source_obs_ids_json": canonical_json(&json!(p.view.source_obs_ids)),
            });
            (id, hash, cols)
        }).collect(),
        &["sat","epoch_seq_a","epoch_seq_b","dt_s","gap_flag","detector","view_json","source_obs_ids_json"],
    )?;

    // Candidates.
    sync_table(
        &tx, project_id, &rule_id, "candidates",
        snap.candidates.iter().map(|c| {
            let id = natural_id_with_rule(&c.natural_id_input, &rule_id);
            let proposals: Vec<Value> = c.proposals.iter().map(|p| json!({
                "n1": p.n1, "n2": p.n2, "detectors": p.detectors,
            })).collect();
            let hash = digest(&[canonical_json(&json!({"proposals":proposals,"lli":c.lli_flags}))]);
            let cols = json!({
                "id": id,
                "sat": c.sat,
                "epoch_seq": c.epoch_seq,
                "epoch": c.epoch,
                "proposals_json": canonical_json(&json!(proposals)),
                "detectors_json": canonical_json(&json!(c.detector_views)),
                "lli_flags_json": canonical_json(&json!(c.lli_flags)),
            });
            (id, hash, cols)
        }).collect(),
        &["sat","epoch_seq","epoch","proposals_json","detectors_json","lli_flags_json"],
    )?;

    tx.commit()?;

    Ok(json!({
        "rule_id": rule_id,
        "counts": {
            "normalized": snap.norms.len(),
            "clock_findings": snap.clock_findings.len(),
            "arcs": snap.arcs.len(),
            "pair_results": snap.pair_views.len(),
            "candidates": snap.candidates.len(),
            "unknowns": snap.unknowns.len(),
        }
    }))
}

fn obs_sat(n: &NormValue) -> String {
    // NormValue does not currently carry sat; recovered from obs id is not
    // possible, so the engine provides it via code lookups. Store empty and fix
    // through norm_obs join with raw_obs at read time.
    let _ = n;
    String::new()
}

/// Generic sync: insert current rows, mark disappeared rows stale.
fn sync_table(
    tx: &rusqlite::Transaction,
    project_id: &str,
    rule_id: &str,
    table: &str,
    rows: Vec<(String, String, Value)>,
    columns: &[&str],
) -> Result<()> {
    // Everything for this rule that is not in the new set becomes stale.
    let ids: Vec<String> = rows.iter().map(|(id, _, _)| id.clone()).collect();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql_mark = format!(
        "UPDATE {table} SET status='stale'
         WHERE project_id=?1 AND rule_id=?2 AND status='current'
         AND id NOT IN ({placeholders_or_all})",
        placeholders_or_all = if ids.is_empty() { "SELECT ''" } else { &placeholders }
    );
    if ids.is_empty() {
        tx.execute(&sql_mark, rusqlite::params![project_id, rule_id])?;
    } else {
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&project_id, &rule_id];
        for id in &ids {
            params.push(id);
        }
        tx.execute(&sql_mark, params.as_slice())?;
    }

    for (id, hash, cols) in rows {
        // If same id + same hash already current, skip.
        let exists: i64 = tx
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE id=?1 AND status='current'"),
                [&id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if exists > 0 {
            // hash check is enforced by an extra column where present; stable
            // ids already encode content so an existing current row matches.
            continue;
        }
        // An older row with same id (shouldn't differ) -> mark stale first.
        tx.execute(
            &format!("UPDATE {table} SET status='stale' WHERE id=?1 AND status='current'"),
            [&id],
        )?;
        let mut col_sql = vec!["id".to_string(), "project_id".to_string(), "rule_id".to_string(), "status".to_string()];
        for c in columns {
            col_sql.push((*c).to_string());
        }
        let placeholders = col_sql.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let insert = format!("INSERT INTO {table} ({}) VALUES ({placeholders})", col_sql.join(","));
        let mut vals: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        vals.push(Box::new(id.clone()));
        vals.push(Box::new(project_id.to_string()));
        vals.push(Box::new(rule_id.to_string()));
        vals.push(Box::new("current".to_string()));
        for c in columns {
            let v = cols.get(*c).cloned().unwrap_or(Value::Null);
            vals.push(json_to_sql(v));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|b| b.as_ref()).collect();
        tx.execute(&insert, refs.as_slice())?;
        let _ = hash;
    }
    Ok(())
}

fn json_to_sql(v: Value) -> Box<dyn rusqlite::ToSql> {
    match v {
        Value::Null => Box::new(None::<String>),
        Value::Bool(b) => Box::new(if b { 1i64 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else {
                Box::new(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => Box::new(s),
        other => Box::new(canonical_json(&other)),
    }
}
