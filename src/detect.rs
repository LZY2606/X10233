//! 检测层：电离层组合(GF)、Melbourne-Wubbena、时间差分各自给出候选，
//! 不以任一检测器当真值；整机钟跳与单星周跳分开；无法唯一决定时保留全部合法候选。

use std::collections::BTreeMap;

use crate::model::{freq_hz, Diag, Params, C_MPS};

#[derive(Debug, Clone)]
pub struct EpochRow {
    pub uid: String,
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub freq: i64,
    pub carrier: Option<f64>,
    pub pr_m: Option<f64>,
    pub lli: i64,
}

#[derive(Debug, Clone)]
pub struct NewCandidate {
    pub arc_key: String,
    pub station: String,
    pub sat: String,
    pub freq: Option<i64>,
    pub epoch_ms: i64,
    pub detector: String,
    pub combo: String,
    pub est_cycles: Option<f64>,
    pub est_int: Option<i64>,
    pub score: f64,
    pub reason: Option<String>,
    pub ambig_group: Option<String>,
    /// 初始即被拒绝（如 LLI 误报、被钟跳吸收），由 system 记状态事件
    pub auto_reject: Option<String>,
}

#[derive(Debug, Default)]
pub struct DetectOutcome {
    pub candidates: Vec<NewCandidate>,
    pub diagnostics: Vec<Diag>,
}

#[derive(Debug)]
pub struct Arc {
    pub index: usize,
    pub key: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub rows: Vec<EpochRow>,
    /// 断弧原因（gap / lli）
    pub broke_by: Option<String>,
}

/// 按 (站,星,频) 分组并切弧：不等间隔历元用真实 dt；dt > factor*中位dt 或 LLI bit0 置位即断弧。
pub fn build_arcs(rows: &[EpochRow], params: &Params) -> Vec<Arc> {
    let mut groups: BTreeMap<(String, String, i64), Vec<EpochRow>> = BTreeMap::new();
    for r in rows {
        groups
            .entry((r.station.clone(), r.sat.clone(), r.freq))
            .or_default()
            .push(r.clone());
    }
    let mut arcs = Vec::new();
    for ((st, sat, freq), mut rs) in groups {
        rs.sort_by_key(|r| r.epoch_ms);
        let mut dts: Vec<i64> = rs.windows(2).map(|w| w[1].epoch_ms - w[0].epoch_ms).collect();
        dts.sort();
        let median_dt = if dts.is_empty() { 1 } else { dts[dts.len() / 2].max(1) };
        let max_gap = (params.max_gap_factor * median_dt as f64) as i64;
        let mut cur: Vec<EpochRow> = Vec::new();
        let mut broke: Option<String> = None;
        let mut idx = 0;
        let mut flush = |cur: &mut Vec<EpochRow>, idx: &mut usize, broke: &mut Option<String>, arcs: &mut Vec<Arc>| {
            if cur.is_empty() {
                return;
            }
            let key = format!("{}/{}/f{}#a{}", st, sat, freq, *idx);
            arcs.push(Arc {
                index: *idx,
                key,
                start_ms: cur.first().unwrap().epoch_ms,
                end_ms: cur.last().unwrap().epoch_ms,
                rows: std::mem::take(cur),
                broke_by: broke.take(),
            });
            *idx += 1;
        };
        for (i, r) in rs.iter().enumerate() {
            if i > 0 {
                let dt = r.epoch_ms - rs[i - 1].epoch_ms;
                if dt > max_gap {
                    flush(&mut cur, &mut idx, &mut broke, &mut arcs);
                    broke = Some(format!("gap:{}ms>{}ms", dt, max_gap));
                } else if r.lli & 1 == 1 {
                    flush(&mut cur, &mut idx, &mut broke, &mut arcs);
                    broke = Some("lli".into());
                }
            }
            cur.push(r.clone());
        }
        flush(&mut cur, &mut idx, &mut broke, &mut arcs);
    }
    arcs
}

fn score_of(est: f64) -> f64 {
    let dist = (est - est.round()).abs();
    1.0 / (1.0 + 10.0 * dist)
}

/// 线性外推预测（容忍不等间隔历元），返回 (residual, predicted)。
fn linear_residual(xs: &[(i64, f64)], i: usize) -> Option<f64> {
    if i < 2 {
        return None;
    }
    let (t0, x0) = xs[i - 2];
    let (t1, x1) = xs[i - 1];
    let (t2, x2) = xs[i];
    if t1 == t0 {
        return None;
    }
    let predicted = x1 + (x1 - x0) * ((t2 - t1) as f64 / (t1 - t0) as f64);
    Some(x2 - predicted)
}

/// 等分整周偏移：残差小数部分距 ±0.5 小于 ambig_eps 时，无法唯一决定，保留全部合法候选。
fn emit_jump_candidate(
    out: &mut Vec<NewCandidate>,
    base: NewCandidate,
    est: f64,
    ambig_eps: f64,
    ambig_seed: &mut u64,
) {
    let frac = est - est.floor();
    let near_half = (frac - 0.5).abs() <= ambig_eps;
    if near_half {
        *ambig_seed += 1;
        let group = format!("ambig-{:04}", *ambig_seed);
        for k in [est.floor() as i64, est.ceil() as i64] {
            let mut c = base.clone();
            c.est_cycles = Some(est);
            c.est_int = Some(k);
            c.score = 0.5; // 等分：两个候选等可能
            c.ambig_group = Some(group.clone());
            c.reason = Some("ambiguous_offsets:无法唯一决定，保留全部合法候选".into());
            out.push(c);
        }
    } else {
        let mut c = base;
        c.est_cycles = Some(est);
        c.est_int = Some(est.round() as i64);
        c.score = score_of(est);
        out.push(c);
    }
}

fn base_cand(arc_key: &str, st: &str, sat: &str, freq: Option<i64>, epoch_ms: i64, detector: &str, combo: &str) -> NewCandidate {
    NewCandidate {
        arc_key: arc_key.into(),
        station: st.into(),
        sat: sat.into(),
        freq,
        epoch_ms,
        detector: detector.into(),
        combo: combo.into(),
        est_cycles: None,
        est_int: None,
        score: 0.0,
        reason: None,
        ambig_group: None,
        auto_reject: None,
    }
}

pub fn detect(rows: &[EpochRow], imported_clocks: &[(String, i64, f64)], params: &Params) -> DetectOutcome {
    let mut out = DetectOutcome::default();
    let arcs = build_arcs(rows, params);
    let mut ambig_seed: u64 = 0;

    // 1) 时间差分（每频率载波）
    let mut td_cands: Vec<NewCandidate> = Vec::new();
    for arc in &arcs {
        let xs: Vec<(i64, f64)> = arc
            .rows
            .iter()
            .filter_map(|r| r.carrier.map(|c| (r.epoch_ms, c)))
            .collect();
        for i in 0..xs.len() {
            if let Some(resid) = linear_residual(&xs, i) {
                if resid.abs() > params.td_threshold_cycles {
                    let freq = arc.rows[0].freq;
                    let st = arc.rows[0].station.clone();
                    let sat = arc.rows[0].sat.clone();
                    let b = base_cand(&arc.key, &st, &sat, Some(freq), xs[i].0, "time_difference", &format!("L{}", freq));
                    emit_jump_candidate(&mut td_cands, b, resid, params.ambig_eps, &mut ambig_seed);
                }
            }
        }
    }

    // 2) 整机钟跳聚合：同一站同一历元 >= clock_min_sats 颗星的 TD 跳变（换算 ms）一致 => 钟跳，非单星周跳
    let mut by_epoch: BTreeMap<(String, i64), Vec<usize>> = BTreeMap::new();
    for (i, c) in td_cands.iter().enumerate() {
        by_epoch.entry((c.station.clone(), c.epoch_ms)).or_default().push(i);
    }
    let mut absorbed: Vec<bool> = vec![false; td_cands.len()];
    let mut clock_epochs: Vec<(String, i64)> = Vec::new();
    for ((st, ep), idxs) in &by_epoch {
        let mut sats: Vec<String> = idxs.iter().map(|i| td_cands[*i].sat.clone()).collect();
        sats.sort();
        sats.dedup();
        if (sats.len() as i64) < params.clock_min_sats {
            continue;
        }
        let mut jumps_ms: Vec<f64> = idxs
            .iter()
            .filter_map(|i| {
                let c = &td_cands[*i];
                match (c.est_cycles, c.freq.and_then(freq_hz)) {
                    (Some(cy), Some(f)) => Some(cy / f * 1000.0),
                    _ => None,
                }
            })
            .collect();
        if jumps_ms.len() < params.clock_min_sats as usize {
            continue;
        }
        jumps_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = jumps_ms[jumps_ms.len() / 2];
        if jumps_ms.iter().all(|j| (j - med).abs() <= params.clock_tol_ms) {
            for i in idxs {
                absorbed[*i] = true;
            }
            clock_epochs.push((st.clone(), *ep));
            let matched = imported_clocks
                .iter()
                .any(|(s, e, _)| s == st && e == ep);
            let mut c = base_cand(
                &format!("{}/ALL/fALL#clock", st),
                st,
                "ALL",
                None,
                *ep,
                "receiver_clock",
                "ALL",
            );
            c.est_cycles = Some(med); // 单位：毫秒（钟跳）
            c.score = 1.0;
            c.reason = Some(if matched {
                format!("receiver_clock_jump:{:.6}ms，与导入的钟跳事件一致", med)
            } else {
                format!("receiver_clock_jump:{:.6}ms，由多星同历元跳变聚合", med)
            });
            out.candidates.push(c);
        }
    }
    for (i, c) in td_cands.iter().enumerate() {
        if absorbed[i] {
            let mut c = c.clone();
            c.auto_reject = Some("receiver_clock_jump:整机钟跳，不计单星周跳".into());
            out.candidates.push(c);
        } else {
            out.candidates.push(c.clone());
        }
    }

    // 3) 电离层组合(GF) 与 Melbourne-Wubbena：需要同一 (站,星) 的双频历元交集
    let mut by_ss: BTreeMap<(String, String), BTreeMap<i64, BTreeMap<i64, EpochRow>>> = BTreeMap::new();
    for r in rows {
        by_ss
            .entry((r.station.clone(), r.sat.clone()))
            .or_default()
            .entry(r.epoch_ms)
            .or_default()
            .insert(r.freq, r.clone());
    }
    for ((st, sat), by_epoch_rows) in &by_ss {
        let freqs: Vec<i64> = {
            let mut f: Vec<i64> = by_epoch_rows.values().flat_map(|m| m.keys().cloned()).collect();
            f.sort();
            f.dedup();
            f
        };
        let has1 = freqs.contains(&1);
        let has2 = freqs.contains(&2);
        if !has1 || !has2 {
            let missing = if has1 { 2 } else { 1 };
            let n_epochs = by_epoch_rows.len();
            out.diagnostics.push(Diag {
                line: None,
                code: "freq_missing".into(),
                message: format!(
                    "{}/{} 缺少 f{}（{} 个历元），GF/MW 组合不可用，仅时间差分可用",
                    st, sat, missing, n_epochs
                ),
            });
            continue;
        }
        let mut gf: Vec<(i64, f64)> = Vec::new();
        let mut mw: Vec<(i64, f64)> = Vec::new();
        let mut partial = 0usize;
        for (ep, m) in by_epoch_rows {
            match (m.get(&1), m.get(&2)) {
                (Some(r1), Some(r2)) => {
                    if let (Some(l1), Some(l2)) = (r1.carrier, r2.carrier) {
                        gf.push((*ep, l1 - l2));
                        if let (Some(p1), Some(p2)) = (r1.pr_m, r2.pr_m) {
                            let (f1, f2) = (freq_hz(1).unwrap(), freq_hz(2).unwrap());
                            let wl = (f1 * l1 - f2 * l2) / (f1 - f2);
                            let nl_pr = (f1 * p1 + f2 * p2) / (f1 + f2);
                            let lambda_wl = C_MPS / (f1 - f2);
                            mw.push((*ep, wl - nl_pr / lambda_wl));
                        }
                    }
                }
                _ => partial += 1,
            }
        }
        if partial > 0 {
            out.diagnostics.push(Diag {
                line: None,
                code: "freq_missing".into(),
                message: format!(
                    "{}/{} 有 {} 个历元缺另一频率，组合序列在该区间留空（不插值、不丢原始值）",
                    st, sat, partial
                ),
            });
        }
        let arc_key = format!("{}/{}/f1+f2#combo", st, sat);
        for (series, detector, combo, thr) in [
            (&gf, "iono_gf", "GF(L1-L2)", params.gf_threshold_cycles),
            (&mw, "melbourne_wubbena", "MW(widelane)", params.mw_threshold_cycles),
        ] {
            for i in 0..series.len() {
                if let Some(resid) = linear_residual(series, i) {
                    if resid.abs() > thr {
                        let ep = series[i].0;
                        // 钟跳历元上的组合跳变被钟跳吸收，不再另立候选
                        if clock_epochs.iter().any(|(s, e)| s == st && *e == ep) {
                            continue;
                        }
                        let b = base_cand(&arc_key, st, sat, None, ep, detector, combo);
                        emit_jump_candidate(&mut out.candidates, b, resid, params.ambig_eps, &mut ambig_seed);
                    }
                }
            }
        }
    }

    // 4) LLI 误报：LLI 断弧处若没有任何检测器候选 => 记一等数据候选并由 system 标记误报
    for arc in &arcs {
        if arc.broke_by.as_deref() == Some("lli") {
            let ep = arc.start_ms;
            let st = &arc.rows[0].station;
            let sat = &arc.rows[0].sat;
            let any = out.candidates.iter().any(|c| {
                c.station == *st && c.sat == *sat && c.epoch_ms == ep && c.auto_reject.is_none()
            });
            if !any {
                let mut c = base_cand(
                    &arc.key,
                    st,
                    sat,
                    Some(arc.rows[0].freq),
                    ep,
                    "lli_flag",
                    "LLI",
                );
                c.score = 0.0;
                c.auto_reject = Some("lli_false_alarm:LLI 置位但各检测器均无不连续".into());
                out.candidates.push(c);
            }
        }
    }

    out.candidates.sort_by_key(|c| (c.epoch_ms, c.station.clone(), c.sat.clone(), c.detector.clone()));
    out
}
