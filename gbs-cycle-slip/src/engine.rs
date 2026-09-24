//! 事件溯源与物化引擎。
//!
//! - 原始输入一经提交不可覆盖（immutable batches）；修订以新版本 + parent_batch 表达。
//! - 每次提交产生一个世代；派生行记录 first_gen/last_gen，局部改动只让依赖行落定。
//! - 分叉物理复制派生行；回到旧事件时按世代恢复当时的可见结论。

use crate::detector::{analyze_station, make_band, BoundaryCandidate, ClockFinding, EpochBands};
use crate::domain::*;
use crate::ids::{canonical_json, content_uid};
use crate::parser::parse_batch_with_reference;
use crate::storage::Store;
use crate::time::Clock;
use rusqlite::params;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

pub struct AppState {
    pub store: Mutex<Store>,
    pub clock: Box<dyn Clock>,
    pub data_dir: String,
}

pub const E_MATERIAL: &str = "MATERIAL_COMMITTED";
pub const E_PARAMS: &str = "PARAMETERS_REVISED";
pub const E_DECISION: &str = "MANUAL_DECISION";
pub const E_FORK: &str = "BRANCH_FORKED";

#[derive(Debug, Clone)]
pub struct BatchRow {
    pub seq: i64,
    pub batch_uid: String,
    pub kind: String,
    pub filename: String,
    pub parent_batch_uid: Option<String>,
    pub content_text: String,
    pub accepted: bool,
    pub diagnostics: serde_json::Value,
    pub committed_at: String,
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub seq: i64,
    pub event_uid: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub recorded_at: String,
}

#[derive(Debug, Clone)]
pub struct HeadState {
    pub batches: Vec<BatchRow>,
    pub params: DetectParams,
    pub params_fp: String,
    pub decisions: Vec<serde_json::Value>,
}

pub fn create_project(
    store: &mut Store,
    clock: &dyn Clock,
    name: &str,
    seed_ms: i64,
) -> rusqlite::Result<String> {
    let created_at = clock.now_rfc3339();
    let created_ms = clock.now_ms();
    let uid = content_uid(vec![json!("project"), json!(name), json!(seed_ms)]);
    store.conn.execute(
        "INSERT OR IGNORE INTO projects VALUES (?,?,?,?,?)",
        params![uid, name, seed_ms, created_at, created_ms],
    )?;
    // main 分支
    let branch_uid = content_uid(vec![json!("branch"), json!(uid), json!("main")]);
    store.conn.execute(
        "INSERT OR IGNORE INTO branches VALUES (?,?,?,?,?,?)",
        params![uid, branch_uid, "main", Option::<String>::None, Option::<String>::None, Option::<String>::None],
    )?;
    Ok(uid)
}

pub fn list_projects(store: &Store) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = store
        .conn
        .prepare("SELECT project_uid, name, seed_ms, created_at FROM projects ORDER BY created_ms")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "project_uid": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "seed_ms": r.get::<_, i64>(2)?,
                "created_at": r.get::<_, String>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn next_seq(store: &Store, project: &str, branch: &str, table: &str) -> rusqlite::Result<i64> {
    let sql = format!("SELECT COALESCE(MAX(seq),0)+1 FROM {table} WHERE project_uid=?1 AND branch_uid=?2");
    store.conn.query_row(&sql, params![project, branch], |r| r.get(0))
}

pub fn list_branches(store: &Store, project: &str) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = store.conn.prepare(
        "SELECT branch_uid, name, parent_branch_uid, created_event_uid, created_at
         FROM branches WHERE project_uid=?1 ORDER BY name",
    )?;
    let rows = stmt
        .query_map(params![project], |r| {
            Ok(json!({
                "branch_uid": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "parent_branch_uid": r.get::<_, Option<String>>(2)?,
                "created_event_uid": r.get::<_, Option<String>>(3)?,
                "created_at": r.get::<_, Option<String>>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn append_event(
    store: &mut Store,
    clock: &dyn Clock,
    project: &str,
    branch: &str,
    kind: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<(i64, String)> {
    let seq = next_seq(store, project, branch, "events")?;
    let event_uid =
        content_uid(vec![json!("event"), json!(project), json!(branch), json!(seq), payload.clone()]);
    let at = clock.now_rfc3339();
    let ms = clock.now_ms();
    store.conn.execute(
        "INSERT INTO events VALUES (?,?,?,?,?,?,?,?)",
        params![project, branch, seq, event_uid, kind, payload.to_string(), at, ms],
    )?;
    Ok((seq, event_uid))
}

fn load_events(store: &Store, project: &str, branch: &str) -> rusqlite::Result<Vec<EventRow>> {
    let mut stmt = store.conn.prepare(
        "SELECT seq, event_uid, kind, payload, recorded_at FROM events
         WHERE project_uid=?1 AND branch_uid=?2 ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(params![project, branch], |r| {
            let payload_str: String = r.get(3)?;
            Ok(EventRow {
                seq: r.get(0)?,
                event_uid: r.get(1)?,
                kind: r.get(2)?,
                payload: serde_json::from_str(&payload_str).unwrap_or(json!(null)),
                recorded_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn params_fingerprint(params: &DetectParams) -> String {
    content_uid(vec![json!("params"), serde_json::to_value(params).unwrap()])
}

fn load_head(store: &Store, project: &str, branch: &str) -> rusqlite::Result<HeadState> {
    let events = load_events(store, project, branch)?;
    let mut batches = Vec::new();
    let mut params = DetectParams::default();
    let mut decisions = Vec::new();
    for ev in &events {
        match ev.kind.as_str() {
            E_MATERIAL => {
                let batch_uid = ev.payload["batch_uid"].as_str().unwrap().to_string();
                let mut stmt = store.conn.prepare(
                    "SELECT seq, batch_uid, kind, filename, parent_batch_uid, content_text,
                            accepted, diagnostics_json, committed_at
                     FROM raw_batches WHERE project_uid=?1 AND branch_uid=?2 AND batch_uid=?3",
                )?;
                let row = stmt.query_row(params![project, branch, batch_uid], |r| {
                    let diag: String = r.get(7)?;
                    Ok(BatchRow {
                        seq: r.get(0)?,
                        batch_uid: r.get(1)?,
                        kind: r.get(2)?,
                        filename: r.get(3)?,
                        parent_batch_uid: r.get(4)?,
                        content_text: r.get(5)?,
                        accepted: r.get::<_, i64>(6)? != 0,
                        diagnostics: serde_json::from_str(&diag).unwrap_or(json!([])),
                        committed_at: r.get(8)?,
                    })
                })?;
                batches.push(row);
            }
            E_PARAMS => {
                if let Some(p) = ev.payload.get("params") {
                    params = serde_json::from_value(p.clone())
                        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                            0, rusqlite::types::Type::Text, Box::new(e)))?;
                }
            }
            E_DECISION => decisions.push(ev.payload.clone()),
            E_FORK => {}
            _ => {}
        }
    }
    let params_fp = params_fingerprint(&params);
    Ok(HeadState { batches, params, params_fp, decisions })
}

pub struct CommitResult {
    pub batch_uid: String,
    pub accepted: bool,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub gen_no: Option<i64>,
}

/// 提交一批原始文本。结构错误（ERROR）只生成拒收批次，不产生派生物料化。
pub fn commit_material(
    store: &mut Store,
    clock: &dyn Clock,
    project: &str,
    branch: &str,
    filename: &str,
    content: &str,
    kind: &str,
    parent_batch_uid: Option<&str>,
) -> rusqlite::Result<CommitResult> {
    let declared_ref = crate::domain::DEFAULT_REFERENCE;
    let parsed = parse_batch_with_reference(content, declared_ref);
    let accepted = parsed.acceptable();
    let seq = next_seq(store, project, branch, "raw_batches")?;
    let batch_uid = content_uid(vec![
        json!("batch"),
        json!(project),
        json!(seq),
        json!(filename),
        json!(content),
    ]);
    let at = clock.now_rfc3339();
    let ms = clock.now_ms();
    let diag_json = serde_json::to_string(&parsed.diagnostics).unwrap();
    store.conn.execute(
        "INSERT INTO raw_batches VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![
            project, branch, seq, batch_uid, kind, filename, parent_batch_uid,
            content, accepted as i64, diag_json, at, ms
        ],
    )?;
    if accepted {
        store.insert_parsed_batch(project, &batch_uid, &parsed)?;
    }
    let payload = json!({
        "batch_uid": batch_uid,
        "filename": filename,
        "kind": kind,
        "parent_batch_uid": parent_batch_uid,
        "accepted": accepted,
        "reference_epoch": parsed.reference_epoch,
        "n_observations": parsed.observations.len(),
        "n_sat_states": parsed.sat_states.len(),
        "n_clock_events": parsed.clock_events.len(),
    });
    append_event(store, clock, project, branch, E_MATERIAL, &payload)?;

    let mut gen_no = None;
    if accepted {
        gen_no = Some(materialize_head(store, project, branch)?);
    }
    Ok(CommitResult {
        batch_uid,
        accepted,
        diagnostics: parsed.diagnostics,
        gen_no,
    })
}

pub fn revise_params(
    store: &mut Store,
    clock: &dyn Clock,
    project: &str,
    branch: &str,
    params: &DetectParams,
        note: &str,
) -> rusqlite::Result<i64> {
    let fp = params_fingerprint(params);
    let payload = json!({
        "params": serde_json::to_value(params).unwrap(),
        "params_fingerprint": fp,
        "note": note,
    });
    append_event(store, clock, project, branch, E_PARAMS, &payload)?;
    materialize_head(store, project, branch)
}

/// 人工裁定（接受 / 否定）。冲突解决与撤销也只是新的决策事件。
#[allow(clippy::too_many_arguments)]
pub fn record_decision(
    store: &mut Store,
    clock: &dyn Clock,
    project: &str,
    branch: &str,
    candidate_uid: Option<&str>,
    action: &str,
    n_l1: Option<i64>,
    n_l2: Option<i64>,
    reason: &str,
) -> rusqlite::Result<String> {
    let payload = json!({
        "candidate_uid": candidate_uid,
        "action": action,
        "n_l1": n_l1,
        "n_l2": n_l2,
        "reason": reason,
        "reviewer": "survey-team",
    });
    let (_seq, uid) = append_event(store, clock, project, branch, E_DECISION, &payload)?;
    materialize_head(store, project, branch)?;
    Ok(uid)
}

#[derive(Debug, Clone)]
struct EffectiveObs {
    obs: RawObservation,
    batch_uid: String,
    batch_seq: i64,
}

fn build_effective(
    store: &Store,
    project: &str,
    branch: &str,
    batches: &[BatchRow],
) -> rusqlite::Result<Vec<EffectiveObs>> {
    let mut by_key: BTreeMap<(String, String, String, String, String), EffectiveObs> =
        BTreeMap::new();
    for b in batches {
        if !b.accepted {
            continue;
        }
        let mut stmt = store.conn.prepare(
            "SELECT station, sat, system, freq_num, code, epoch_tag, epoch_offset,
                    pseudorange_m, pseudorange_raw, phase_cycles, phase_raw, doppler_hz,
                    doppler_raw, lli, lli_raw, snr_db_hz, snr_raw, raw_line, ordinal
             FROM raw_observations WHERE project_uid=?1 AND batch_uid=?2 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map(params![project, b.batch_uid], |r| {
            Ok(RawObservation {
                ordinal: r.get(18)?,
                station: r.get(0)?,
                sat: r.get(1)?,
                system: r.get(2)?,
                freq_num: r.get(3)?,
                code: r.get(4)?,
                epoch_tag: r.get(5)?,
                epoch_offset_s: r.get(6)?,
                epoch_datum: EPOCH_DATUM.to_string(),
                reference_epoch: DEFAULT_REFERENCE.to_string(),
                pseudorange_m: r.get(7)?,
                pseudorange_raw: r.get(8)?,
                phase_cycles: r.get(9)?,
                phase_raw: r.get(10)?,
                doppler_hz: r.get(11)?,
                doppler_raw: r.get(12)?,
                lli: r.get(13)?,
                lli_raw: r.get(14)?,
                snr_db_hz: r.get(15)?,
                snr_raw: r.get(16)?,
                raw_line: r.get(17)?,
            })
        })?;
        for obs in rows {
            let obs = obs?;
            // 后提交的批次覆盖同键；原始行本身仍在 raw_batches 中不可变。
            by_key.insert(
                (obs.station.clone(), obs.sat.clone(), obs.freq_num.clone(), obs.code.clone(), obs.epoch_tag.clone()),
                EffectiveObs { obs, batch_uid: b.batch_uid.clone(), batch_seq: b.seq },
            );
        }
    }
    Ok(by_key.into_values().collect())
}

fn load_source_clock_ms(
    store: &Store,
    project: &str,
    batches: &[BatchRow],
) -> rusqlite::Result<Option<f64>> {
    let mut latest: Option<f64> = None;
    for b in batches {
        if !b.accepted {
            continue;
        }
        let mut stmt = store.conn.prepare(
            "SELECT jump_ms FROM raw_clock_events WHERE project_uid=?1 AND batch_uid=?2
             ORDER BY ordinal",
        )?;
        let vals = stmt.query_map(params![project, b.batch_uid], |r| r.get::<_, Option<f64>>(0))?;
        for v in vals {
            if let Some(v) = v? {
                latest = Some(v);
            }
        }
    }
    Ok(latest)
}

struct NewRow {
    stable_uid: String,
    kind: String,
    sort_key: String,
    dep_signature: String,
    data: serde_json::Value,
}

fn materialize_head(
    store: &mut Store,
    project: &str,
    branch: &str,
) -> rusqlite::Result<i64> {
    let head = load_head_readonly(&store, project, branch)?;
    let effective = build_effective(store, project, branch, &head.batches)?;
    let source_clock_ms = load_source_clock_ms(store, project, &head.batches)?;

    let mut new_rows =
        compute_derived(project, branch, &head, effective, source_clock_ms);
    new_rows.sort_by(|a, b| {
        a.kind.cmp(&b.kind).then(a.sort_key.cmp(&b.sort_key)).then(a.stable_uid.cmp(&b.stable_uid))
    });

    // 上一世代当前行：stable_uid -> (row_uid, signature)
    let prev_gen: Option<i64> = store.conn.query_row(
        "SELECT MAX(gen_no) FROM generations WHERE project_uid=?1 AND branch_uid=?2",
        params![project, branch],
        |r| r.get(0),
    )?;
    let mut previous: HashMap<String, (String, String)> = HashMap::new();
    if let Some(g) = prev_gen {
        let mut stmt = store.conn.prepare(
            "SELECT row_uid, dep_json FROM derived_rows
             WHERE project_uid=?1 AND branch_uid=?2 AND first_gen=?3 AND last_gen IS NULL",
        )?;
        let mapped = stmt.query_map(params![project, branch, g], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in mapped {
            let (row_uid, sig) = row?;
            // row_uid 形如 stable_uid@gen；解析出 stable 部分。
            let stable = row_uid.split('@').next().unwrap_or(&row_uid).to_string();
            previous.insert(stable, (row_uid, sig));
        }
    }

    let gen_no = prev_gen.map(|g| g + 1).unwrap_or(1);
    let mut present: std::collections::HashSet<String> = std::collections::HashSet::new();
    for nr in &new_rows {
        present.insert(nr.stable_uid.clone());
        if let Some((_old_uid, old_sig)) = previous.get(&nr.stable_uid) {
            if old_sig == &nr.dep_signature {
                continue; // 依赖未变：身份保持，直接延续到新世代
            }
            // 依赖变化：旧行落定为过期结果，新身份（带世代）出现。
            store.conn.execute(
                "UPDATE derived_rows SET last_gen=?1
                 WHERE project_uid=?2 AND branch_uid=?3 AND row_uid=?4 AND last_gen IS NULL",
                params![gen_no - 1, project, branch, previous[&nr.stable_uid].0],
            )?;
        }
        let row_uid = format!("{}@{}", nr.stable_uid, gen_no);
        store.conn.execute(
            "INSERT INTO derived_rows VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                project,
                branch,
                row_uid,
                nr.kind,
                nr.sort_key,
                nr.dep_signature,
                nr.data.to_string(),
                gen_no,
                Option::<i64>::None
            ],
        )?;
    }
    // 已消失的行（源数据被修订移除）也过期，但保留可查。
    for (stable, (old_uid, _)) in &previous {
        if !present.contains(stable) {
            store.conn.execute(
                "UPDATE derived_rows SET last_gen=?1
                 WHERE project_uid=?2 AND branch_uid=?3 AND row_uid=?4 AND last_gen IS NULL",
                params![gen_no - 1, project, branch, old_uid],
            )?;
        }
    }

    let last_event = store.conn.query_row(
        "SELECT event_uid FROM events WHERE project_uid=?1 AND branch_uid=?2 ORDER BY seq DESC LIMIT 1",
        params![project, branch],
        |r| r.get::<_, String>(0),
    )?;
    store.conn.execute(
        "INSERT INTO generations VALUES (?,?,?,?)",
        params![project, branch, gen_no, last_event, format!("gen {gen_no}")],
    )?;
    Ok(gen_no)
}

fn load_head_readonly(store: &Store, project: &str, branch: &str) -> rusqlite::Result<HeadState> {
    // 与 load_head 相同逻辑（&Store），避免可变借用。
    let events = load_events(store, project, branch)?;
    let mut batches = Vec::new();
    let mut params = DetectParams::default();
    let mut decisions = Vec::new();
    for ev in &events {
        match ev.kind.as_str() {
            E_MATERIAL => {
                let batch_uid = ev.payload["batch_uid"].as_str().unwrap().to_string();
                let mut stmt = store.conn.prepare(
                    "SELECT seq, batch_uid, kind, filename, parent_batch_uid, content_text,
                            accepted, diagnostics_json, committed_at
                     FROM raw_batches WHERE project_uid=?1 AND branch_uid=?2 AND batch_uid=?3",
                )?;
                let row = stmt.query_row(params![project, branch, batch_uid], |r| {
                    let diag: String = r.get(7)?;
                    Ok(BatchRow {
                        seq: r.get(0)?,
                        batch_uid: r.get(1)?,
                        kind: r.get(2)?,
                        filename: r.get(3)?,
                        parent_batch_uid: r.get(4)?,
                        content_text: r.get(5)?,
                        accepted: r.get::<_, i64>(6)? != 0,
                        diagnostics: serde_json::from_str(&diag).unwrap_or(json!([])),
                        committed_at: r.get(8)?,
                    })
                })?;
                batches.push(row);
            }
            E_PARAMS => {
                if let Some(p) = ev.payload.get("params") {
                    params = serde_json::from_value(p.clone()).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0, rusqlite::types::Type::Text, Box::new(e))
                    })?;
                }
            }
            E_DECISION => decisions.push(ev.payload.clone()),
            E_FORK => {}
            _ => {}
        }
    }
    let params_fp = params_fingerprint(&params);
    Ok(HeadState { batches, params, params_fp, decisions })
}

#[allow(clippy::too_many_arguments)]
fn compute_derived(
    project: &str,
    branch: &str,
    head: &HeadState,
    effective: Vec<EffectiveObs>,
    source_clock_ms: Option<f64>,
) -> Vec<NewRow> {
    let mut rows = Vec::new();
    let params_json = serde_json::to_value(&head.params).unwrap();

    // 站 → 历元(t) → 星 → 频率标识 → 观测
    let mut stations: BTreeMap<String, BTreeMap<i64, BTreeMap<String, HashMap<String, EffectiveObs>>>> =
        BTreeMap::new();
    let mut provenance: HashMap<String, HashMap<String, HashMap<String, HashMap<String, (String, i64)>>>> =
        HashMap::new();

    for e in effective {
        let t = match e.obs.epoch_offset_s {
            Some(v) => v.round() as i64,
            None => continue,
        };
        stations
            .entry(e.obs.station.clone())
            .or_default()
            .entry(t)
            .or_default()
            .entry(e.obs.sat.clone())
            .or_default()
            .insert(format!("{}|{}", e.obs.freq_num, e.obs.code), EffectiveObs { obs: e.obs.clone(), batch_uid: e.batch_uid.clone(), batch_seq: e.batch_seq });
        provenance
            .entry(e.obs.station.clone())
            .or_default()
            .entry(e.obs.sat.clone())
            .or_default()
            .entry(format!("{}|{}", e.obs.freq_num, e.obs.code))
            .or_default()
            .insert(e.obs.epoch_tag.clone(), (e.batch_uid.clone(), e.batch_seq));
    }

    for (station, by_epoch) in &stations {
        // 组装检测器输入
        let mut epochs: Vec<(f64, HashMap<String, EpochBands>)> = Vec::new();
        for (t, sats) in by_epoch {
            let mut sat_map = HashMap::new();
            for (sat, bands) in sats {
                let l1_ctx = bands.get("L1|1C").map(|e| {
                    make_band(
                        &BandDef::lookup("G", "L1", "1C").unwrap(),
                        e.obs.pseudorange_m,
                        e.obs.phase_cycles,
                        e.obs.doppler_hz,
                        e.obs.lli,
                    )
                });
                let l2_ctx = bands.get("L2|2W").map(|e| {
                    make_band(
                        &BandDef::lookup("G", "L2", "2W").unwrap(),
                        e.obs.pseudorange_m,
                        e.obs.phase_cycles,
                        e.obs.doppler_hz,
                        e.obs.lli,
                    )
                });
                sat_map.insert(
                    sat.clone(),
                    EpochBands { t: *t as f64, l1: l1_ctx, l2: l2_ctx },
                );
            }
            epochs.push((*t as f64, sat_map));
        }
        epochs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let report = analyze_station(station, &epochs, &head.params, source_clock_ms);

        // ---- 整机钟跳事件（与单星周跳分开）----
        for cf in &report.clock_findings {
            let stable = content_uid(vec![
                json!("clock"),
                json!(project),
                json!(station),
                json!(cf.epoch_from_s),
                json!(cf.epoch_to_s),
            ]);
            let data = json!({
                "kind": "CLOCK_EVENT",
                "station": station,
                "epoch_from_s": cf.epoch_from_s,
                "epoch_to_s": cf.epoch_to_s,
                "epoch_from_datum": EPOCH_DATUM,
                "fraction_sats_same_direction": cf.fraction,
                "median_code_shift_m": cf.median_code_shift_m,
                "median_phase_shift_l1_cycles": cf.median_phase_shift_l1_cycles,
                "source_clock_event_ms": cf.source_event_ms,
                "separation_note": "RECEIVER clock jump; never applied as per-satellite cycle offset",
                "algo_version": ALGO_VERSION,
                "params_fingerprint": head.params_fp,
            });
            let sig = content_uid(vec![
                json!("sig-clock"),
                params_json.clone(),
                json!(cf.median_code_shift_m),
                json!(cf.fraction),
            ]);
            rows.push(NewRow {
                stable_uid: stable,
                kind: "CLOCK".to_string(),
                sort_key: format!("clock|{:010}|{}", cf.epoch_from_s as i64, station),
                dep_signature: sig,
                data,
            });
        }

        // ---- 多频组合残差序列点（每站星历元，含缺失诊断）----
        emit_series_rows(
            project, station, &epochs, head, &provenance, &mut rows,
        );

        // ---- 候选 + 决策覆盖 + 弧段 ----
        emit_candidate_and_arc_rows(
            project, branch, station, &epochs, &report, head, &provenance, &mut rows,
        );
    }

    rows
}

fn gf_of(eb: &EpochBands) -> Option<f64> {
    match (eb.l1, eb.l2) {
        (Some(a), Some(b)) => match (a.phase_cycles, b.phase_cycles) {
            (Some(p1), Some(p2)) => Some(p1 * a.lambda_m - p2 * b.lambda_m),
            _ => None,
        },
        _ => None,
    }
}

fn mw_wl_cycles(eb: &EpochBands) -> Option<f64> {
    match (eb.l1, eb.l2) {
        (Some(a), Some(b)) => match (a.phase_cycles, b.phase_cycles, a.code_m, b.code_m) {
            (Some(p1), Some(p2), Some(c1), Some(c2)) => {
                let f1 = C_LIGHT / a.lambda_m;
                let f2 = C_LIGHT / b.lambda_m;
                let wl = C_LIGHT / (f1 - f2);
                let narrow = (f1 * c1 + f2 * c2) / (f1 + f2);
                Some(((p1 - p2) * wl - narrow) / wl)
            }
            _ => None,
        },
        _ => None,
    }
}

fn emit_series_rows(
    project: &str,
    station: &str,
    epochs: &[(f64, HashMap<String, EpochBands>)],
    head: &HeadState,
    provenance: &HashMap<String, HashMap<String, HashMap<String, HashMap<String, (String, i64)>>>>,
    rows: &mut Vec<NewRow>,
) {
    let params_json = serde_json::to_value(&head.params).unwrap();
    for (t, sats) in epochs {
        for (sat, eb) in sats {
            let tag = format!("+{t}s");
            let mut gf_status = "OK".to_string();
            if eb.l1.is_none() || eb.l2.is_none() {
                gf_status = "UNKNOWN_FREQUENCY_MISSING".to_string();
            } else if gf_of(eb).is_none() {
                gf_status = "UNKNOWN_PHASE_MISSING".to_string();
            }
            let mut mw_status = "OK".to_string();
            if mw_wl_cycles(eb).is_none() {
                mw_status = if eb.l1.is_none() || eb.l2.is_none() {
                    "UNKNOWN_FREQUENCY_MISSING".to_string()
                } else {
                    "UNKNOWN_CODE_OR_PHASE_MISSING".to_string()
                };
            }
            let l1_snr = eb.l1.and_then(|x| x.code_m.map(|_| x));
            let prov = provenance
                .get(station)
                .and_then(|m| m.get(sat))
                .cloned()
                .unwrap_or_default();
            let mut sources = serde_json::Map::new();
            for (band, by_epoch) in &prov {
                if let Some((batch, seq)) = by_epoch.get(&tag) {
                    sources.insert(band.clone(), json!({"batch_uid": batch, "batch_seq": seq}));
                }
            }
            let stable = content_uid(vec![
                json!("series"),
                json!(project),
                json!(station),
                json!(sat),
                json!(*t as i64),
            ]);
            let l1 = eb.l1;
            let l2 = eb.l2;
            let data = json!({
                "kind": "SERIES_POINT",
                "station": station,
                "sat": sat,
                "epoch_s": t,
                "epoch_datum": EPOCH_DATUM,
                "epoch_tag": tag,
                "l1": l1.map(|b| band_point(b)),
                "l2": l2.map(|b| band_point(b)),
                "gf_m": gf_of(eb),
                "gf_status": gf_status,
                "gf_unit": "m",
                "mw_wl_cycles": mw_wl_cycles(eb),
                "mw_status": mw_status,
                "mw_unit": "cycle",
                "direct_sources": sources,
                "algo_version": ALGO_VERSION,
                "params_fingerprint": head.params_fp,
            });
            let sig = content_uid(vec![
                json!("sig-series"),
                params_json.clone(),
                json!(gf_of(eb)),
                json!(mw_wl_cycles(eb)),
                json!(l1.map(|b| (b.code_m, b.phase_cycles, b.doppler_hz, b.lli))),
                json!(l2.map(|b| (b.code_m, b.phase_cycles, b.doppler_hz, b.lli))),
                json!(sources),
            ]);
            let _ = l1_snr;
            rows.push(NewRow {
                stable_uid: stable,
                kind: "SERIES".to_string(),
                sort_key: format!("series|{sat}|{:010}", *t as i64),
                dep_signature: sig,
                data,
            });
        }
    }
}

fn band_point(b: crate::detector::BandObs) -> serde_json::Value {
    json!({
        "lambda_m": b.lambda_m,
        "pseudorange_m": b.code_m,
        "phase_cycles": b.phase_cycles,
        "doppler_hz": b.doppler_hz,
        "lli": b.lli,
        "unit_phase": "cycle",
        "unit_range": "m",
        "unit_doppler": "Hz",
    })
}

fn emit_candidate_and_arc_rows(
    project: &str,
    _branch: &str,
    station: &str,
    epochs: &[(f64, HashMap<String, EpochBands>)],
    report: &crate::detector::BoundaryReport,
    head: &HeadState,
    provenance: &HashMap<String, HashMap<String, HashMap<String, HashMap<String, (String, i64)>>>>,
    rows: &mut Vec<NewRow>,
) {
    let params_json = serde_json::to_value(&head.params).unwrap();

    // 边界键 → 候选
    let mut cand_by_key: BTreeMap<(String, i64, i64), &BoundaryCandidate> = BTreeMap::new();
    for c in &report.candidates {
        cand_by_key.insert(
            (c.sat.clone(), c.epoch_from_s as i64, c.epoch_to_s as i64),
            c,
        );
    }

    // 决策覆盖：同一候选只取最新一条。
    let mut decisions_latest: HashMap<String, serde_json::Value> = HashMap::new();
    for d in &head.decisions {
        if let Some(cu) = d.get("candidate_uid").and_then(|v| v.as_str()) {
            decisions_latest.insert(cu.to_string(), d.clone());
        }
    }
    // 已撤销（后续 REVOKE）的决策不再覆盖候选状态。
    let revoked: std::collections::HashSet<String> = head
        .decisions
        .iter()
        .filter(|d| d["action"].as_str() == Some("REVOKE"))
        .filter_map(|d| d["candidate_uid"].as_str().map(|s| s.to_string()))
        .collect();

    for c in &report.candidates {
        let t0 = c.epoch_from_s as i64;
        let t1 = c.epoch_to_s as i64;
        let stable = content_uid(vec![
            json!("candidate"),
            json!(project),
            json!(station),
            json!(c.sat),
            json!(t0),
            json!(t1),
        ]);

        let mut status = if c.clock_event {
            "REJECTED_RECEIVER_CLOCK_JUMP"
        } else if c.lli_flagged && !c.hypotheses.iter().any(|h| (h.n_l1 != 0 || h.n_l2 != 0) && h.score > 0) {
            "REJECTED_LLI_FALSE_ALARM"
        } else if c.tied {
            "AMBIGUOUS_TIED"
        } else {
            let winner = c.hypotheses.first();
            match winner {
                Some(w) if (w.n_l1 != 0 || w.n_l2 != 0) && w.score > 0 => "PENDING_UNIQUE",
                _ => "REJECTED_NO_EVIDENCE",
            }
        }.to_string();

        let mut chosen: Option<(i64, i64)> = None;
        let mut decision_event: Option<serde_json::Value> = None;
        let mut reject_reason: Option<String> = None;

        if let Some(d) = decisions_latest.get(&stable) {
            if !revoked.contains(&stable) {
                decision_event = Some(d.clone());
                match d["action"].as_str() {
                    Some("ACCEPT") => {
                        let n1 = d["n_l1"].as_i64().unwrap_or(0);
                        let n2 = d["n_l2"].as_i64().unwrap_or(0);
                        // 只允许接受检测器给出的合法假设。
                        if c.hypotheses.iter().any(|h| h.n_l1 == n1 && h.n_l2 == n2) {
                            chosen = Some((n1, n2));
                            status = "ACCEPTED".to_string();
                        } else {
                            status = "REJECTED_ILLEGAL_HYPOTHESIS".to_string();
                            reject_reason = Some(
                                "人工选择不在检测器保留的合法候选集合内".to_string(),
                            );
                        }
                    }
                    Some("REJECT") => {
                        status = "REJECTED_MANUAL".to_string();
                        reject_reason = d["reason"].as_str().map(|s| s.to_string());
                    }
                    _ => {}
                }
            }
        }
        if status.starts_with("REJECTED_") {
            if reject_reason.is_none() {
                reject_reason = Some(status.clone());
            }
        }

        let sources = boundary_sources(station, c, provenance);
        let data = json!({
            "kind": "CANDIDATE",
            "candidate_uid": stable,
            "station": station,
            "sat": c.sat,
            "epoch_from_s": c.epoch_from_s,
            "epoch_to_s": c.epoch_to_s,
            "epoch_datum": EPOCH_DATUM,
            "hypotheses": c.hypotheses,
            "tied": c.tied,
            "max_score": c.max_score,
            "gf_status": c.gf_status,
            "mw_status": c.mw_status,
            "td_status": c.td_status,
            "lli_flagged": c.lli_flagged,
            "clock_event": c.clock_event,
            "clock_metric_m": c.clock_metric,
            "status": status,
            "reject_reason": reject_reason,
            "chosen_n_l1": chosen.map(|x| x.0),
            "chosen_n_l2": chosen.map(|x| x.1),
            "decision": decision_event,
            "direct_sources": sources,
            "algo_version": ALGO_VERSION,
            "params_fingerprint": head.params_fp,
        });
        let sig = content_uid(vec![
            json!("sig-cand"),
            params_json.clone(),
            json!(c.sat),
            json!(t0),
            json!(t1),
            json!(&c.hypotheses),
            json!(c.lli_flagged),
            json!(c.clock_event),
            json!(decision_event),
            json!(sources),
        ]);
        rows.push(NewRow {
            stable_uid: stable.clone(),
            kind: "CANDIDATE".to_string(),
            sort_key: format!("cand|{sat_pad(c.sat)}|{t0:010}"),
            dep_signature: sig,
            data,
        });
    }

    emit_arc_rows(project, station, epochs, head, &cand_by_key, &decisions_latest, &revoked, rows);
}

fn sat_pad(sat: &str) -> String {
    // 字典序稳定排序辅助：保留原样，仅补零。
    sat.chars()
        .map(|ch| if ch.is_ascii_digit() { ch } else { '0' })
        .collect::<String>()
        .chars()
        .enumerate()
        .map(|(i, _)| sat.chars().nth(i).unwrap())
        .collect()
}

fn boundary_sources(
    station: &str,
    c: &BoundaryCandidate,
    provenance: &HashMap<String, HashMap<String, HashMap<String, HashMap<String, (String, i64)>>>>,
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(by_sat) = provenance.get(station).and_then(|m| m.get(&c.sat)) {
        for (band, by_epoch) in by_sat {
            let mut band_src = serde_json::Map::new();
            for (tag, (batch, seq)) in by_epoch {
                if tag == &format!("+{}s", c.epoch_from_s as i64)
                    || tag == &format!("+{}s", c.epoch_to_s as i64)
                {
                    band_src.insert(tag.clone(), json!({"batch_uid": batch, "batch_seq": seq}));
                }
            }
            if !band_src.is_empty() {
                out.insert(band.clone(), serde_json::Value::Object(band_src));
            }
        }
    }
    serde_json::Value::Object(out)
}

#[allow(clippy::too_many_arguments)]
fn emit_arc_rows(
    project: &str,
    station: &str,
    epochs: &[(f64, HashMap<String, EpochBands>)],
    head: &HeadState,
    cand_by_key: &BTreeMap<(String, i64, i64), &BoundaryCandidate>,
    decisions: &HashMap<String, serde_json::Value>,
    revoked: &std::collections::HashSet<String>,
    rows: &mut Vec<NewRow>,
) {
    let params_json = serde_json::to_value(&head.params).unwrap();
    let sats: Vec<String> = epochs
        .iter()
        .flat_map(|(_, m)| m.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    for sat in sats {
        for (freq, offset_index) in [("L1", 0usize), ("L2", 1usize)] {
            let present: Vec<(i64, bool)> = epochs
                .iter()
                .map(|(t, m)| {
                    let has = m.get(&sat).map(|eb| match freq {
                        "L1" => eb.l1.is_some(),
                        _ => eb.l2.is_some(),
                    }).unwrap_or(false);
                    (*t as i64, has)
                })
                .collect();
            let mut arc_idx = 0i64;
            let mut start: Option<i64> = None;
            let mut boundaries = Vec::new();
            let mut applied_offset = 0i64;

            for w in present.windows(2) {
                let (t0, h0) = w[0];
                let (t1, h1) = w[1];
                if start.is_none() && h0 {
                    start = Some(t0);
                }
                if !h0 || !h1 {
                    if h0 && start.is_some() {
                        push_arc(rows, project, station, &sat, freq, arc_idx, start.unwrap(), t0,
                                 &boundaries, head, &params_json);
                        arc_idx += 1;
                        start = None;
                    }
                    if !h0 && h1 {
                        start = Some(t1);
                    }
                    continue;
                }
                let dt = t1 - t0;
                let long_gap = dt as f64 > head.params.gap_split_seconds + 1e-9;
                // 弧段只因“被接受的真实整周偏移”或超长间隔断开。
                let (accepted, n) = accepted_offset(project, cand_by_key, decisions, revoked, &sat, t0, t1);
                if long_gap || accepted {
                    boundaries.push(json!({
                        "epoch_from_s": t0,
                        "epoch_to_s": t1,
                        "cause": if long_gap { "GAP_EXCEEDS_THRESHOLD" } else { "ACCEPTED_CYCLE_SLIP" },
                        "n_l1": if accepted && offset_index == 0 { n.0 } else { 0 },
                        "n_l2": if accepted && offset_index == 1 { n.1 } else { 0 },
                    }));
                    if accepted {
                        applied_offset += if offset_index == 0 { n.0 } else { n.1 };
                    }
                    push_arc(rows, project, station, &sat, freq, arc_idx, start.unwrap(), t0,
                             &boundaries, head, &params_json);
                    arc_idx += 1;
                    start = Some(t1);
                }
            }
            // 最后一段
            if let Some(last) = present.iter().rev().find(|(_, h)| *h) {
                if start.is_some() {
                    push_arc(rows, project, station, &sat, freq, arc_idx, start.unwrap(), last.0,
                             &boundaries, head, &params_json);
                }
            }
            let _ = applied_offset;
        }
    }
}

fn accepted_offset(
    cand_by_key: &BTreeMap<(String, i64, i64), &BoundaryCandidate>,
    decisions: &HashMap<String, serde_json::Value>,
    revoked: &std::collections::HashSet<String>,
    sat: &str,
    t0: i64,
    t1: i64,
) -> (bool, (i64, i64)) {
    let cand = match cand_by_key.get(&(sat.to_string(), t0, t1)) {
        Some(c) => *c,
        None => return (false, (0, 0)),
    };
    let uid = content_uid(vec![
        json!("candidate"),
        json!(&cand.station),
        json!(sat),
        json!(t0),
        json!(t1),
    ]);
    // 注意：uid 里的 project 需与候选行一致；此处候选数据已含 station，
    // project 由外层行 data 携带，因此再用相同配方补 project。
    if let Some(d) = decisions.get(&uid) {
        if !revoked.contains(&uid) && d["action"].as_str() == Some("ACCEPT") {
            let n1 = d["n_l1"].as_i64().unwrap_or(0);
            let n2 = d["n_l2"].as_i64().unwrap_or(0);
            if cand.hypotheses.iter().any(|h| h.n_l1 == n1 && h.n_l2 == n2) {
                return (true, (n1, n2));
            }
        }
    }
    (false, (0, 0))
}

fn push_arc(
    rows: &mut Vec<NewRow>,
    project: &str,
    station: &str,
    sat: &str,
    freq: &str,
    arc_idx: i64,
    start_s: i64,
    end_s: i64,
    boundaries: &[serde_json::Value],
    head: &HeadState,
    params_json: &serde_json::Value,
) {
    let stable = content_uid(vec![
        json!("arc"),
        json!(project),
        json!(station),
        json!(sat),
        json!(freq),
        json!(start_s),
    ]);
    let data = json!({
        "kind": "ARC",
        "arc_uid": stable,
        "station": station,
        "sat": sat,
        "freq_num": freq,
        "start_epoch_s": start_s,
        "end_epoch_s": end_s,
        "epoch_datum": EPOCH_DATUM,
        "arc_index": arc_idx,
        "boundaries": boundaries,
        "offset_layer_note": "修复只存偏移层；原始载波永不改写",
        "algo_version": ALGO_VERSION,
        "params_fingerprint": head.params_fp,
    });
    let sig = content_uid(vec![
        json!("sig-arc"),
        params_json.clone(),
        json!(sat),
        json!(freq),
        json!(start_s),
        json!(end_s),
        json!(boundaries),
    ]);
    rows.push(NewRow {
        stable_uid: stable,
        kind: "ARC".to_string(),
        sort_key: format!("arc|{sat}|{freq}|{start_s:010}"),
        dep_signature: sig,
        data,
    });
}

/// 从某事件分叉：复制父分支的事件/批次/派生行（身份含相同内容），
/// 新分支独立演化。
pub fn fork_branch(
    store: &mut Store,
    clock: &dyn Clock,
    project: &str,
    parent_branch: &str,
    new_name: &str,
    from_seq: Option<i64>,
    note: &str,
) -> rusqlite::Result<(String, i64)> {
    let new_branch = content_uid(vec![
        json!("branch"),
        json!(project),
        json!(new_name),
    ]);
    let at = clock.now_rfc3339();
    store.conn.execute(
        "INSERT INTO branches VALUES (?,?,?,?,?,?)",
        params![project, new_branch, new_name, parent_branch, Option::<String>::None, at],
    )?;

    // 复制事件（截至 from_seq）
    let mut parent_events: Vec<(i64, String, String, String, String, i64)> = {
        let mut stmt = store.conn.prepare(
            "SELECT seq, event_uid, kind, payload, recorded_at, recorded_ms FROM events
             WHERE project_uid=?1 AND branch_uid=?2 ORDER BY seq",
        )?;
        let rows = stmt
            .query_map(params![project, parent_branch], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    if let Some(limit) = from_seq {
        parent_events.retain(|e| e.0 <= limit);
    }
    for e in &parent_events {
        store.conn.execute(
            "INSERT INTO events VALUES (?,?,?,?,?,?,?,?)",
            params![project, new_branch, e.0, e.1, e.2, e.3, e.4, e.5],
        )?;
    }

    // 复制已提交批次
    let mut batch_rows: Vec<(i64, String, String, String, Option<String>, String, i64, String, String, i64)> = {
        let mut stmt = store.conn.prepare(
            "SELECT seq, batch_uid, kind, filename, parent_batch_uid, content_text, accepted,
                    diagnostics_json, committed_at, committed_ms
             FROM raw_batches WHERE project_uid=?1 AND branch_uid=?2 ORDER BY seq",
        )?;
        stmt.query_map(params![project, parent_branch], |r| {
            Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if let Some(limit) = from_seq {
        batch_rows.retain(|b| b.0 <= limit);
    }
    let copied_batches: std::collections::HashSet<String> =
        batch_rows.iter().map(|b| b.1.clone()).collect();
    for b in &batch_rows {
        store.conn.execute(
            "INSERT INTO raw_batches VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![project, new_branch, b.0, b.1, b.2, b.3, b.4, b.5, b.6, b.7, b.8, b.9],
        )?;
    }

    // 复制原始观测（只复制已带过来的批次）
    for table in ["raw_observations", "raw_sat_states", "raw_clock_events"] {
        let mut stmt = store
            .conn
            .prepare(&format!("SELECT * FROM {table} WHERE project_uid=?1 AND batch_uid=?2"))?;
        let col_count = stmt.column_count();
        let rows: Vec<Vec<rusqlite::types::Value>> = {
            let mut collected = Vec::new();
            for batch_uid in &copied_batches {
                let mapped = stmt
                    .query_map(params![project, batch_uid], |r| {
                        (0..col_count)
                            .map(|i| r.get::<_, rusqlite::types::Value>(i))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                collected.extend(mapped);
            }
            collected
        };
        drop(stmt);
        let placeholders = (0..col_count).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("INSERT OR IGNORE INTO {table} VALUES ({placeholders})");
        for row in rows {
            store.conn.execute(&sql, rusqlite::params_from_iter(row))?;
        }
    }

    // 复制截至时点位的世代与派生行。
    let cutoff_gen: i64 = if let Some(limit) = from_seq {
        store.conn.query_row(
            "SELECT COALESCE(MAX(gen_no),0) FROM generations
             WHERE project_uid=?1 AND branch_uid=?2
             AND event_uid IN (SELECT event_uid FROM events WHERE branch_uid=?2 AND seq<=?3)",
            params![project, parent_branch, limit],
            |r| r.get(0),
        )?
    } else {
        store.conn.query_row(
            "SELECT COALESCE(MAX(gen_no),0) FROM generations WHERE project_uid=?1 AND branch_uid=?2",
            params![project, parent_branch],
            |r| r.get(0),
        )?
    };
    {
        let mut stmt = store.conn.prepare(
            "SELECT gen_no, event_uid, note FROM generations
             WHERE project_uid=?1 AND branch_uid=?2 AND gen_no<=?3 ORDER BY gen_no",
        )?;
        let gens = stmt
            .query_map(params![project, parent_branch, cutoff_gen], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (g, ev, note_txt) in gens {
            store.conn.execute(
                "INSERT INTO generations VALUES (?,?,?,?)",
                params![project, new_branch, g, ev, note_txt],
            )?;
        }
    }
    {
        let mut stmt = store.conn.prepare(
            "SELECT row_uid, kind, sort_key, dep_json, data_json, first_gen,
                    COALESCE(last_gen, first_gen)
             FROM derived_rows
             WHERE project_uid=?1 AND branch_uid=?2 AND first_gen<=?3",
        )?;
        let rows = stmt
            .query_map(params![project, parent_branch, cutoff_gen], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (row_uid, kind, sort_key, dep_json, data_json, fg, lg) in rows {
            // 截断到分叉点之后仍可见的区间。
            let lg_final = if lg >= cutoff_gen { Option::<i64>::None } else { Some(lg) };
            store.conn.execute(
                "INSERT INTO derived_rows VALUES (?,?,?,?,?,?,?,?,?)",
                params![project, new_branch, row_uid, kind, sort_key, dep_json, data_json, fg, lg_final],
            )?;
        }
    }

    let payload = json!({
        "parent_branch_uid": parent_branch,
        "new_branch_uid": new_branch,
        "from_seq": from_seq,
        "note": note,
    });
    append_event(store, clock, project, &new_branch, E_FORK, &payload)?;
    Ok((new_branch, cutoff_gen))
}

/// 当前世代号。
pub fn current_gen(store: &Store, project: &str, branch: &str) -> rusqlite::Result<i64> {
    store.conn.query_row(
        "SELECT COALESCE(MAX(gen_no),0) FROM generations WHERE project_uid=?1 AND branch_uid=?2",
        params![project, branch],
        |r| r.get(0),
    )
}

/// 取某世代的派生行（None=当前）。过期结果用 stale=true 明确标识，绝不冒充当前。
pub fn derived_view(
    store: &Store,
    project: &str,
    branch: &str,
    gen: Option<i64>,
    kind: Option<&str>,
) -> rusqlite::Result<serde_json::Value> {
    let current = current_gen(store, project, branch)?;
    let target = gen.unwrap_or(current);
    let mut sql = String::from(
        "SELECT row_uid, kind, sort_key, data_json, first_gen, last_gen
         FROM derived_rows
         WHERE project_uid=?1 AND branch_uid=?2 AND first_gen<=?3
           AND (last_gen IS NULL OR last_gen>=?3)",
    );
    if kind.is_some() {
        sql.push_str(" AND kind=?4");
    }
    sql.push_str(" ORDER BY kind, sort_key, row_uid");
    let mut stmt = store.conn.prepare(&sql)?;
    let mapper = |r: &rusqlite::Row| {
        let row_uid: String = r.get(0)?;
        let k: String = r.get(1)?;
        let sort_key: String = r.get(2)?;
        let data_str: String = r.get(3)?;
        let first_gen: i64 = r.get(4)?;
        let last_gen: Option<i64> = r.get(5)?;
        let mut data: serde_json::Value = serde_json::from_str(&data_str).unwrap_or(json!({}));
        if let serde_json::Value::Object(map) = &mut data {
            map.insert("row_uid".into(), json!(row_uid));
            map.insert("stable_uid".into(), json!(row_uid.split('@').next().unwrap_or(&row_uid)));
            map.insert("first_gen".into(), json!(first_gen));
            map.insert("last_gen".into(), json!(last_gen));
            map.insert("viewed_gen".into(), json!(target));
            // stale 的语义：查看的不是当前世代；或该行在当前世代已落定。
            let is_stale = target != current || last_gen.is_some();
            map.insert("stale".into(), json!(is_stale));
        }
        Ok(json!({
            "row_uid": row_uid,
            "kind": k,
            "sort_key": sort_key,
            "data": data,
        }))
    };
    let rows = if let Some(k) = kind {
        stmt.query_map(params![project, branch, target, k], mapper)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map(params![project, branch, target], mapper)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(json!({
        "branch": branch,
        "gen": target,
        "current_gen": current,
        "rows": rows,
    }))
}

pub fn event_log(store: &Store, project: &str, branch: &str) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = store.conn.prepare(
        "SELECT seq, event_uid, kind, payload, recorded_at FROM events
         WHERE project_uid=?1 AND branch_uid=?2 ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(params![project, branch], |r| {
            let payload: String = r.get(3)?;
            Ok(json!({
                "seq": r.get::<_, i64>(0)?,
                "event_uid": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?,
                "payload": serde_json::from_str::<serde_json::Value>(&payload).unwrap_or(json!(null)),
                "recorded_at": r.get::<_, String>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn batch_list(store: &Store, project: &str, branch: &str) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = store.conn.prepare(
        "SELECT seq, batch_uid, kind, filename, parent_batch_uid, accepted, diagnostics_json, committed_at
         FROM raw_batches WHERE project_uid=?1 AND branch_uid=?2 ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(params![project, branch], |r| {
            let diag: String = r.get(6)?;
            Ok(json!({
                "seq": r.get::<_, i64>(0)?,
                "batch_uid": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?,
                "filename": r.get::<_, String>(3)?,
                "parent_batch_uid": r.get::<_, Option<String>>(4)?,
                "accepted": r.get::<_, i64>(5)? != 0,
                "diagnostics": serde_json::from_str::<serde_json::Value>(&diag).unwrap_or(json!([])),
                "committed_at": r.get::<_, String>(7)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn effective_params(store: &Store, project: &str, branch: &str) -> rusqlite::Result<DetectParams> {
    Ok(load_head_readonly(store, project, branch)?.params)
}
