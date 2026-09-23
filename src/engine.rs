//! 事件溯源核心：原始层 append-only，派生层可重放、可过期、可按历史事件查看。

pub mod detect;

use crate::db::Db;
use crate::detect::*;
use crate::identity::{q9, stable_id};
use crate::model::*;
use crate::parser::parse_gob;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// 真空光速 (m/s)，算法常量进入算法版本而非参数指纹。
pub const C: f64 = 299_792_458.0;

/// 虚拟时间基准（固定 seed，不访问系统时钟）；第 n 个事件 = 基准 + n 秒。
pub const VIRTUAL_EPOCH: &str = "2026-01-01T00:00:00Z";

pub struct EventAppender;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub event_id: String,
    pub seq: i64,
    pub virtual_ts: String,
    pub raw_obs_ids: Vec<String>,
    pub raw_other_ids: Vec<String>,
    pub diagnostic_ids: Vec<String>,
    pub replaced_ids: Vec<String>,
    pub fatal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamPayload {
    pub params: Parameters,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionPayload {
    pub action: String, // accept | reject
    pub candidate_id: String,
    pub boundary_id: String,
    pub station: String,
    pub sat: String,
    pub epoch: f64,
    pub vector: BTreeMap<String, i64>,
    pub note: String,
}

pub fn virtual_ts(seq: i64) -> String {
    format!("2026-01-01T00:{:02}:{:02}Z", seq / 60, seq % 60)
}

fn next_seq(conn: &rusqlite::Connection, branch: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE branch = ?1",
        params![branch],
    ).unwrap_or(1)
}

fn last_event(conn: &rusqlite::Connection, branch: &str) -> Option<String> {
    conn.query_row(
        "SELECT event_id FROM events WHERE branch = ?1 ORDER BY seq DESC LIMIT 1",
        params![branch],
    ).ok()
}

fn insert_event(
    conn: &rusqlite::Connection, branch: &str, kind: &str, summary: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<(String, i64, String)> {
    let seq = next_seq(conn, branch);
    let parent = last_event(conn, branch);
    let ts = virtual_ts(seq);
    let material = json!({
        "branch": branch, "seq": seq, "parent": parent,
        "kind": kind, "payload": payload,
    });
    let event_id = stable_id("evt_", &material);
    conn.execute(
        "INSERT INTO events(event_id, seq, branch, parent_event_id, kind, virtual_ts, summary, payload)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![event_id, seq, branch, parent, kind, ts, summary, payload.to_string()],
    )?;
    Ok((event_id, seq, ts))
}

/// 一份原始观测的稳定身份材料：物理身份字段（不含时间戳/事件号）。
#[derive(Serialize)]
struct RawIdentity<'a> {
    station: &'a str, sat: &'a str, epoch: f64,
    band: &'a str, code: &'a str,
    pseudorange_raw: &'a Option<String>,
    carrier_raw: &'a Option<String>,
    doppler_raw: &'a Option<String>,
    lli_raw: &'a Option<String>,
    snr_raw: &'a Option<String>,
}

impl EventAppender {
    /// 导入（或打补丁）。结构错误直接返回 fatal 且不写任何数据；
    /// 值无法转换只产生 diagnostics，原始 token 照存。
    pub fn ingest(db: &Db, gob_text: &str, is_patch: bool) -> IngestResult {
        let parsed = parse_gob(gob_text, true);
        if let Some(fatal) = parsed.fatal {
            return IngestResult {
                event_id: String::new(), seq: 0, virtual_ts: String::new(),
                raw_obs_ids: vec![], raw_other_ids: vec![], diagnostic_ids: vec![],
                replaced_ids: vec![], fatal: Some(fatal),
            };
        }
        let bundle = parsed.bundle;
        let mut conn = db.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();

        let payload = json!({
            "case_id": bundle.case_id,
            "title": bundle.title,
            "seed": bundle.seed,
            "is_patch": is_patch,
            "replaces": bundle.replaces,
            "format": bundle.format,
            "format_version": bundle.format_version,
            "observation_count": bundle.observations.len(),
        });
        let kind = if is_patch { "ingest_patch" } else { "ingest" };
        let summary = if is_patch {
            format!("局部源数据补丁（{} 条观测）", bundle.observations.len())
        } else {
            format!("导入案例 {}（{} 条观测）", bundle.case_id, bundle.observations.len())
        };
        let (event_id, seq, ts) = insert_event(&tx, "main", kind, &summary, &payload).unwrap();

        let mut raw_obs_ids = Vec::new();
        let mut replaced_ids = Vec::new();
        for obs in &bundle.observations {
            let material = RawIdentity {
                station: &obs.station, sat: &obs.sat, epoch: obs.epoch,
                band: &obs.band, code: &obs.code,
                pseudorange_raw: &obs.pseudorange_raw,
                carrier_raw: &obs.carrier_raw,
                doppler_raw: &obs.doppler_raw,
                lli_raw: &obs.lli_raw,
                snr_raw: &obs.snr_raw,
            };
            let raw_id = stable_id("raw_", &material);
            if is_patch && bundle.replaces {
                let old: Option<String> = tx.query_row(
                    "SELECT raw_id FROM raw_obs
                     WHERE station=?1 AND sat=?2 AND epoch=?3 AND band=?4
                       AND superseded_event IS NULL",
                    params![obs.station, obs.sat, obs.epoch, obs.band],
                ).ok();
                if let Some(old_id) = old {
                    tx.execute(
                        "UPDATE raw_obs SET superseded_event=?1 WHERE raw_id=?2",
                        params![event_id, old_id],
                    ).unwrap();
                    replaced_ids.push(old_id);
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO raw_obs
                 (raw_id, introduced_event, station, sat, epoch, band, code, json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![raw_id, event_id, obs.station, obs.sat, obs.epoch, obs.band,
                        obs.code, serde_json::to_string(obs).unwrap()],
            ).unwrap();
            raw_obs_ids.push(raw_id);
        }

        let mut raw_other_ids = Vec::new();
        for f in &bundle.freqs {
            let id = stable_id("rawf_", f);
            tx.execute(
                "INSERT OR IGNORE INTO raw_other(raw_id, introduced_event, kind, key, json)
                 VALUES (?1,?2,'freqdef',?3,?4)",
                params![id, event_id, format!("{}-{}-{}", f.constellation, f.band, f.code),
                        serde_json::to_string(f).unwrap()],
            ).unwrap();
            raw_other_ids.push(id);
        }
        for s in &bundle.stations {
            let id = stable_id("raws_", s);
            tx.execute(
                "INSERT OR IGNORE INTO raw_other(raw_id, introduced_event, kind, key, json)
                 VALUES (?1,?2,'station',?3,?4)",
                params![id, event_id, s.station, serde_json::to_string(s).unwrap()],
            ).unwrap();
            raw_other_ids.push(id);
        }
        for ce in &bundle.clock_events {
            let id = stable_id("rawc_", ce);
            tx.execute(
                "INSERT OR IGNORE INTO raw_other(raw_id, introduced_event, kind, key, json)
                 VALUES (?1,?2,'clock',?3,?4)",
                params![id, event_id,
                        format!("{}-{}", ce.station, ce.epoch_raw),
                        serde_json::to_string(ce).unwrap()],
            ).unwrap();
            raw_other_ids.push(id);
        }
        for ss in &bundle.sat_states {
            let id = stable_id("rawsat_", ss);
            tx.execute(
                "INSERT OR IGNORE INTO raw_other(raw_id, introduced_event, kind, key, json)
                 VALUES (?1,?2,'satstate',?3,?4)",
                params![id, event_id,
                        format!("{}-{}", ss.sat, ss.epoch_raw),
                        serde_json::to_string(ss).unwrap()],
            ).unwrap();
            raw_other_ids.push(id);
        }

        let mut diagnostic_ids = Vec::new();
        for d in &bundle.diagnostics {
            let id = stable_id("diag_", json!({"event": event_id, "d": d}));
            tx.execute(
                "INSERT OR IGNORE INTO diagnostics(diag_id, introduced_event, kind, line_no, message)
                 VALUES (?1,?2,?3,?4,?5)",
                params![id, event_id, d.code, d.line_no, d.message],
            ).unwrap();
            diagnostic_ids.push(id);
        }

        tx.commit().unwrap();
        Replay::recompute(db, "main");
        IngestResult {
            event_id, seq, virtual_ts: ts,
            raw_obs_ids, raw_other_ids, diagnostic_ids, replaced_ids, fatal: None,
        }
    }

    pub fn current_params(db: &Db, branch: &str) -> Parameters {
        let conn = db.conn.lock().unwrap();
        let chain = effective_chain(&conn, branch);
        let mut p = Parameters::default();
        for ev in chain {
            if ev.kind == "params" {
                if let Some(v) = ev.payload.get("params") {
                    p = serde_json::from_value(v.clone()).unwrap_or_default();
                }
            }
        }
        p
    }

    pub fn revise_params(db: &Db, branch: &str, params: Parameters, reason: &str) -> String {
        let payload = json!(ParamPayload { params, reason: reason.to_string() });
        let mut conn = db.conn.lock().unwrap();
        let (id, _seq, _ts) = insert_event(&conn, branch, "params",
            &format!("参数修订：{reason}"), &payload).unwrap();
        drop(conn);
        Replay::recompute(db, branch);
        id
    }

    pub fn decide(db: &Db, branch: &str, dp: DecisionPayload) -> Result<String, String> {
        if dp.action != "accept" && dp.action != "reject" {
            return Err("decision.action 必须是 accept 或 reject".into());
        }
        if dp.action == "accept" {
            // accept 只允许落在仍保留的合法候选上
            let conn = db.conn.lock().unwrap();
            let exists: bool = conn.query_row(
                "SELECT 1 FROM derived WHERE kind='candidate' AND branch=?1
                 AND json_extract(json,'$.candidate_id')=?2 AND state != 'stale'",
                params![branch, dp.candidate_id], |_| Ok(())).is_ok();
            drop(conn);
            if !exists {
                return Err(format!("候选 {} 不在当前合法候选集合中，拒绝自动接受", dp.candidate_id));
            }
        }
        let summary = match dp.action.as_str() {
            "accept" => format!("人工接受 {} 在 {}s 的偏移 {:?}", dp.sat, dp.epoch, dp.vector),
            _ => format!("人工否定候选 {}", dp.candidate_id),
        };
        let payload = serde_json::to_value(&dp).unwrap();
        let mut conn = db.conn.lock().unwrap();
        let (id, _seq, _ts) = insert_event(&conn, branch, "decision", &summary, &payload)?;
        drop(conn);
        Replay::recompute(db, branch);
        Ok(id)
    }

    /// 从历史事件处分叉：新分支祖先链指向该事件，派生层在新分支独立重放。
    pub fn fork(db: &Db, new_branch: &str, from_event: &str) -> Result<(), String> {
        let mut conn = db.conn.lock().unwrap();
        let already: bool = conn.query_row("SELECT 1 FROM branches WHERE name=?1",
            params![new_branch], |_| Ok(())).is_ok();
        if already {
            return Err(format!("分支 {new_branch} 已存在"));
        }
        let parent_branch: String = conn.query_row(
            "SELECT branch FROM events WHERE event_id=?1", params![from_event],
            |r| r.get(0)).map_err(|_| format!("事件 {from_event} 不存在"))?;
        let seq = next_seq(&conn, &parent_branch);
        conn.execute(
            "INSERT INTO branches(name, forked_from_event, created_seq) VALUES (?1,?2,?3)",
            params![new_branch, from_event, seq]).unwrap();
        Ok(())
    }

    pub fn branches(db: &Db) -> Vec<BranchRecord> {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, forked_from_event, created_seq FROM branches ORDER BY created_seq, name")
            .unwrap();
        stmt.query_map([], |r| Ok(BranchRecord {
            name: r.get(0)?, forked_from_event: r.get(1)?, created_seq: r.get(2)?,
        })).unwrap().filter_map(|x| x.ok()).collect()
    }
}
}

/// 重放器：从事件链重建任意分支的当前/历史视图。
pub struct Replay;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedRow {
    pub derived_id: String,
    pub kind: String,
    pub state: String,
    pub station: Option<String>,
    pub sat: Option<String>,
    pub epoch: Option<f64>,
    pub band: Option<String>,
    pub score: Option<f64>,
    pub created_event: String,
    pub superseded_event: Option<String>,
    pub sources: Vec<String>,
    pub algo_version: String,
    pub param_fp: String,
    pub body: serde_json::Value,
}

fn param_fp(p: &Parameters) -> String {
    stable_id("param_", p)
}

/// 某分支在 tip 处生效的事件链（祖先 -> 新）。
pub fn effective_chain(conn: &rusqlite::Connection, branch: &str) -> Vec<EventRecord> {
    let mut rows: Vec<EventRecord> = {
        let mut stmt = conn.prepare(
            "SELECT event_id, seq, branch, parent_event_id, kind, virtual_ts, summary, payload
             FROM events WHERE branch=?1 ORDER BY seq").unwrap();
        stmt.query_map(params![branch], |r| Ok(EventRecord {
            event_id: r.get(0)?, seq: r.get(1)?, branch: r.get(2)?,
            parent_event_id: r.get(3)?, kind: r.get(4)?,
            virtual_ts: r.get(5)?, summary: r.get(6)?,
            payload: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(json!({})),
        })).unwrap().filter_map(|x| x.ok()).collect()
    };
    // fork 分支的祖先在其他分支上：沿 parent 链回溯收集。
    let tip = rows.last().cloned();
    let mut chain_ids: BTreeSet<String> = rows.iter().map(|r| r.event_id.clone()).collect();
    let mut cursor = tip.and_then(|t| t.parent_event_id);
    while let Some(pid) = cursor {
        if !chain_ids.insert(pid.clone()) { break; }
        if let Ok(rec) = conn.query_row(
            "SELECT event_id, seq, branch, parent_event_id, kind, virtual_ts, summary, payload
             FROM events WHERE event_id=?1", params![pid],
            |r| Ok(EventRecord {
                event_id: r.get(0)?, seq: r.get(1)?, branch: r.get(2)?,
                parent_event_id: r.get(3)?, kind: r.get(4)?,
                virtual_ts: r.get(5)?, summary: r.get(6)?,
                payload: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(json!({})),
            }),
        ) {
            cursor = rec.parent_event_id.clone();
            rows.push(rec);
        } else { break; }
    }
    rows.sort_by_key(|r| (r.virtual_ts.clone(), r.event_id.clone()));
    rows
}

#[derive(Default)]
struct Workspace {
    params: Parameters,
    /// (station,sat,band) -> 历元升序样本
    series: BTreeMap<(String, String, String), Vec<EpochSample>>,
    /// (constellation,band) -> 波长（m）
    waves: BTreeMap<(String, String), f64>,
    /// (station,epoch) -> 钟跳 ns（取最后一次导入生效）
    clocks: BTreeMap<(String, f64), f64>,
    /// (station,sat,epoch) -> band -> LLI
    llis: BTreeMap<(String, String, f64), BTreeMap<String, i64>>,
    /// 已生效的 accepted 决策：boundary key -> (event_id, vector)
    accepted: BTreeMap<String, (String, BTreeMap<String, i64>)>,
    /// 已生效的 rejected 候选：candidate_id -> event_id
    rejected: BTreeMap<String, String>,
}

impl Replay {
    /// 按事件链重建分支派生层。幂等：先逻辑删除该分支所有派生行再重建。
    /// 历史可查看性通过保留 superseded 的行实现（见 derived_history）。
    pub fn recompute(db: &Db, branch: &str) {
        let mut conn = db.conn.lock().unwrap();
        let chain = effective_chain(&conn, branch);

        // 归档当前行为新的历史代（每次重放都能回到“当时看到的结论”）。
        conn.execute(
            "UPDATE derived SET state='stale' WHERE branch=?1 AND state != 'stale'",
            params![branch],
        ).unwrap();

        let mut ws = Workspace::default();
        for ev in &chain {
            match ev.kind.as_str() {
                "params" => {
                    if let Some(p) = ev.payload.get("params") {
                        ws.params = serde_json::from_value(p.clone()).unwrap_or_default();
                    }
                }
                "decision" => {
                    if let Ok(dp) = serde_json::from_value::<DecisionPayload>(ev.payload.clone()) {
                        match dp.action.as_str() {
                            "accept" => { ws.accepted.insert(dp.boundary_id.clone(), (ev.event_id.clone(), dp.vector.clone())); }
                            "reject" => { ws.rejected.insert(dp.candidate_id.clone(), ev.event_id.clone()); }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            if ev.kind == "ingest" || ev.kind == "ingest_patch" {
                Self::load_ingest_into(&conn, ev, &mut ws);
            }
        }

        let pfp = param_fp(&ws.params);
        Self::build_derived(&conn, branch, &ws, &pfp);
    }

    fn load_ingest_into(conn: &rusqlite::Connection, ev: &EventRecord, ws: &mut Workspace) {
        // 频率
        {
            let mut stmt = conn.prepare(
                "SELECT json FROM raw_other WHERE introduced_event=?1 AND kind='freqdef'").unwrap();
            let rows: Vec<String> = stmt.query_map(params![ev.event_id], |r| r.get(0))
                .unwrap().filter_map(|x| x.ok()).collect();
            for j in rows {
                if let Ok(f) = serde_json::from_str::<FreqDef>(&j) {
                    if let Some(hz) = f.freq_hz {
                        ws.waves.insert((f.constellation.clone(), f.band.clone()), C / hz);
                    }
                }
            }
        }
        // 钟跳
        {
            let mut stmt = conn.prepare(
                "SELECT json FROM raw_other WHERE introduced_event=?1 AND kind='clock'").unwrap();
            for j in stmt.query_map(params![ev.event_id], |r| r.get::<_, String>(0)).unwrap().flatten() {
                if let Ok(c) = serde_json::from_str::<ClockEvent>(&j) {
                    if let Some(ns) = c.jump_ns {
                        ws.clocks.insert((c.station, c.epoch), ns);
                    }
                }
            }
        }
        // 观测：只取仍生效（未被补丁取代）的行
        {
            let mut stmt = conn.prepare(
                "SELECT json FROM raw_obs
                 WHERE introduced_event=?1 AND superseded_event IS NULL").unwrap();
            for j in stmt.query_map(params![ev.event_id], |r| r.get::<_, String>(0)).unwrap().flatten() {
                if let Ok(o) = serde_json::from_str::<RawObservation>(&j) {
                    let key = (o.station.clone(), o.sat.clone(), o.band.clone());
                    ws.series.entry(key).or_default().push(EpochSample {
                        epoch: o.epoch,
                        p: o.pseudorange_m,
                        l: o.carrier_cycles,
                        doppler: o.doppler_hz,
                        lli: o.lli_flag,
                    });
                    if let Some(flag) = o.lli_flag {
                        ws.llis.entry((o.station.clone(), o.sat.clone(), o.epoch))
                            .or_default().insert(o.band.clone(), flag);
                    }
                }
            }
        }
    }

    fn build_derived(
        conn: &rusqlite::Connection, branch: &str, ws: &Workspace, pfp: &str,
    ) {
        let p = &ws.params;

        // 1) 按站收集历元网格（不等间隔显式保留）
        let mut station_epochs: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
        for ((st, _sat, band), samples) in &ws.series {
            // 弧段行：每站星频，按 max_gap 切分
            let mut sorted = samples.clone();
            sorted.sort_by(|a, b| a.epoch.partial_cmp(&b.epoch).unwrap());
            sorted.dedup_by_key(|s| s.epoch);
            let mut seg_start_idx = 0usize;
            for i in 1..=sorted.len() {
                let split = i == sorted.len()
                    || sorted[i].epoch - sorted[i - 1].epoch > p.max_gap_s;
                if split {
                    let seg = &sorted[seg_start_idx..i];
                    if seg.len() >= 2 {
                        let body = json!({
                            "station": st, "sat": _sat, "band": band,
                            "epoch_start": seg.first().unwrap().epoch,
                            "epoch_end": seg.last().unwrap().epoch,
                            "sample_count": seg.len(),
                            "intervals_s": seg.windows(2)
                                .map(|w| q9(w[1].epoch - w[0].epoch)).collect::<Vec<_>>(),
                            "unit_epoch": "s",
                        });
                        let id = stable_id("arc_", json!({
                            "station": st, "sat": _sat, "band": band,
                            "start": seg.first().unwrap().epoch,
                            "end": seg.last().unwrap().epoch,
                            "param": pfp, "algo": ALGO_VERSION,
                        }));
                        insert_derived(conn, branch, "arc", "current", st, _sat,
                            Some(seg.first().unwrap().epoch), Some(band.clone()), None,
                            &vec![], id, pfp, body);
                    }
                    seg_start_idx = i;
                }
            }
            for s in &sorted {
                station_epochs.entry(st.clone()).or_default()
                    .insert((s.epoch * 1000.0).round() as i64);
            }
        }

        // 2) 站历元网格行
        for (st, epochs) in &station_epochs {
            let eps: Vec<f64> = epochs.iter().map(|e| *e as f64 / 1000.0).collect();
            let body = json!({
                "station": st,
                "epochs_s": eps,
                "intervals_s": eps.windows(2).map(|w| q9(w[1]-w[0])).collect::<Vec<_>>(),
                "uniform": eps.windows(2).all(|w| (w[1]-w[0]-(eps[1]-eps[0])).abs() < 1e-9),
                "unit": "s",
            });
            let id = stable_id("grid_", json!({"station": st, "epochs": eps, "param": pfp}));
            insert_derived(conn, branch, "epoch_grid", "current", st, "",
                None, None, None, &vec![], id, pfp, body);
        }

        // 3) 钟跳事件派生行（整机共模，独立于单星）
        for ((st, epoch), ns) in &ws.clocks {
            let body = json!({
                "station": st, "epoch_s": epoch, "jump_ns": ns,
                "jump_m": q9(*ns * 1e-9 * C),
                "scope": "receiver_common_mode",
                "unit_jump": "ns", "unit_range": "m",
            });
            let id = stable_id("clockjump_", json!({"station": st, "epoch": epoch, "ns": q9(*ns)}));
            insert_derived(conn, branch, "clock_jump", "current", st, "",
                Some(*epoch), None, None, &vec![], id, pfp, body);
        }

        // 4) 站星边界：逐历元相邻样本做多检测器评估
        let mut sats_by_station: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut bands_by_sat: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for ((st, sat, band), _) in &ws.series {
            sats_by_station.entry(st.clone()).or_default().insert(sat.clone());
            bands_by_sat.entry(sat.clone()).or_default().insert(band.clone());
        }

        for (st, sats) in &sats_by_station {
            for sat in sats {
                let bands: Vec<String> = bands_by_sat[sat].iter().cloned().collect();
                // 以该站星所有波段历元并集作为边界
                let mut all_epochs: BTreeSet<i64> = BTreeSet::new();
                for band in &bands {
                    if let Some(s) = ws.series.get(&(st.clone(), sat.clone(), band.clone())) {
                        for e in s { all_epochs.insert((e.epoch * 1000.0).round() as i64); }
                    }
                }
                let eps: Vec<f64> = all_epochs.iter().map(|e| *e as f64 / 1000.0).collect();
                for pair in eps.windows(2) {
                    let (e0, e1) = (pair[0], pair[1]);
                    let bkey = format!("{st}|{sat}|{}", e1);
                    let at_clock = ws.clocks.get(&(st.clone(), e1)).copied()
                        .or_else(|| ws.clocks.get(&(st.clone(), e0)).copied());
                    evaluate_boundary(conn, branch, ws, p, pfp, st, sat, &bands, e0, e1,
                        at_clock, &bkey);
                }
            }
        }
    }
}

fn sample_at(
    ws: &Workspace, st: &str, sat: &str, band: &str, epoch: f64,
) -> Option<&EpochSample> {
    ws.series.get(&(st.to_string(), sat.to_string(), band.to_string()))
        .and_then(|v| v.iter().find(|s| (s.epoch - epoch).abs() < 1e-9))
}

fn constellation_of(sat: &str) -> String {
    sat.chars().next().map(|c| c.to_string()).unwrap_or_else(|| "G".into())
}

#[allow(clippy::too_many_arguments)]
fn evaluate_boundary(
    conn: &rusqlite::Connection, branch: &str, ws: &Workspace, p: &Parameters, pfp: &str,
    st: &str, sat: &str, bands: &[String], e0: f64, e1: f64,
    clock_ns: Option<f64>, bkey: &str,
) {
    let gnss = constellation_of(sat);
    let dt = e1 - e0;
    let gap = dt > p.max_gap_s;
    let mut band_status: BTreeMap<String, String> = BTreeMap::new();
    let mut sources: Vec<String> = Vec::new();

    // --- 逐波段时间差分 ---
    let mut td: BTreeMap<String, DetectorResult> = BTreeMap::new();
    for band in bands {
        let cur = sample_at(ws, st, sat, band, e1);
        let prev = sample_at(ws, st, sat, band, e0);
        match (prev, cur) {
            (Some(a), Some(b)) => {
                let have_both = a.l.is_some() && b.l.is_some()
                    && (a.doppler.is_some() || b.doppler.is_some());
                if have_both {
                    let mut prev_c = a.clone();
                    let mut cur_c = b.clone();
                    if clock_ns.is_some() {
                        // 整机钟跳在相位时间差分中表现为共模常数（ns*1e-9*c/lam 周），
                        // 不在这里当单星周跳：扣掉钟跳模型后再看残差。
                        if let (Some(ns), Some(w)) =
                            (clock_ns, ws.waves.get(&(gnss.clone(), band.clone())).copied()) {
                            let cycles = ns * 1e-9 * C / w;
                            if let Some(l) = cur_c.l { cur_c.l = Some(l - cycles); }
                            prev_c.doppler = None;
                        }
                    }
                    let res = td_residual(&prev_c, &cur_c);
                    let status = if clock_ns.is_some() { "clock_corrected" } else { "ok" };
                    td.insert(band.clone(), DetectorResult {
                        detector: "time_difference".into(),
                        status: status.into(),
                        residual: res,
                        sigma: p.td_sigma,
                        unit: "cycles".into(),
                        detail: json!({"dt_s": q9(dt), "gap_exceeds_max": gap,
                                       "clock_ns": clock_ns}),
                        native_candidates: res.map(|r| candidates_around(r, p.td_sigma,
                            p.candidate_radius, band)).unwrap_or_default(),
                        reject_reasons: vec![],
                    });
                    band_status.insert(band.clone(), "present".into());
                } else {
                    band_status.insert(band.clone(), "missing_carrier_or_doppler".into());
                    td.insert(band.clone(), DetectorResult {
                        detector: "time_difference".into(),
                        status: "insufficient".into(), residual: None,
                        sigma: p.td_sigma, unit: "cycles".into(),
                        detail: json!({}), native_candidates: vec![],
                        reject_reasons: vec!["载波或 Doppler 缺失".into()],
                    });
                }
                sources.push(format!("raw:{st}:{sat}:{}:{band}", e1));
            }
            _ => {
                band_status.insert(band.clone(), "missing_frequency".into());
                td.insert(band.clone(), DetectorResult {
                    detector: "time_difference".into(),
                    status: "missing_frequency".into(), residual: None,
                    sigma: p.td_sigma, unit: "cycles".into(),
                    detail: json!({}), native_candidates: vec![],
                    reject_reasons: vec![format!("波段 {band} 在该边界一侧缺测")],
                });
            }
        }
    }

    // --- LLI：只是线索，从不当真值，单独给出，不直接生成周跳结论 ---
    let lli_flags = ws.llis.get(&(st.to_string(), sat.to_string(), e1)).cloned().unwrap_or_default();
    let lli_status = if lli_flags.values().any(|f| *f != 0) {
        "lli_set"
    } else { "clear" };

    // --- GF 电离层组合（需要前两个波段都有载波） ---
    let mut gf: Option<DetectorResult> = None;
    if bands.len() >= 2 {
        let (b1, b2) = (bands[0].clone(), bands[1].clone());
        let w1 = ws.waves.get(&(gnss.clone(), b1.clone())).copied();
        let w2 = ws.waves.get(&(gnss.clone(), b2.clone())).copied();
        let l1p = sample_at(ws, st, sat, &b1, e0).and_then(|s| s.l);
        let l1c = sample_at(ws, st, sat, &b1, e1).and_then(|s| s.l);
        let l2p = sample_at(ws, st, sat, &b2, e0).and_then(|s| s.l);
        let l2c = sample_at(ws, st, sat, &b2, e1).and_then(|s| s.l);
        if let (Some(w1), Some(w2)) = (w1, w2) {
            if let Some(gf_res) = gf_residual_cycles(l1p, l1c, l2p, l2c,
                &BandInfo { band: b1.clone(), wavelength_m: w1 },
                &BandInfo { band: b2.clone(), wavelength_m: w2 }) {
                let status = if clock_ns.is_some() { "clock_common_mode_cancelled" } else { "ok" };
                let mut nats = Vec::new();
                // GF 只约束差分 n1-n2；枚举 (n1,n2) 使 (n1 - lam2/lam1 n2) 接近残差
                let ratio = w2 / w1;
                let radius = p.candidate_radius;
                for n1 in -radius..=radius {
                    for n2 in -radius..=radius {
                        let predicted = n1 as f64 - ratio * n2 as f64;
                        let dist = (gf_res - predicted).abs();
                        if dist <= 3.0 * p.gf_sigma {
                            nats.push(NativeCandidate {
                                vector: BTreeMap::from([(b1.clone(), n1), (b2.clone(), n2)]),
                                score: q9(dist / p.gf_sigma),
                            });
                        }
                    }
                }
                nats.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
                gf = Some(DetectorResult {
                    detector: "ionosphere_free_gf".into(),
                    status: status.into(),
                    residual: Some(gf_res), sigma: p.gf_sigma, unit: "cycles(b1)",
                    detail: json!({"bands": [b1, b2], "wavelength_ratio": q9(ratio),
                                   "clock_ns": clock_ns}),
                    native_candidates: nats, reject_reasons: vec![],
                });
            } else {
                gf = Some(missing_result("ionosphere_free_gf", p.gf_sigma,
                    "双频载波在该边界不齐，GF 不评估"));
            }
        }
    }

    // --- Melbourne-Wubbena（需要双频伪距+载波；钟跳边界伪距受污染则标记，不强行下结论） ---
    let mut mw: Option<DetectorResult> = None;
    if bands.len() >= 2 {
        let (b1, b2) = (bands[0].clone(), bands[1].clone());
        if let (Some(w1), Some(w2)) = (
            ws.waves.get(&(gnss.clone(), b1.clone())).copied(),
            ws.waves.get(&(gnss.clone(), b2.clone())).copied(),
        ) {
            let c1 = sample_at(ws, st, sat, &b1, e1);
            let c2 = sample_at(ws, st, sat, &b2, e1);
            if let (Some(s1), Some(s2)) = (c1, c2) {
                if clock_ns.is_some() {
                    mw = Some(DetectorResult {
                        detector: "melbourne_wubbena".into(),
                        status: "masked_clock".into(), residual: None,
                        sigma: p.mw_sigma, unit: "wide_lane_cycles",
                        detail: json!({"clock_ns": clock_ns,
                                       "reason": "整机钟跳污染伪距，MW 该边界不参评"}),
                        native_candidates: vec![],
                        reject_reasons: vec!["钟跳边界 MW 掩码".into()],
                    });
                } else if let (Some(l1), Some(l2), Some(p1v), Some(p2v)) =
                    (s1.l, s2.l, s1.p, s2.p) {
                    let value = mw_widelane_cycles(l1, l2, p1v, p2v,
                        &BandInfo { band: b1.clone(), wavelength_m: w1 },
                        &BandInfo { band: b2.clone(), wavelength_m: w2 });
                    let frac = value - value.round();
                    let nats = candidates_around(frac, p.mw_sigma, p.candidate_radius, "wide_lane");
                    mw = Some(DetectorResult {
                        detector: "melbourne_wubbena".into(),
                        status: "ok".into(), residual: Some(q9(frac)),
                        sigma: p.mw_sigma, unit: "wide_lane_cycles",
                        detail: json!({"raw_widelane": value, "bands": [b1, b2]}),
                        native_candidates: nats, reject_reasons: vec![],
                    });
                } else {
                    mw = Some(missing_result("melbourne_wubbena", p.mw_sigma,
                        "双频伪距或载波缺失，MW 不评估"));
                }
            }
        }
    }

    write_boundary(conn, branch, ws, p, pfp, st, sat, e1, bkey, dt, gap,
        &td, gf, mw, lli_status, &lli_flags, &band_status, clock_ns, &sources);
}

fn missing_result(name: &str, sigma: f64, why: &str) -> DetectorResult {
    DetectorResult {
        detector: name.into(), status: "insufficient".into(), residual: None,
        sigma, unit: "cycles".into(), detail: json!({}),
        native_candidates: vec![], reject_reasons: vec![why.to_string()],
    }
}

#[allow(clippy::too_many_arguments)]
fn write_boundary(
    conn: &rusqlite::Connection, branch: &str, ws: &Workspace, p: &Parameters, pfp: &str,
    st: &str, sat: &str, e1: f64, bkey: &str, dt: f64, gap: bool,
    td: &BTreeMap<String, DetectorResult>, gf: Option<DetectorResult>,
    mw: Option<DetectorResult>, lli_status: &str,
    lli_flags: &BTreeMap<String, i64>,
    band_status: &BTreeMap<String, String>, clock_ns: Option<f64>,
    sources: &[String],
) {
    let td_bands: Vec<String> = td.keys().cloned().collect();

    // 合法向量集合：以 TD 各波段候选做笛卡尔基（未参评波段固定为 0），
    // 再并上 GF 的双频向量。MW 只评宽巷差，不单独定向量，仅作评分证据。
    let mut vectors: BTreeSet<Vec<i64>> = BTreeSet::new();
    vectors.insert(vec![0; td_bands.len()]);
    for (i, band) in td_bands.iter().enumerate() {
        if let Some(r) = td.get(band) {
            if r.residual.is_some() {
                let nearest = r.residual.unwrap().round() as i64;
                let mut v = vec![0; td_bands.len()];
                v[i] = nearest;
                vectors.insert(v);
                // 等分整周：当残差落在整数中间，nearest±1 都保留
                v[i] = nearest + 1;
                vectors.insert(v.clone());
                v[i] = nearest - 1;
                vectors.insert(v);
            }
        }
    }
    if let Some(g) = &gf {
        for nc in &g.native_candidates {
            let v: Vec<i64> = td_bands.iter().map(|b| *nc.vector.get(b).unwrap_or(&0)).collect();
            vectors.insert(v);
        }
    }

    let mut candidate_rows: Vec<serde_json::Value> = Vec::new();
    let mut scored: Vec<(Vec<i64>, f64, BTreeMap<String, f64>)> = Vec::new();
    for v in &vectors {
        let map: BTreeMap<String, i64> = td_bands.iter().enumerate()
            .map(|(i, b)| (b.clone(), v[i])).collect();
        let mut per_det: BTreeMap<String, f64> = BTreeMap::new();
        // TD 分数：该向量下逐波段残差距离/sigma，取最差波段
        let mut td_max: Option<f64> = None;
        for (i, band) in td_bands.iter().enumerate() {
            if let Some(r) = td.get(band) {
                if let Some(res) = r.residual {
                    let d = (res - v[i] as f64).abs() / p.td_sigma;
                    td_max = Some(td_max.map_or(d, |m| m.max(d)));
                }
            }
        }
        if let Some(d) = td_max { per_det.insert("time_difference".into(), q9(d)); }

        // GF 分数：只在双频向量与 GF 可用时
        if td_bands.len() >= 2 {
            if let Some(g) = &gf {
                if let (Some(res), Some(w_ratio)) =
                    (g.residual, g.detail.get("wavelength_ratio").and_then(|x| x.as_f64())) {
                    let predicted = v[0] as f64 - w_ratio * v[1] as f64;
                    let d = (res - predicted).abs() / p.gf_sigma;
                    per_det.insert("ionosphere_free_gf".into(), q9(d));
                }
            }
            // MW 分数：宽巷差 (n1-n2) 对应的小数残差
            if let Some(m) = &mw {
                if let (Some(res), Some(info)) = (m.residual,
                    m.detail.get("raw_widelane").and_then(|x| x.as_f64())) {
                    let wide_int = info.round() as i64 + (v[0] - v[1]);
                    let predicted_frac = wide_int as f64 - info.round();
                    let d = (res - predicted_frac).abs() / p.mw_sigma;
                    per_det.insert("melbourne_wubbena".into(), q9(d));
                }
            }
        }

        // 综合分：可用检测器中的最差归一化距离（保守，不允许一个检测器替另一个背书）
        if !per_det.is_empty() {
            let combined = per_det.values().cloned()
                .fold(0f64, f64::max);
            scored.push((v.clone(), q9(combined), per_det));
        }
    }
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let boundary_id = stable_id("bnd_", json!({"station": st, "sat": sat, "epoch": e1}));
    let accepted = ws.accepted.get(bkey).cloned();

    for (rank, (v, combined, per_det)) in scored.iter().enumerate() {
        let map: BTreeMap<String, i64> = td_bands.iter().enumerate()
            .map(|(i, b)| (b.clone(), v[i])).collect();
        let candidate_id = stable_id("cand_", json!({
            "boundary": boundary_id, "vector": map,
        }));
        let mut state = "unknown";
        let mut reject: Vec<String> = Vec::new();
        if let Some((_, ref avec)) = accepted {
            if avec == &map { state = "accepted"; }
            else {
                state = "rejected";
                reject.push("另一候选已被人工接受".into());
            }
        } else if let Some(evt) = ws.rejected.get(&candidate_id) {
            state = "rejected";
            reject.push(format!("人工否定（事件 {evt}）"));
        } else {
            // 不把任何检测器当真值：只有综合最优且与次优明确分开才自动 current，
            // 多个等分整周偏移并列 -> ambiguous，全部合法候选保留。
            let best = scored.first().map(|x| x.1).unwrap_or(f64::INFINITY);
            let tied: Vec<_> = scored.iter().filter(|x| (x.1 - best).abs() < 1e-9).collect();
            if (combined - best).abs() < 1e-9 && tied.len() == 1 {
                state = "current";
            } else if (combined - best).abs() < 1e-9 {
                state = "ambiguous";
            } else {
                state = "rejected";
                reject.push(format!("综合得分 {combined} 次于最优 {best}"));
            }
        }

        let body = json!({
            "candidate_id": candidate_id,
            "boundary_id": boundary_id,
            "station": st, "sat": sat, "epoch_s": e1, "dt_s": q9(dt),
            "gap_exceeds_max": gap,
            "vector_cycles": map,
            "combined_score": combined,
            "per_detector_sigma_distance": per_det,
            "rank": rank,
            "state": state,
            "reject_reasons": reject,
            "lli_status": lli_status,
            "lli_flags": lli_flags,
            "clock_ns": clock_ns,
            "unit": "cycles",
        });
        let id = stable_id("candrow_", json!({"candidate": candidate_id, "param": pfp, "algo": ALGO_VERSION}));
        candidate_rows.push(body.clone());
        insert_derived(conn, branch, "candidate", state, st, sat, Some(e1), None,
            Some(*combined), sources, id, pfp, body);
    }

    let detectors_json = json!({
        "time_difference": td,
        "ionosphere_free_gf": gf,
        "melbourne_wubbena": mw,
    });
    let boundary_body = json!({
        "boundary_id": boundary_id,
        "station": st, "sat": sat, "epoch_s": e1, "dt_s": q9(dt),
        "gap_exceeds_max": gap,
        "band_status": band_status,
        "lli_status": lli_status,
        "lli_flags": lli_flags,
        "clock_ns": clock_ns,
        "detectors": detectors_json,
        "candidate_ids": candidate_rows.iter()
            .map(|c| c["candidate_id"].clone()).collect::<Vec<_>>(),
        "accepted_vector": accepted.as_ref().map(|(_, m)| json!(m)),
        "unit_epoch": "s",
    });
    let bid = stable_id("bndrow_", json!({"boundary": boundary_id, "param": pfp, "algo": ALGO_VERSION}));
    insert_derived(conn, branch, "boundary", "current", st, sat, Some(e1), None,
        None, sources, bid, pfp, boundary_body);

    // 修复只存偏移层：accepted 决策产生一条独立 offset 行，不回改原始载波。
    if let Some((evt_id, map)) = &accepted {
        for (band, cycles) in map {
            if *cycles != 0 {
                let body = json!({
                    "boundary_id": boundary_id, "station": st, "sat": sat,
                    "epoch_s": e1, "band": band, "offset_cycles": cycles,
                    "decision_event": evt_id,
                    "layer": "repair_offset_only",
                    "raw_carrier_modified": false,
                    "unit": "cycles",
                });
                let id = stable_id("offset_", json!({
                    "boundary": boundary_id, "band": band, "vector": cycles,
                    "event": evt_id}));
                insert_derived(conn, branch, "repair_offset", "accepted", st, sat,
                    Some(e1), Some(band.clone()), None, sources, id, pfp, body);
            }
        }
    }
}

fn insert_derived(
    conn: &rusqlite::Connection, branch: &str, kind: &str, state: &str,
    station: &str, sat: &str, epoch: Option<f64>, band: Option<String>,
    score: Option<f64>, sources: &[String], id: String, pfp: &str,
    body: serde_json::Value,
) {
    conn.execute(
        "INSERT OR REPLACE INTO derived
         (derived_id, kind, branch, created_event, state, station, sat, epoch, band,
          score, sources, algo_version, param_fp, json)
         VALUES (?1,?2,?3,'REPLAY',?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![id, kind, branch, state, station, sat, epoch, band, score,
                serde_json::to_string(sources).unwrap(), ALGO_VERSION, pfp,
                serde_json::to_string(&body).unwrap()],
    ).unwrap();
}

#[derive(Serialize)]
pub struct ProjectState {
    pub branch: String,
    pub branches: Vec<BranchRecord>,
    pub events: Vec<EventRecord>,
    pub params: Parameters,
    pub raw_observations: Vec<serde_json::Value>,
    pub raw_clocks: Vec<serde_json::Value>,
    pub raw_freqs: Vec<serde_json::Value>,
    pub raw_stations: Vec<serde_json::Value>,
    pub raw_sat_states: Vec<serde_json::Value>,
    pub diagnostics: Vec<serde_json::Value>,
    pub arcs: Vec<serde_json::Value>,
    pub grids: Vec<serde_json::Value>,
    pub clock_jumps: Vec<serde_json::Value>,
    pub boundaries: Vec<serde_json::Value>,
    pub candidates: Vec<serde_json::Value>,
    pub offsets: Vec<serde_json::Value>,
    pub stale_boundaries: Vec<serde_json::Value>,
    pub stale_candidates: Vec<serde_json::Value>,
    pub view: ViewInfo,
}

#[derive(Serialize)]
pub struct ViewInfo {
    pub mode: String,
    pub at_event: Option<String>,
    pub note: String,
}

impl Replay {
    /// 读取一个视图。at_event=None 为当前；给事件 id 则把该事件之后的派生行
    /// 视为“未来/过期”，只展示其祖先事件所对应的最新一代结论。
    pub fn state(db: &Db, branch: &str, at_event: Option<&str>) -> ProjectState {
        let conn = db.conn.lock().unwrap();
        let all_chain = effective_chain(&conn, branch);
        let (visible_events, mode, note) = if let Some(stop) = at_event {
            let mut visible: Vec<EventRecord> = Vec::new();
            for ev in &all_chain {
                visible.push(ev.clone());
                if ev.event_id == stop { break; }
            }
            let found = visible.iter().any(|e| e.event_id == stop);
            if !found {
                (all_chain.clone(), "current".into(),
                    "指定事件不在该分支祖先链上，回退到当前视图".to_string())
            } else {
                (visible, "historical".into(),
                    format!("历史视图，截至事件 {stop}；之后的结论标记为过期，不可冒充当前"))
            }
        } else {
            (all_chain.clone(), "current".into(), "当前视图".to_string())
        };

        let mut p = Parameters::default();
        for ev in &visible_events {
            if ev.kind == "params" {
                if let Some(v) = ev.payload.get("params") {
                    p = serde_json::from_value(v.clone()).unwrap_or_default();
                }
            }
        }

        // 可见的派生行：current 代；历史视图下，需要从全链可见事件重建“当时一代”。
        // 做法：当前表中 current 行为最新代；stale 行保留每次重放归档。
        // 我们按 created_event 落在可见事件集合内 且是最靠近停点的一代选取。
        let visible_ids: BTreeSet<String> = visible_events.iter().map(|e| e.event_id.clone()).collect();
        let read_rows = |kind: &str, want_state: Option<&str>| -> Vec<serde_json::Value> {
            let mut stmt = conn.prepare(
                "SELECT json, state, created_event FROM derived
                 WHERE branch=?1 AND kind=?2 ORDER BY epoch, derived_id").unwrap();
            let rows = stmt.query_map(params![branch, kind], |r| {
                let j: String = r.get(0)?;
                let st: String = r.get(1)?;
                let ce: String = r.get(2)?;
                Ok((serde_json::from_str::<serde_json::Value>(&j).unwrap_or(json!({})), st, ce))
            }).unwrap().flatten().collect::<Vec<_>>();
            if at_event.is_none() {
                rows.into_iter().filter(|(_, st, _)| match want_state {
                    Some(w) => st == w,
                    None => *st != "stale",
                }).map(|(j, _, _)| j).collect()
            } else {
                // 历史视图：分组按 body 稳定身份取 created_event 可见的最新一代
                let mut best: BTreeMap<String, (serde_json::Value, String, String)> = BTreeMap::new();
                for (j, st, ce) in rows {
                    let key = j.get("boundary_id").or_else(|| j.get("candidate_id"))
                        .or_else(|| j.get("derived_id"))
                        .map(|x| x.to_string()).unwrap_or_default();
                    if visible_ids.contains(&ce) {
                        best.entry(key)
                            .and_modify(|cur| {
                                if order_event(&all_chain, &ce, &cur.2).is_gt() {
                                    *cur = (j.clone(), st.clone(), ce.clone());
                                }
                            })
                            .or_insert((j, st, ce));
                    }
                }
                best.into_values().map(|(j, st, _)| {
                    let mut j = j;
                    if st == "stale" {
                        j["view_state"] = json!("stale");
                    }
                    j
                }).collect()
            }
        };

        let raw_json = |sql_kind: &str| -> Vec<serde_json::Value> {
            let mut stmt = conn.prepare(
                "SELECT json FROM raw_other WHERE kind=?1 AND superseded_event IS NULL
                 ORDER BY key, raw_id").unwrap();
            stmt.query_map(params![sql_kind], |r| {
                Ok(serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(0)?).unwrap())
            }).unwrap().flatten().collect()
        };

        let raw_observations: Vec<serde_json::Value> = {
            let mut stmt = conn.prepare(
                "SELECT json FROM raw_obs WHERE superseded_event IS NULL
                 ORDER BY station, sat, epoch, band").unwrap();
            stmt.query_map([], |r| {
                Ok(serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(0)?).unwrap())
            }).unwrap().flatten().collect()
        };
        let diagnostics: Vec<serde_json::Value> = {
            let mut stmt = conn.prepare(
                "SELECT diag_id, introduced_event, kind, line_no, message
                 FROM diagnostics ORDER BY introduced_event, line_no").unwrap();
            stmt.query_map([], |r| Ok(json!({
                "diag_id": r.get::<_, String>(0)?,
                "introduced_event": r.get::<_, String>(1)?,
                "code": r.get::<_, String>(2)?,
                "line_no": r.get::<_, Option<i64>>(3)?,
                "message": r.get::<_, String>(4)?,
            }))).unwrap().flatten().collect()
        };

        ProjectState {
            branch: branch.to_string(),
            branches: EventAppender::branches_reentrant(&conn),
            events: all_chain,
            params: p,
            raw_observations,
            raw_clocks: raw_json("clock"),
            raw_freqs: raw_json("freqdef"),
            raw_stations: raw_json("station"),
            raw_sat_states: raw_json("satstate"),
            diagnostics,
            arcs: read_rows("arc", Some("current")),
            grids: read_rows("epoch_grid", Some("current")),
            clock_jumps: read_rows("clock_jump", Some("current")),
            boundaries: read_rows("boundary", Some("current")),
            candidates: read_rows("candidate", None),
            offsets: read_rows("repair_offset", Some("accepted")),
            stale_boundaries: read_rows("boundary", Some("stale")),
            stale_candidates: read_rows("candidate", Some("stale")),
            view: ViewInfo { mode, at_event: at_event.map(|s| s.to_string()), note },
        }
    }
}

enum Ordering { Lt, Eq, Gt }
impl Ordering { fn is_gt(&self) -> bool { matches!(self, Ordering::Gt) } }

fn order_event(chain: &[EventRecord], a: &str, b: &str) -> Ordering {
    let pos = |id: &str| chain.iter().position(|e| e.event_id == id);
    match (pos(a), pos(b)) {
        (Some(x), Some(y)) if x > y => Ordering::Gt,
        (Some(x), Some(y)) if x == y => Ordering::Eq,
        _ => Ordering::Lt,
    }
}
