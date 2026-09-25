//! 检测引擎：在一个不可变来源版本 + 参数指纹上，确定性地产出全部派生结果。
//!
//! 三路检测器相互独立：电离层组合(GF)、Melbourne-Wubbena(MW)、时间差分(TD)。
//! 不以任一检测器当真值：候选、拒绝原因、未知/数据不足都是一等数据，融合时保留全部合法假设。
//! 钟跳（多信号一致）与单星周跳分开分类；修复只表达为整周偏移层。

use crate::derived::*;
use crate::ids::{content_id, joined_id};
use crate::model::*;
use crate::params::DetectorParams;
use std::collections::{BTreeMap, BTreeSet};

/// 频带组合：按频率编号排序后取前两个作为 L1/L2（双频组合）。
fn dual_bands(freqs: &BTreeSet<u8>) -> Option<(u8, u8)> {
    let mut it = freqs.iter().copied();
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

fn norm_obs(o: &RawObservation) -> NormObs {
    let mut diags = Vec::new();
    let grab = |v: &Option<RawValue>| -> Option<f64> {
        if let Some(rv) = v {
            if let Some(d) = &rv.diagnostic {
                diags.push(d.clone());
            }
            rv.normalized
        } else {
            None
        }
    };
    NormObs {
        pseudo_range_m: grab(&o.pseudo_range),
        carrier_phase_cyc: grab(&o.carrier_phase),
        doppler_hz: grab(&o.doppler),
        snr_dbhz: grab(&o.snr_dbhz),
        lli: o.lli,
        diagnostics: diags,
    }
}

/// 取规范化相位（缺测/不可转换为 None）。
fn phase(norm: &NormObs) -> Option<f64> {
    norm.carrier_phase_cyc
}

fn nearest_int(x: f64) -> i64 {
    let r = x.round();
    if r == 0.0 && x.is_sign_negative() {
        0
    } else {
        r as i64
    }
}

/// 在半径内枚举整周偏移，残差绝对值 <= 阈值（多解时由融合阶段保留全部）。
fn integer_offsets(residual: f64, radius: i64, tol: f64) -> Vec<(i64, f64)> {
    let center = nearest_int(residual);
    let mut out = Vec::new();
    for k in (-radius)..=radius {
        let n = center + k;
        let r = residual - n as f64;
        if r.abs() <= tol {
            out.push((n, r));
        }
    }
    out
}

/// 单个频带的时间差分（Doppler 预测）过渡记录。
fn build_transition(
    imp: &RawImport,
    params: &DetectorParams,
    epoch: usize,
    sat: &str,
    band: u8,
    norm: &[BTreeMap<String, BTreeMap<u8, NormObs>>],
) -> Option<BandTransition> {
    let prev = norm[epoch - 1].get(sat)?.get(&band)?;
    let cur = norm[epoch].get(sat)?.get(&band)?;
    let lp = phase(prev)?;
    let lc = phase(cur)?;
    let dt = imp.dt_seconds[epoch];
    let observed = lc - lp;

    // Doppler 预测：RINEX 约定 Doppler ≈ -dL/dt（Hz 即 周/秒）。
    // 取两端 Doppler 均值预测整周增量。缺 Doppler 时无法预测 -> Unknown（不当作无周跳）。
    let predicted = match (prev.doppler_hz, cur.doppler_hz, dt) {
        (Some(dp), Some(dc), Some(dt)) if dt > 0.0 => Some(-0.5 * (dp + dc) * dt),
        _ => None,
    };

    let id = joined_id("trn", &[&imp.id, &epoch.to_string(), sat, &band.to_string(), &params.fingerprint()]);
    let mut candidates = Vec::new();
    let (residual, status) = match predicted {
        Some(pred) => {
            let r = observed - pred;
            let ints = integer_offsets(r, params.search_radius_cycles, params.td_threshold_cycles);
            for (n, resid) in &ints {
                if *n != 0 {
                    candidates.push(DetectorCandidate {
                        detector: DetectorKind::TimeDifference,
                        band,
                        offset_cycles: *n,
                        residual_cycles: *resid,
                    });
                }
            }
            if ints.iter().any(|(n, _)| *n == 0) {
                (Some(r), DetectorVerdict::NoSlip)
            } else {
                (Some(r), DetectorVerdict::Candidate)
            }
        }
        None => (None, DetectorVerdict::Unknown),
    };

    Some(BandTransition {
        id,
        epoch_index: epoch,
        satellite: sat.to_string(),
        band,
        dt_seconds: dt,
        td_residual_cycles: residual,
        doppler_predicted_delta_cyc: predicted,
        observed_delta_cyc: Some(observed),
        candidates,
        lli_flag: cur.lli,
        status,
        source_import: imp.id.clone(),
        algo_version: params.algo_version.clone(),
        param_fingerprint: params.fingerprint(),
    })
}

/// 双频 GF 与 MW 组合残差（每个卫星每个边界一条）。
fn build_combo(
    params: &DetectorParams,
    epoch: usize,
    sat: &str,
    b1: u8,
    b2: u8,
    f1: &FreqSpec,
    f2: &FreqSpec,
    norm: &[BTreeMap<String, BTreeMap<u8, NormObs>>],
    imp: &RawImport,
    mw_prev: &[f64],
) -> Option<ComboResidual> {
    let get = |e: usize, band: u8| -> Option<&NormObs> { norm[e].get(sat).and_then(|m| m.get(&band)) };
    let (p0, c0) = (get(epoch - 1, b1)?, get(epoch - 1, b2)?);
    let (p1, c1) = (get(epoch, b1), get(epoch, b2));
    // 任一端点缺任一频带 => 频率缺失，组合无数据。
    let (p1, c1) = (p1?, c1?);

    // ---- 电离层（几何无关）组合：L_I = L1 - (f1/f2) L2，单位 L1 周 ----
    let ratio = f1.frequency_hz / f2.frequency_hz;
    let gf = |a: &NormObs, b: &NormObs| -> Option<f64> {
        Some(a.carrier_phase_cyc? - ratio * b.carrier_phase_cyc?)
    };
    let gf0 = gf(p0, c0)?;
    let gf1 = gf(p1, c1)?;
    let gf_jump = gf1 - gf0;
    // k1 - ratio*k2 = jump（k1,k2 整数）。枚举 k2，推导 k1。
    let mut gf_candidates: Vec<DetectorCandidate> = Vec::new();
    let mut gf_pairs: Vec<(i64, i64, f64)> = Vec::new();
    let mut gf_any_zero = false;
    for k2 in (-params.search_radius_cycles)..=params.search_radius_cycles {
        let k1f = gf_jump + ratio * k2 as f64;
        let k1 = nearest_int(k1f);
        let resid = k1f - k1 as f64;
        if resid.abs() <= params.gf_threshold_cycles {
            if k1 == 0 && k2 == 0 {
                gf_any_zero = true;
            }
            gf_candidates.push(DetectorCandidate { detector: DetectorKind::GeometryFree, band: b1, offset_cycles: k1, residual_cycles: resid });
            gf_candidates.push(DetectorCandidate { detector: DetectorKind::GeometryFree, band: b2, offset_cycles: k2, residual_cycles: resid });
            gf_pairs.push((k1, k2, resid));
        }
    }
    // 去重（k1,k2 成对）。
    gf_candidates.dedup_by_key(|c| (c.band, c.offset_cycles));
    let gf_status = if gf_any_zero {
        DetectorVerdict::NoSlip
    } else if !gf_candidates.is_empty() {
        DetectorVerdict::Candidate
    } else {
        DetectorVerdict::InsufficientData
    };

    // ---- Melbourne-Wubbena 宽巷 ----
    // WL(周) = (f1 L1 - f2 L2)/(f1-f2)；NL(m) = (f1 P1 + f2 P2)/(f1+f2)
    // N_w(WL 周) = WL - NL / lambda_w。
    let lambda_w = crate::mw::wl_wavelength(f1, f2);
    let nwl = |a: &NormObs, b: &NormObs| -> Option<f64> {
        let l1 = a.carrier_phase_cyc?;
        let l2 = b.carrier_phase_cyc?;
        let pr1 = a.pseudo_range_m?;
        let pr2 = b.pseudo_range_m?;
        let wl_cyc = (f1.frequency_hz * l1 - f2.frequency_hz * l2) / (f1.frequency_hz - f2.frequency_hz);
        let nl_m = (f1.frequency_hz * pr1 + f2.frequency_hz * pr2) / (f1.frequency_hz + f2.frequency_hz);
        Some(wl_cyc - nl_m / lambda_w)
    };
    let n0 = nwl(p0, c0)?;
    let n1 = nwl(p1, c1)?;
    let mean = if mw_prev.is_empty() {
        n0
    } else {
        mw_prev.iter().sum::<f64>() / mw_prev.len() as f64
    };
    let mw_jump = n1 - mean;
    let mut mw_candidates: Vec<DetectorCandidate> = Vec::new();
    let mw_ints = integer_offsets(mw_jump, params.search_radius_cycles, params.mw_threshold_cycles);
    let mw_zero = mw_ints.iter().any(|(n, _)| *n == 0);
    for (n, resid) in &mw_ints {
        if *n != 0 {
            // 宽巷周跳 k_w = k1 - k2，归属到一个组合条目（band 用 b1 标记宽巷约束）。
            mw_candidates.push(DetectorCandidate { detector: DetectorKind::MelbourneWubbena, band: b1, offset_cycles: *n, residual_cycles: *resid });
        }
    }
    let mw_status = if mw_prev.len() + 1 < params.mw_min_history {
        DetectorVerdict::InsufficientData
    } else if mw_zero {
        DetectorVerdict::NoSlip
    } else if !mw_candidates.is_empty() {
        DetectorVerdict::Candidate
    } else {
        DetectorVerdict::InsufficientData
    };

    Some(ComboResidual {
        id: joined_id("cmb", &[&imp.id, &epoch.to_string(), sat, &params.fingerprint()]),
        epoch_index: epoch,
        satellite: sat.to_string(),
        gf_residual_cycles: Some(gf_jump),
        gf_candidates,
        gf_pairs,
        gf_status,
        mw_residual_wl_cycles: Some(mw_jump),
        mw_candidates,
        mw_status,
        source_import: imp.id.clone(),
        algo_version: params.algo_version.clone(),
        param_fingerprint: params.fingerprint(),
    })
}

/// 一个卫星边界上的全部频带 TD 候选（按 band 收集）。
fn td_offsets_by_band(
    transitions: &[BandTransition],
    epoch: usize,
    sat: &str,
) -> BTreeMap<u8, Vec<(i64, f64)>> {
    let mut out: BTreeMap<u8, Vec<(i64, f64)>> = BTreeMap::new();
    for t in transitions.iter().filter(|t| t.epoch_index == epoch && t.satellite == sat) {
        let mut v: Vec<(i64, f64)> = t.candidates.iter().map(|c| (c.offset_cycles, c.residual_cycles)).collect();
        v.sort_by_key(|(n, _)| *n);
        out.insert(t.band, v);
    }
    out
}

/// 零假设（无周跳）是否被 TD 允许（残差在阈值内或数据未知）。
fn td_allows_zero(transitions: &[BandTransition], epoch: usize, sat: &str) -> bool {
    let mut any = false;
    for t in transitions.iter().filter(|t| t.epoch_index == epoch && t.satellite == sat) {
        any = true;
        if t.status == DetectorVerdict::Candidate {
            return false;
        }
    }
    any
}

#[derive(Clone)]
struct Hyp {
    offsets: BTreeMap<u8, i64>,
    score: f64,
    supporters: BTreeSet<String>,
    per_detector: Vec<DetectorCandidate>,
    note: Option<String>,
}

impl Hyp {
    fn key(&self) -> Vec<(u8, i64)> {
        self.offsets.iter().map(|(b, n)| (*b, *n)).collect()
    }
    fn is_zero(&self) -> bool {
        self.offsets.values().all(|n| *n == 0)
    }
}

/// 双频融合：TD 笛卡尔积 ∩ GF 线性约束 ∩ MW 宽巷约束；不以任一检测器当真值，保留全部合法假设。
fn fuse_dual_band(
    b1: u8,
    b2: u8,
    td: &BTreeMap<u8, Vec<(i64, f64)>>,
    combo: Option<&ComboResidual>,
    params: &DetectorParams,
) -> (Vec<Hyp>, bool) {
    // TD 每频带候选（含 0 假设：残差允许 0 时把 0 也纳入）。
    let td_list = |band: u8| -> Vec<(i64, f64)> {
        match td.get(&band) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => vec![(0, 0.0)],
        }
    };
    let l1 = td_list(b1);
    let l2 = td_list(b2);

    // GF 合法 (k1,k2) 对（band/offset 成对出现）。
    let gf_pairs: BTreeSet<(i64, i64)> = match combo {
        Some(c) if c.gf_status == DetectorVerdict::Candidate => c.gf_pairs.iter().map(|(a, b2, _)| (*a, *b2)).collect(),
        _ => BTreeSet::new(),
    };

    // MW 合法宽巷差 k1-k2（取候选 offset；NoSlip 时仅允许 0）。
    let mw_diffs: Option<BTreeSet<i64>> = combo.map(|c| match c.mw_status {
        DetectorVerdict::Candidate => c.mw_candidates.iter().map(|x| x.offset_cycles).collect(),
        DetectorVerdict::NoSlip => {
            let mut s = BTreeSet::new();
            s.insert(0);
            s
        }
        _ => BTreeSet::new(), // 数据不足：不加约束
    });

    // GF NoSlip 不强制（GF 可能漏检）；仅当 GF 明确 Candidate 时做交集约束。
    let gf_active = combo.map(|c| c.gf_status == DetectorVerdict::Candidate).unwrap_or(false);
    let mw_active = combo.map(|c| c.mw_status == DetectorVerdict::Candidate || c.mw_status == DetectorVerdict::NoSlip).unwrap_or(false);

    let mut hyps: Vec<Hyp> = Vec::new();
    for (n1, r1) in &l1 {
        for (n2, r2) in &l2 {
            if gf_active && !gf_pairs.contains(&(*n1, *n2)) {
                continue;
            }
            if mw_active {
                if let Some(diffs) = &mw_diffs {
                    if !diffs.contains(&(*n1 - *n2)) {
                        continue;
                    }
                }
            }
            let mut h = Hyp {
                offsets: BTreeMap::from([(b1, *n1), (b2, *n2)]),
                score: 0.0,
                supporters: BTreeSet::new(),
                per_detector: Vec::new(),
                note: None,
            };
            if *n1 != 0 || *n2 != 0 {
                h.supporters.insert("TD".into());
                h.score += 2.0 - r1.abs() - r2.abs();
                h.per_detector.push(DetectorCandidate { detector: DetectorKind::TimeDifference, band: b1, offset_cycles: *n1, residual_cycles: *r1 });
                h.per_detector.push(DetectorCandidate { detector: DetectorKind::TimeDifference, band: b2, offset_cycles: *n2, residual_cycles: *r2 });
            }
            if let Some(c) = combo {
                if c.gf_status == DetectorVerdict::Candidate && gf_pairs.contains(&(*n1, *n2)) {
                    h.supporters.insert("GF".into());
                    h.score += 1.0;
                }
                if c.mw_status == DetectorVerdict::Candidate {
                    if c.mw_candidates.iter().any(|x| x.offset_cycles == *n1 - *n2) {
                        h.supporters.insert("MW".into());
                        h.score += 1.5;
                    }
                }
            }
            hyps.push(h);
        }
    }
    // 若约束交集为空（检测器严重冲突），退回纯 TD 候选并记录冲突。
    let conflict = hyps.iter().all(|h| h.is_zero())
        && (gf_active || mw_active)
        && (l1.iter().any(|(n, _)| *n != 0) || l2.iter().any(|(n, _)| *n != 0));
    if conflict {
        hyps.clear();
        for (n1, r1) in &l1 {
            for (n2, r2) in &l2 {
                if *n1 == 0 && *n2 == 0 {
                    continue;
                }
                hyps.push(Hyp {
                    offsets: BTreeMap::from([(b1, *n1), (b2, *n2)]),
                    score: 2.0 - r1.abs() - r2.abs(),
                    supporters: BTreeSet::from(["TD".to_string()]),
                    per_detector: vec![
                        DetectorCandidate { detector: DetectorKind::TimeDifference, band: b1, offset_cycles: *n1, residual_cycles: *r1 },
                        DetectorCandidate { detector: DetectorKind::TimeDifference, band: b2, offset_cycles: *n2, residual_cycles: *r2 },
                    ],
                    note: Some("detectors disagree; no single hypothesis satisfies TD and GF/MW".into()),
                });
            }
        }
    }
    let _ = params;
    (hyps, conflict)
}

fn build_groups(
    imp: &RawImport,
    params: &DetectorParams,
    transitions: &[BandTransition],
    combos: &[ComboResidual],
) -> Vec<CandidateGroup> {
    let n_ep = imp.epoch_times.len();
    let sats: BTreeSet<String> = {
        let mut s = BTreeSet::new();
        for em in &imp.observations {
            for sat in em.keys() {
                s.insert(sat.clone());
            }
        }
        s
    };
    let mut groups = Vec::new();

    for epoch in 1..n_ep {
        // --- 整机钟跳共识：跨卫星、跨频带的 TD 主候选是否一致（L1 周） ---
        // 收集每个信号上"最近整数、且显著"的跳变；一致比例达到阈值即判定为整机钟跳。
        let mut signals: Vec<(String, u8, f64)> = Vec::new();
        for t in transitions.iter().filter(|t| t.epoch_index == epoch) {
            if let Some(r) = t.td_residual_cycles {
                if r.abs() > params.td_threshold_cycles {
                    signals.push((t.satellite.clone(), t.band, nearest_int(r) as f64));
                }
            }
        }
        let explicit_clock = imp.clock_events.iter().any(|c| c.between_epoch == epoch);
        // 以频率编号最小的频带为公共基准，统计该频带上同一整数跳变出现的卫星数。
        let base_band = *imp.freq_table.keys().next().unwrap_or(&1);
        let mut consensus: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for (sat, band, n) in &signals {
            if *band == base_band {
                consensus.entry(*n as i64).or_default().push(sat.clone());
            }
        }
        let clock_jump = explicit_clock
            || consensus
                .iter()
                .any(|(_, sats_v)| sats_v.len() >= params.clock_consensus_min_signals);
        let (clock_n, clock_sats): (Option<i64>, BTreeSet<String>) = if clock_jump {
            let best = consensus
                .iter()
                .max_by_key(|(_, v)| v.len())
                .map(|(n, v)| (*n, v.iter().cloned().collect::<BTreeSet<_>>()));
            match best {
                Some((n, v)) if v.len() >= params.clock_consensus_min_signals => (Some(n), v),
                _ => {
                    // 显式钟事件但共识不足：仍视作整机钟，卫星集合为空。
                    (None, BTreeSet::new())
                }
            }
        } else {
            (None, BTreeSet::new())
        };

        for sat in &sats {
            let sat_bands: BTreeSet<u8> = imp
                .observations
                .iter()
                .flat_map(|em| em.get(sat).into_iter().flat_map(|m| m.keys().copied().collect::<Vec<_>>()))
                .collect();
            if sat_bands.is_empty() {
                continue;
            }
            let id = joined_id("grp", &[&imp.id, &epoch.to_string(), sat, &params.fingerprint()]);
            let td = td_offsets_by_band(transitions, epoch, sat);
            let combo = combos.iter().find(|c| c.epoch_index == epoch && c.satellite == *sat);
            let lli_set = transitions
                .iter()
                .any(|t| t.epoch_index == epoch && t.satellite == *sat && t.lli_flag.unwrap_or(0) != 0);

            // 整机钟跳：共识卫星上的该组归类为钟跳（不是单星周跳），单星周跳仍保留。
            let in_clock_set = clock_sats.contains(sat);
            if clock_jump && in_clock_set {
                groups.push(CandidateGroup {
                    id,
                    epoch_index: epoch,
                    satellite: sat.clone(),
                    status: GroupStatus::ClockJump,
                    hypotheses: vec![],
                    reject_reasons: vec!["common-mode jump across satellites: receiver clock, not a cycle slip".into()],
                    uncertainty: None,
                    clock_consensus_cycles: clock_n.map(|n| n as f64),
                    source_import: imp.id.clone(),
                    algo_version: params.algo_version.clone(),
                    param_fingerprint: params.fingerprint(),
                    decided_by: None,
                });
                continue;
            }

            // 融合候选假设。
            let (mut hyps_raw, conflict) = match dual_bands(&sat_bands) {
                Some((b1, b2))
                    if imp.observations[epoch].get(sat).map(|m| m.contains_key(&b1) && m.contains_key(&b2)).unwrap_or(false) =>
                {
                    fuse_dual_band(b1, b2, &td, combo, params)
                }
                _ => {
                    // 单频/频率缺失：只在存在的频带上枚举 TD 候选。
                    let band = *sat_bands.iter().last().unwrap();
                    let list = td.get(&band).cloned().unwrap_or_default();
                    let mut vv = Vec::new();
                    for (n, r) in list {
                        vv.push(Hyp {
                            offsets: BTreeMap::from([(band, n)]),
                            score: 2.0 - r.abs(),
                            supporters: BTreeSet::from(["TD".to_string()]),
                            per_detector: vec![DetectorCandidate { detector: DetectorKind::TimeDifference, band, offset_cycles: n, residual_cycles: r }],
                            note: Some("single-band / frequency missing: GF and MW cannot decide, all legal TD candidates kept".into()),
                        });
                    }
                    (vv, false)
                }
            };

            // LLI 旁证：加入支持者，但不单独作为真值。
            if lli_set {
                for h in hyps_raw.iter_mut() {
                    h.supporters.insert("LLI".into());
                    h.score += 0.25;
                }
            }

            let nonzero: Vec<Hyp> = hyps_raw.into_iter().filter(|h| !h.is_zero()).collect();
            let td_has_candidate = td.values().any(|v| !v.is_empty());
            let combo_candidate = combo
                .map(|c| c.gf_status == DetectorVerdict::Candidate || c.mw_status == DetectorVerdict::Candidate)
                .unwrap_or(false);

            let mut reject_reasons = Vec::new();
            if conflict {
                reject_reasons.push("GF/MW/TD disagree; candidates retained, none treated as truth".into());
            }

            let status = if nonzero.is_empty() {
                if lli_set && params.lli_requires_corroboration && !td_has_candidate && !combo_candidate {
                    GroupStatus::LliFalseAlarm
                } else if lli_set {
                    GroupStatus::Pending
                } else if clock_jump && !in_clock_set {
                    // 该卫星不在共识集合：可能是单星周跳，但此处无候选 -> 干净
                    GroupStatus::Clean
                } else {
                    GroupStatus::Clean
                }
            } else {
                // 多解（多个等分/并列整周偏移）：保留全部合法候选 -> Ambiguous/Pending。
                if nonzero.len() > 1 {
                    GroupStatus::Ambiguous
                } else {
                    GroupStatus::Pending
                }
            };
            if status == GroupStatus::LliFalseAlarm {
                reject_reasons.push("LLI flag set but TD/GF/MW show no corroborating jump: treated as false alarm".into());
            }

            let mut seen = BTreeSet::new();
            let mut hypotheses = Vec::new();
            for h in nonzero {
                let key = h.key();
                if !seen.insert(key.clone()) {
                    continue;
                }
                let hid = joined_id(
                    "hyp",
                    &[
                        &imp.id,
                        &epoch.to_string(),
                        sat,
                        &params.fingerprint(),
                        &serde_json::to_string(&key).unwrap(),
                    ],
                );
                let mut supporters: Vec<String> = h.supporters.into_iter().collect();
                supporters.sort();
                hypotheses.push(RepairHypothesis {
                    id: hid,
                    offsets: h.offsets,
                    score: (h.score * 1000.0).round() / 1000.0,
                    supporters,
                    per_detector: h.per_detector,
                    note: h.note,
                });
            }
            hypotheses.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

            let uncertainty = if status == GroupStatus::Ambiguous {
                Some("multiple equally-legal integer offsets; not uniquely determined".into())
            } else if conflict {
                Some("detectors disagree".into())
            } else {
                None
            };

            groups.push(CandidateGroup {
                id,
                epoch_index: epoch,
                satellite: sat.clone(),
                status,
                hypotheses,
                reject_reasons,
                uncertainty,
                clock_consensus_cycles: clock_n.map(|n| n as f64).filter(|_| in_clock_set),
                source_import: imp.id.clone(),
                algo_version: params.algo_version.clone(),
                param_fingerprint: params.fingerprint(),
                decided_by: None,
            });
        }
    }
    groups.sort_by(|a, b| a.epoch_index.cmp(&b.epoch_index).then(a.satellite.cmp(&b.satellite)));
    groups
}

/// 构建弧段：站-星-频带连续相位段。间断原因显式记录（长间隔/频率缺失/已采纳周跳）。
fn build_arcs(
    imp: &RawImport,
    params: &DetectorParams,
    accepted: &BTreeMap<(usize, String), BTreeMap<u8, i64>>,
) -> Vec<Arc> {
    let n_ep = imp.epoch_times.len();
    let sats: BTreeSet<String> = imp.observations.iter().flat_map(|em| em.keys().cloned()).collect();
    let mut arcs = Vec::new();

    for sat in &sats {
        let bands: BTreeSet<u8> = imp
            .observations
            .iter()
            .flat_map(|em| em.get(sat).into_iter().flat_map(|m| m.keys().copied()).collect::<Vec<_>>())
            .collect();
        for band in &bands {
            let present: Vec<usize> = (0..n_ep)
                .filter(|e| imp.observations[*e].get(sat).and_then(|m| m.get(band)).is_some())
                .collect();
            let mut seg_start = 0usize;
            for w in 1..present.len() {
                let prev_e = present[w - 1];
                let e = present[w];
                let mut reasons: Vec<String> = Vec::new();
                let dt = imp.dt_seconds[e].unwrap_or(0.0);
                if dt > params.max_gap_seconds {
                    reasons.push(format!("gap {:.0}s at epoch {} exceeds max-gap (uneven epochs)", dt, e));
                }
                if e - prev_e > 1 {
                    reasons.push(format!("frequency {} missing at epochs {:?}", band, (prev_e + 1..e).collect::<Vec<_>>()));
                }
                if let Some(off) = accepted.get(&(e, sat.clone())) {
                    if off.get(band).copied().unwrap_or(0) != 0 {
                        reasons.push(format!("accepted cycle-slip repair on band {} at epoch {}", band, e));
                    }
                }
                if !reasons.is_empty() {
                    arcs.push(Arc {
                        id: joined_id("arc", &[&imp.id, sat, &band.to_string(), &present[seg_start].to_string(), &prev_e.to_string()]),
                        satellite: sat.clone(),
                        band: *band,
                        start_epoch: present[seg_start],
                        end_epoch: prev_e,
                        start_time: imp.epoch_times[present[seg_start]].clone(),
                        end_time: imp.epoch_times[prev_e].clone(),
                        split_reasons: reasons,
                        source_import: imp.id.clone(),
                    });
                    seg_start = w;
                }
            }
            if !present.is_empty() {
                let a = present[seg_start];
                let z = *present.last().unwrap();
                arcs.push(Arc {
                    id: joined_id("arc", &[&imp.id, sat, &band.to_string(), &a.to_string(), &z.to_string()]),
                    satellite: sat.clone(),
                    band: *band,
                    start_epoch: a,
                    end_epoch: z,
                    start_time: imp.epoch_times[a].clone(),
                    end_time: imp.epoch_times[z].clone(),
                    split_reasons: vec![],
                    source_import: imp.id.clone(),
                });
            }
        }
    }
    arcs.sort_by(|a, b| {
        a.satellite.cmp(&b.satellite).then(a.band.cmp(&b.band)).then(a.start_epoch.cmp(&b.start_epoch))
    });
    arcs
}

/// 完整、确定性地运行检测器。
/// `accepted` 为当前已采纳偏移（仅用于弧段边界；不影响残差）。
pub fn run_detector(
    imp: &RawImport,
    params: &DetectorParams,
    accepted: &BTreeMap<(usize, String), BTreeMap<u8, i64>>,
) -> DetectorRun {
    // 规范化（只在派生层做单位归一；原始层保持原样）
    let normalized: Vec<BTreeMap<String, BTreeMap<u8, NormObs>>> = imp
        .observations
        .iter()
        .map(|em| em.iter().map(|(sat, fm)| (sat.clone(), fm.iter().map(|(b, o)| (*b, norm_obs(o))).collect())).collect())
        .collect();

    // 频带集合
    let mut sat_band: BTreeMap<String, BTreeSet<u8>> = BTreeMap::new();
    for em in &normalized {
        for (sat, fm) in em {
            sat_band.entry(sat.clone()).or_default().extend(fm.keys().copied());
        }
    }

    let mut transitions: Vec<BandTransition> = Vec::new();
    let mut combos: Vec<ComboResidual> = Vec::new();
    let n_ep = imp.epoch_times.len();

    for (sat, bands) in &sat_band {
        // MW 滑动历史（每卫星一份宽巷模糊度序列）
        let mut mw_hist: Vec<f64> = Vec::new();
        for epoch in 1..n_ep {
            for band in bands {
                if let Some(t) = build_transition(imp, params, epoch, sat, *band, &normalized) {
                    transitions.push(t);
                }
            }
            if let Some((b1, b2)) = dual_bands(bands) {
                let f1 = &imp.freq_table[&b1];
                let f2 = &imp.freq_table[&b2];
                if let Some(c) = build_combo(params, epoch, sat, b1, b2, f1, f2, &normalized, imp, &mw_hist) {
                    if let Some(n) = c.mw_residual_wl_cycles {
                        // 把当前宽巷模糊度（均值前）入历史：用 mean + jump 还原 n1
                        mw_hist.push(if mw_hist.is_empty() { n } else {
                            mw_hist.iter().sum::<f64>() / mw_hist.len() as f64 + n
                        });
                    }
                    combos.push(c);
                }
            }
        }
    }
    transitions.sort_by(|a, b| a.epoch_index.cmp(&b.epoch_index).then(a.satellite.cmp(&b.satellite)).then(a.band.cmp(&b.band)));
    combos.sort_by(|a, b| a.epoch_index.cmp(&b.epoch_index).then(a.satellite.cmp(&b.satellite)));

    let mut groups = build_groups(imp, params, &transitions, &combos);
    // 已采纳/否定决定映射到组状态。
    for g in groups.iter_mut() {
        if let Some(off) = accepted.get(&(g.epoch_index, g.satellite.clone())) {
            if off.is_empty() {
                g.status = GroupStatus::Rejected;
            } else {
                g.status = GroupStatus::Accepted;
            }
        }
    }

    let arcs = build_arcs(imp, params, accepted);

    let input_fragments = imp
        .diagnostics
        .iter()
        .cloned()
        .chain(imp.clock_events.iter().map(|c| format!("clock event between epoch {}: {} {}", c.between_epoch, c.jump.value, c.jump.unit)))
        .collect();

    DetectorRun {
        id: joined_id("run", &[&imp.id, &params.fingerprint()]),
        source_import: imp.id.clone(),
        algo_version: params.algo_version.clone(),
        param_fingerprint: params.fingerprint(),
        normalized,
        transitions,
        combos,
        groups,
        arcs,
        input_fragments,
    }
}
