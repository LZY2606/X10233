//! 领域计算引擎（核心状态机）。
//!
//! 三个检测器各自产生候选：电离层组合（GF）、Melbourne-Wubbena（MW）、时间差分（TD）。
//! 任何一个检测器都不是真值；候选是一等数据，被采纳/被拒绝/未知状态都保留。
//! 整机钟跳与单星周跳分开；修复只写偏移层（OffsetLayer），原始层永不修改。
//! 局部输入变化只让依赖它的派生结果过期：身份按内容寻址，未变化对象身份不变。

use crate::hash::stable_id;
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ALGO_VERSION: &str = "gsb-algo-1.0.0";

/// 单条规范化观测（派生层）。不可转换的值带状态保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormObs {
    pub station: String,
    pub sat: String,
    pub epoch_index: usize,
    pub iso: String,
    pub t: f64,
    pub band: String,
    pub wavelength_m: f64,
    pub p_m: Option<f64>,
    pub l_cyc: Option<f64>,
    pub d_hz: Option<f64>,
    pub snr_dbhz: Option<f64>,
    pub lli: Option<u8>,
    pub missing: bool,
    pub diag: Vec<String>,
    /// 直接来源：槽位逻辑键 + 原始记录稳定 id
    pub source_slot: String,
    pub source_record_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryReason {
    Start,
    End,
    Gap,            // 不等间隔历元
    MissingBand,    // 频率缺失
    Lli,            // LLI 置位
    ClockJump,      // 整机钟跳
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryStatus {
    Open,
    Candidate,
    ClockBoundary,
    FalseLli,
    Inconclusive,
    Error,
}

/// 弧段边界（站-星-频，位于第 idx 个规范化观测之前）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub epoch_index: usize,
    pub iso: String,
    pub dt_s: f64,
    pub reasons: Vec<BoundaryReason>,
    pub status: BoundaryStatus,
    /// 三个检测器残差（周或米），无组合时为 None
    pub gf_jump_m: Option<f64>,
    pub mw_jump_cyc: Option<f64>,
    pub td_res_cyc: Option<f64>,
    /// 稳健噪声估计（与模型噪声取大者后用于打分）
    pub gf_sigma_m: f64,
    pub wl_sigma_cyc: f64,
    pub td_sigma_cyc: f64,
    /// 各检测器给出的整周偏移候选（周，载波）
    pub candidates: Vec<Candidate>,
    pub diag: Vec<String>,
    pub source_records: Vec<String>,
    pub algo_version: String,
    pub param_fingerprint: String,
}

/// 周跳候选（一等数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// 稳定身份：与所在边界的内容 + 整数偏移绑定
    pub id: String,
    pub offset_cycles: i32,
    /// 0..=1 综合得分
    pub score: f64,
    /// 哪些检测器支持该整数偏移
    pub supporting: Vec<String>,
    /// 每个检测器与该整数的残差（周/米，归一到各自 sigma 的 n_sigma）
    pub detector_nsigma: BTreeMap<String, f64>,
    /// 判定状态：active（唯一）/ ambiguous（多个等分）/ rejected / error / unknown
    pub state: CandidateState,
    pub reject_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Active,
    Ambiguous,
    Rejected,
    Error,
    Unknown,
}

/// 弧段（连续载波相位段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub start_epoch: usize,
    pub end_epoch: usize,
    pub start_iso: String,
    pub end_iso: String,
    pub n_obs: usize,
    pub boundaries: Vec<String>,
    pub source_records: Vec<String>,
}

/// 整机钟跳（共模）事件的派生确认，独立于单星周跳。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockDetection {
    pub id: String,
    pub station: String,
    pub epoch_index: usize,
    pub iso: String,
    pub median_jump_m: f64,
    pub median_jump_ns: f64,
    pub common_ratio: f64,
    pub n_sats: usize,
    pub n_agree: usize,
    pub source_clock_event: Option<String>,
    pub source_records: Vec<String>,
}

/// 修复偏移层。修复只写这一层，绝不回写原始观测。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetLayer {
    /// 被修复的候选/边界稳定 id
    pub target_boundary_id: String,
    pub candidate_id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub epoch_index: usize,
    /// 应用到载波的整周偏移（周）
    pub offset_cycles: i32,
    /// 单位
    pub unit: String,
}

/// 一次完整计算（某个上下文/result_set 下的当前结果）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Computation {
    pub context_hash: String,
    pub algo_version: String,
    pub param_fingerprint: String,
    pub norm: Vec<NormObs>,
    pub boundaries: Vec<Boundary>,
    pub arcs: Vec<Arc>,
    pub clock: Vec<ClockDetection>,
    /// 规范化层诊断（无法转换等），按记录聚合
    pub norm_diag: Vec<NormDiag>,
    /// 用到的原始记录 id（依赖集合，用于局部过期）
    pub source_record_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormDiag {
    pub source_record_id: String,
    pub band: String,
    pub diag: String,
}

/// 参数指纹：算法版本 + 参数确定性序列化。
pub fn param_fingerprint(params: &Params) -> String {
    let canon = format!(
        "{}|dt={:.6}|gf={:.6}|wl={:.6}|td={:.6}|r={}|ns={:.6}|cr={:.6}|gapf={:.6}",
        params.algo_version,
        params.nominal_dt_s,
        params.gf_sigma_m,
        params.wl_sigma_cycle,
        params.td_sigma_cycle,
        params.search_radius_cycle,
        params.n_sigma,
        params.clock_common_ratio,
        params.gap_factor
    );
    stable_id("params", &canon)
}

fn mad_scale(xs: &[f64]) -> f64 {
    if xs.len() < 3 {
        return 0.0;
    }
    let mut v: Vec<f64> = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    let mut dev: Vec<f64> = v.iter().map(|x| (x - med).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = dev[dev.len() / 2];
    1.4826 * mad
}

fn parse_lli(s: &str) -> Option<u8> {
    s.trim().parse::<u8>().ok()
}

/// 规范化一份已折叠到某上下文的输入。records 已按提交顺序（父在前）排列。
/// 同一槽位只取最后版本；DROP 版本删除槽位。
pub fn normalize(freqs: &[FrequencyDef], folded: &[FoldedRecord]) -> (Vec<NormObs>, Vec<NormDiag>) {
    let freq_by_band: BTreeMap<&str, &FrequencyDef> = freqs.iter().map(|f| (f.band.as_str(), f)).collect();
    let mut norm = Vec::new();
    let mut diags = Vec::new();

    // 全局历元顺序：按时间排序后给 epoch_index（展示用稳定顺序仍由 iso 决定）
    let mut times: Vec<(f64, String)> =
        folded.iter().map(|r| (r.obs.time.secs, r.obs.time.iso.clone())).collect();
    times.sort_by(|a, b| a.partial_cmp(&b.0).unwrap().then_with(|| a.1.cmp(&b.1)));
    times.dedup();
    let mut epoch_of: BTreeMap<String, usize> = BTreeMap::new();
    for (i, (_, iso)) in times.iter().enumerate() {
        epoch_of.insert(iso.clone(), i);
    }

    for r in folded {
        let idx = *epoch_of.get(&r.obs.time.iso).unwrap_or(&r.obs.epoch_index);
        for b in &r.obs.bands {
            let mut diag = Vec::new();
            let wl = freq_by_band.get(b.band.as_str()).map(|f| f.wavelength_m).unwrap_or(0.0);
            if wl <= 0.0 {
                diag.push(format!("频带 {} 无波长定义，组合不可计算", b.band));
            }
            let p_m = b.pseudorange.as_ref().and_then(|q| match q.status {
                ValueStatus::Ok => q.value,
                ValueStatus::Unparseable { ref diag: d } => {
                    diags.push(NormDiag { source_record_id: r.record_id.clone(), band: b.band.clone(), diag: d.clone() });
                    diag.push(d.clone());
                    None
                }
                ValueStatus::Missing => None,
            });
            let l_cyc = b.carrier.as_ref().and_then(|q| match q.status {
                ValueStatus::Ok => q.value,
                ValueStatus::Unparseable { ref diag: d } => {
                    diags.push(NormDiag { source_record_id: r.record_id.clone(), band: b.band.clone(), diag: d.clone() });
                    diag.push(d.clone());
                    None
                }
                ValueStatus::Missing => None,
            });
            let d_hz = b.doppler.as_ref().and_then(|q| q.value);
            let snr = b.snr.as_ref().and_then(|q| q.value);
            if let Some(q) = &b.carrier {
                if let ValueStatus::Unparseable { diag: d } = &q.status {
                    diag.push(format!("载波无法转换，保留原文 `{}`: {d}", q.raw));
                }
            }
            norm.push(NormObs {
                station: r.obs.station.clone(),
                sat: r.obs.sat.clone(),
                epoch_index: idx,
                iso: r.obs.time.iso.clone(),
                t: r.obs.time.secs,
                band: b.band.clone(),
                wavelength_m: wl,
                p_m: p_m,
                l_cyc,
                d_hz,
                snr_dbhz: snr,
                lli: parse_lli(&b.lli_raw),
                missing: b.missing,
                diag,
                source_slot: r.slot_key.clone(),
                source_record_id: r.record_id.clone(),
            });
        }
    }
    norm.sort_by(|a, b| {
        a.station
            .cmp(&b.station)
            .then_with(|| a.sat.cmp(&b.sat))
            .then_with(|| a.band.cmp(&b.band))
            .then_with(|| a.t.partial_cmp(&b.t).unwrap())
    });
    (norm, diags)
}

/// 折叠后的当前版本记录。
#[derive(Debug, Clone)]
pub struct FoldedRecord {
    pub slot_key: String,
    pub record_id: String,
    pub commit_id: String,
    pub obs: EpochObs,
}

/// 共模钟跳检测：在每对相邻历元，统计多数卫星同符号同方向的 P/L 跳变。
/// 返回被判定为整机钟跳的 (epoch_iso, epoch_index, station, med_m, ratio)。
pub fn detect_clock(norm: &[NormObs], records: &[FoldedRecord], params: &Params) -> Vec<ClockDetection> {
    let mut out = Vec::new();
    let stations: std::collections::BTreeSet<String> = norm.iter().map(|n| n.station.clone()).collect();
    for station in stations {
        // 历元序列
        let mut epochs: Vec<(usize, String, f64)> = norm
            .iter()
            .filter(|n| n.station == station)
            .map(|n| (n.epoch_index, n.iso.clone(), n.t))
            .collect();
        epochs.sort();
        epochs.dedup_by(|a, b| a.0 == b.0);
        for w in epochs.windows(2) {
            let (i0, iso0, _) = &w[0];
            let (i1, iso1, _) = &w[1];
            // 每颗卫星在该两历元的 P 跳变（取 L1）
            let mut jumps: Vec<(String, f64)> = Vec::new();
            let sats: std::collections::BTreeSet<String> =
                norm.iter().filter(|n| n.station == station && (n.epoch_index == *i0 || n.epoch_index == *i1)).map(|n| n.sat.clone()).collect();
            for sat in sats {
                let get = |ei: usize| -> Option<&NormObs> {
                    norm.iter().find(|n| n.station == station && n.sat == sat && n.band == "L1" && n.epoch_index == ei)
                };
                if let (Some(a), Some(b)) = (get(*i0), get(*i1)) {
                    if let (Some(pa), Some(pb)) = (a.p_m, b.p_m) {
                        let j = pb - pa;
                        if j.abs() > 50.0 {
                            jumps.push((sat.clone(), j));
                        }
                    }
                }
            }
            if jumps.len() < 2 {
                continue;
            }
            let mut vals: Vec<f64> = jumps.iter().map(|(_, v)| *v).collect();
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = vals[vals.len() / 2];
            if med.abs() < 50.0 {
                continue;
            }
            let tol = 30.0;
            let agree = jumps.iter().filter(|(_, v)| (v - med).abs() < tol && v.signum() == med.signum()).count();
            let ratio = agree as f64 / jumps.len() as f64;
            if ratio >= params.clock_common_ratio {
                // 找到该历元原始 CLOCK 事件（若有）
                let src_clock = records.iter().find_map(|r| {
                    if r.obs.station == station && r.obs.time.iso == *iso1 {
                        r.obs.clock_event.as_ref().map(|_| r.record_id.clone())
                    } else {
                        None
                    }
                });
                let source_records: Vec<String> = records
                    .iter()
                    .filter(|r| r.obs.station == station && (r.obs.time.iso == *iso0 || r.obs.time.iso == *iso1))
                    .map(|r| r.record_id.clone())
                    .collect();
                let canon = format!("clock|{station}|{iso1}|{}|{:.3}|{:.3}", med, ratio, agree);
                out.push(ClockDetection {
                    id: stable_id("clk", &canon),
                    station,
                    epoch_index: *i1,
                    iso: iso1.clone(),
                    median_jump_m: med,
                    median_jump_ns: med / 299_792_458.0 * 1e9,
                    common_ratio: ratio,
                    n_sats: jumps.len(),
                    n_agree: agree,
                    source_clock_event: src_clock,
                    source_records,
                });
            }
        }
    }
    out.sort_by(|a, b| a.station.cmp(&b.station).then_with(|| a.epoch_index.cmp(&b.epoch_index)));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// 组合：几何无关（电离层）GF = L1*lambda1 - L2*lambda2 （米）
fn gf_series(n1: &NormObs, n2: &NormObs) -> Option<f64> {
    match (n1.l_cyc, n2.l_cyc) {
        (Some(l1), Some(l2)) => Some(l1 * n1.wavelength_m - l2 * n2.wavelength_m),
        _ => None,
    }
}

/// Melbourne-Wubbena 宽巷模糊度（周）。
/// Nw = [ (L1-L2) - (f1*P1+f2*P2)/(f1+f2)/lambda_w ] ，
/// 这里以周表示：Nw = (L1 - L2) - (f1 P1 + f2 P2)/((f1+f2) lambda_w)。
fn mw_nw(n1: &NormObs, n2: &NormObs, f1: f64, f2: f64) -> Option<f64> {
    let lw = 299_792_458.0 / (f1 - f2);
    let l = match (n1.l_cyc, n2.l_cyc) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    }?;
    let p = match (n1.p_m, n2.p_m) {
        (Some(p1), Some(p2)) => Some((f1 * p1 + f2 * p2) / (f1 + f2) / lw),
        _ => None,
    }?;
    Some(l - p)
}

/// 时间差分：载波增量减 Doppler 预测（周）。
fn td_residual(prev: &NormObs, cur: &NormObs) -> Option<f64> {
    let dl = (cur.l_cyc? - prev.l_cyc?) as f64;
    let d = cur.d_hz?;
    let dt = cur.t - prev.t;
    if dt <= 0.0 {
        return None;
    }
    Some(dl - d * dt)
}

struct Series {
    key: (String, String, String),
    obs: Vec<NormObs>,
}

fn group_series(norm: &[NormObs]) -> Vec<Series> {
    let mut map: BTreeMap<(String, String, String), Vec<NormObs>> = BTreeMap::new();
    for n in norm {
        map.entry((n.station.clone(), n.sat.clone(), n.band.clone())).or_default().push(n.clone());
    }
    map.into_iter().map(|(key, mut obs)| {
        obs.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
        Series { key, obs }
    }).collect()
}

fn pair_at<'a>(norm: &'a [NormObs], station: &str, sat: &str, band: &str, ei: usize) -> Option<&'a NormObs> {
    norm.iter().find(|n| n.station == station && n.sat == sat && n.band == band && n.epoch_index == ei)
}

/// 主计算入口。
pub fn compute(freqs: &[FrequencyDef], folded: &[FoldedRecord], params: &Params, context_hash: String) -> Computation {
    let (norm, norm_diag) = normalize(freqs, folded);
    let clock = detect_clock(&norm, folded, params);
    let clock_keys: std::collections::BTreeSet<(String, usize)> =
        clock.iter().map(|c| (c.station.clone(), c.epoch_index)).collect();
    let pf = param_fingerprint(params);

    let mut boundaries: Vec<Boundary> = Vec::new();
    let mut arcs: Vec<Arc> = Vec::new();

    let freq_of = |band: &str| freqs.iter().find(|f| f.band == band).cloned();
    let has_band2 = freqs.iter().any(|f| f.band == "L2");

    for s in group_series(&norm) {
        let (station, sat, band) = s.key.clone();
        let obs = &s.obs;
        if obs.is_empty() {
            continue;
        }
        // 稳健噪声：用该序列非边界处的 TD/GF/MW 一阶差分 MAD
        let td_samples: Vec<f64> = obs.windows(2).filter_map(|w| td_residual(&w[0], &w[1]).map(|x| x.abs())).collect();
        let td_scale = mad_scale(&td_samples);
        let mut gf_samples: Vec<f64> = Vec::new();
        let mut mw_samples: Vec<f64> = Vec::new();
        if has_band2 {
            for w in obs.windows(2) {
                let ei0 = w[0].epoch_index;
                let ei1 = w[1].epoch_index;
                if let (Some(a1), Some(b1), Some(a2), Some(b2)) = (
                    pair_at(&norm, &station, &sat, "L1", ei0),
                    pair_at(&norm, &station, &sat, "L1", ei1),
                    pair_at(&norm, &station, &sat, "L2", ei0),
                    pair_at(&norm, &station, &sat, "L2", ei1),
                ) {
                    if let (Some(g0), Some(g1)) = (gf_series(a1, a2), gf_series(b1, b2)) {
                        gf_samples.push((g1 - g0).abs());
                    }
                    let (f1, f2) = match (freq_of("L1"), freq_of("L2")) {
                        (Some(x), Some(y)) => (x.frequency_hz, y.frequency_hz),
                        _ => (0.0, 0.0),
                    };
                    if let (Some(m0), Some(m1)) = (mw_nw(a1, a2, f1, f2), mw_nw(b1, b2, f1, f2)) {
                        mw_samples.push((m1 - m0).abs());
                    }
                }
            }
        }
        let gf_sigma = params.gf_sigma_m.max(mad_scale(&gf_samples));
        let wl_sigma = params.wl_sigma_cycle.max(mad_scale(&mw_samples));
        let td_sigma = params.td_sigma_cycle.max(td_scale);

        // 弧段切分
        let mut arc_start = 0usize;
        let mut arc_boundary_ids: Vec<String> = Vec::new();
        for k in 0..obs.len() {
            let cur = &obs[k];
            let mut reasons: Vec<BoundaryReason> = Vec::new();
            let mut is_break = false;
            if k == 0 {
                reasons.push(BoundaryReason::Start);
                is_break = true;
            }
            let mut gf_jump = None;
            let mut mw_jump = None;
            let mut td_res = None;
            if k > 0 {
                let prev = &obs[k - 1];
                let dt = cur.t - prev.t;
                // 不等间隔
                if dt > params.nominal_dt_s * params.gap_factor + 1e-9 {
                    reasons.push(BoundaryReason::Gap);
                    is_break = true;
                }
                // 频率缺失：当前缺载波，或上一历元缺
                if cur.missing || cur.l_cyc.is_none() {
                    reasons.push(BoundaryReason::MissingBand);
                    is_break = true;
                }
                // LLI
                if let Some(lli) = cur.lli {
                    if lli & 1 == 1 || lli == 1 || lli == 3 {
                        reasons.push(BoundaryReason::Lli);
                    }
                }
                // 钟跳
                if clock_keys.contains(&(station.clone(), cur.epoch_index)) {
                    reasons.push(BoundaryReason::ClockJump);
                    is_break = true;
                }
                // 残差
                td_res = td_residual(prev, cur);
                if has_band2 {
                    let (f1, f2) = match (freq_of("L1"), freq_of("L2")) {
                        (Some(x), Some(y)) => (x.frequency_hz, y.frequency_hz),
                        _ => (0.0, 0.0),
                    };
                    if let (Some(c1), Some(c2), Some(p1), Some(p2)) = (
                        pair_at(&norm, &station, &sat, "L1", cur.epoch_index),
                        pair_at(&norm, &station, &sat, "L2", cur.epoch_index),
                        pair_at(&norm, &station, &sat, "L1", prev.epoch_index),
                        pair_at(&norm, &station, &sat, "L2", prev.epoch_index),
                    ) {
                        if let (Some(gc), Some(gp)) = (gf_series(c1, c2), gf_series(p1, p2)) {
                            gf_jump = Some(gc - gp);
                        }
                        if let (Some(mc), Some(mp)) = (mw_nw(c1, c2, f1, f2), mw_nw(p1, p2, f1, f2)) {
                            mw_jump = Some(mc - mp);
                        }
                    }
                }
            }

            let source_records: Vec<String> = if k > 0 {
                vec![obs[k - 1].source_record_id.clone(), cur.source_record_id.clone()]
            } else {
                vec![cur.source_record_id.clone()]
            };

            // 候选与状态
            let (candidates, status, mut diag) =
                build_candidates(band, gf_jump, mw_jump, td_res, gf_sigma, wl_sigma, td_sigma, params, &reasons, cur.wavelength_m);

            if reasons.contains(&BoundaryReason::Lli) {
                // LLI 但没有显著候选 => 误报；有候选则为真周跳
                let has_sig = candidates.iter().any(|c| c.state != CandidateState::Rejected && c.offset_cycles != 0);
                if !has_sig {
                    diag.push("LLI 置位但 GF/MW/TD 均无显著整周偏移，判为 LLI 误报".to_string());
                }
            }

            let bcanon = format!(
                "{algo}|{pf}|{station}|{sat}|{band}|{iso}|{dt:.6}|{g:?}|{m:?}|{t:?}|{src}",
                algo = params.algo_version,
                pf = pf,
                iso = cur.iso,
                dt = if k > 0 { cur.t - obs[k - 1].t } else { 0.0 },
                g = gf_jump,
                m = mw_jump,
                t = td_res,
                src = source_records.join(",")
            );
            let bid = stable_id("bnd", &bcanon);

            let mut final_status = status;
            if reasons.contains(&BoundaryReason::ClockJump) {
                final_status = BoundaryStatus::ClockBoundary;
            } else if reasons.contains(&BoundaryReason::Lli)
                && !candidates.iter().any(|c| c.state != CandidateState::Rejected && c.offset_cycles != 0)
            {
                final_status = BoundaryStatus::FalseLli;
            } else if matches!(final_status, BoundaryStatus::Open)
                && candidates.iter().any(|c| matches!(c.state, CandidateState::Error))
            {
                final_status = BoundaryStatus::Inconclusive;
            }

            let b = Boundary {
                id: bid.clone(),
                station: station.clone(),
                sat: sat.clone(),
                band: band.clone(),
                epoch_index: cur.epoch_index,
                iso: cur.iso.clone(),
                dt_s: if k > 0 { cur.t - obs[k - 1].t } else { 0.0 },
                reasons,
                status: final_status,
                gf_jump_m: gf_jump,
                mw_jump_cyc: mw_jump,
                td_res_cyc: td_res,
                gf_sigma_m: gf_sigma,
                wl_sigma_cyc: wl_sigma,
                td_sigma_cyc: td_sigma,
                candidates,
                diag,
                source_records,
                algo_version: params.algo_version.clone(),
                param_fingerprint: pf.clone(),
            };

            if k > 0 && is_break {
                // 收尾上一弧段
                push_arc(&mut arcs, station, sat, band, &obs[arc_start..k], &arc_boundary_ids);
                arc_start = k;
                arc_boundary_ids.clear();
            }
            arc_boundary_ids.push(bid);
            boundaries.push(b);
        }
        // 末点 End
        let last = obs.last().unwrap();
        let end_bid = stable_id(
            "bnd",
            &format!(
                "{algo}|{pf}|{station}|{sat}|{band}|{iso}|end|{src}",
                algo = params.algo_version,
                iso = last.iso,
                src = last.source_record_id
            ),
        );
        arc_boundary_ids.push(end_bid.clone());
        boundaries.push(Boundary {
            id: end_bid,
            station: station.clone(),
            sat: sat.clone(),
            band: band.clone(),
            epoch_index: last.epoch_index,
            iso: last.iso.clone(),
            dt_s: 0.0,
            reasons: vec![BoundaryReason::End],
            status: BoundaryStatus::Open,
            gf_jump_m: None,
            mw_jump_cyc: None,
            td_res_cyc: None,
            gf_sigma_m: gf_sigma,
            wl_sigma_cyc: wl_sigma,
            td_sigma_cyc: td_sigma,
            candidates: vec![],
            diag: vec![],
            source_records: vec![last.source_record_id.clone()],
            algo_version: params.algo_version.clone(),
            param_fingerprint: pf.clone(),
        });
        push_arc(&mut arcs, station, sat, band, &obs[arc_start..], &arc_boundary_ids);
    }

    boundaries.sort_by(|a, b| {
        a.station
            .cmp(&b.station)
            .then_with(|| a.sat.cmp(&b.sat))
            .then_with(|| a.band.cmp(&b.band))
            .then_with(|| a.epoch_index.cmp(&b.epoch_index))
    });
    let source_record_ids = folded.iter().map(|r| r.record_id.clone()).collect();
    Computation {
        context_hash,
        algo_version: params.algo_version.clone(),
        param_fingerprint: pf,
        norm,
        boundaries,
        arcs,
        clock,
        norm_diag,
        source_record_ids,
    }
}

fn push_arc(
    arcs: &mut Vec<Arc>,
    station: &str,
    sat: &str,
    band: &str,
    seg: &[NormObs],
    boundary_ids: &[String],
) {
    if seg.is_empty() {
        return;
    }
    let canon = format!(
        "arc|{station}|{sat}|{band}|{}|{}|{}",
        seg.first().unwrap().iso,
        seg.last().unwrap().iso,
        seg.iter().map(|n| n.source_record_id.clone()).collect::<Vec<_>>().join(",")
    );
    arcs.push(Arc {
        id: stable_id("arc", &canon),
        station: station.to_string(),
        sat: sat.to_string(),
        band: band.to_string(),
        start_epoch: seg.first().unwrap().epoch_index,
        end_epoch: seg.last().unwrap().epoch_index,
        start_iso: seg.first().unwrap().iso.clone(),
        end_iso: seg.last().unwrap().iso.clone(),
        n_obs: seg.len(),
        boundaries: boundary_ids.to_vec(),
        source_records: seg.iter().map(|n| n.source_record_id.clone()).collect(),
    });
}

/// 候选构造。
/// 三个检测器给出的整周解释不同：GF（米）-> 周用波长；MW 直接是宽巷周；TD 直接是周。
/// 不以任一检测器当真值：只有相互印证的整数才成为 active；多个等权 => ambiguous；
/// 单检测器孤立 => rejected（注明原因）；组合缺失 => unknown/error 保留。
#[allow(clippy::too_many_arguments)]
fn build_candidates(
    band: &str,
    gf_jump_m: Option<f64>,
    mw_jump_cyc: Option<f64>,
    td_res_cyc: Option<f64>,
    gf_sigma_m: f64,
    wl_sigma_cyc: f64,
    td_sigma_cyc: f64,
    params: &Params,
    reasons: &[BoundaryReason],
    wavelength_m: f64,
) -> (Vec<Candidate>, BoundaryStatus, Vec<String>) {
    let mut diag = Vec::new();
    let is_clock = reasons.contains(&BoundaryReason::ClockJump);
    let is_gap = reasons.contains(&BoundaryReason::Gap);
    let is_missing = reasons.contains(&BoundaryReason::MissingBand);

    // 各检测器在每个整数偏移上的 |残差| / sigma
    let mut det_n: Vec<(Detector, f64)> = Vec::new(); // 观测到的“周跳量”（周），用于枚举
    let mut observations: BTreeMap<Detector, f64> = BTreeMap::new();

    if let Some(g) = gf_jump_m {
        if wavelength_m > 0.0 {
            // GF 对 L1 跳 k 周 => 米变化 k*lambda1；解释到该 band
            let cycles = if band == "L2" {
                // GF 变化 = k1*lam1 - k2*lam2；单频视角下把 GF 米变化折到本带
                g / wavelength_m
            } else {
                g / wavelength_m
            };
            observations.insert(Detector::GeometryFree, cycles);
        } else {
            diag.push("无波长，GF 残差无法折算为整周".to_string());
        }
    }
    if let Some(mw) = mw_jump_cyc {
        // MW 宽巷周跳：对单频带作近似映射（L1: +nw, L2: 0 的宽巷等价），
        // 这里在 L1 边界把 MW 视作对 L1 整数的独立投票。
        observations.insert(Detector::MelbourneWubbena, mw);
    }
    if let Some(td) = td_res_cyc {
        observations.insert(Detector::TimeDifference, td);
    }

    if observations.is_empty() {
        if is_missing {
            diag.push("频率缺失：无载波/伪距可组成检测器，状态未知（保留弧段断点）".to_string());
            return (Vec::new(), BoundaryStatus::Inconclusive, diag);
        }
        if !is_clock && !is_gap && !reasons.contains(&BoundaryReason::Start) {
            diag.push("三个检测器均无可用组合，无法判定".to_string());
        }
        return (Vec::new(), BoundaryStatus::Open, diag);
    }

    let radius = params.search_radius_cycle;
    let mut rows: Vec<Candidate> = Vec::new();
    let sigmas = [
        (Detector::GeometryFree, gf_sigma_m / wavelength_m.max(1e-9)),
        (Detector::MelbourneWubbena, wl_sigma_cyc),
        (Detector::TimeDifference, td_sigma_cyc),
    ];
    for k in -radius..=radius {
        let kf = k as f64;
        let mut nsigma: BTreeMap<String, f64> = BTreeMap::new();
        let mut supporting: Vec<String> = Vec::new();
        let mut residual_sum = 0.0;
        let mut n_used = 0usize;
        for (det, obs_cyc) in &observations {
            let sigma = sigmas.iter().find(|(d, _)| d == det).map(|(_, s)| *s).unwrap_or(1.0).max(1e-6);
            let n = ((obs_cyc - kf) / sigma).abs();
            nsigma.insert(det.key().to_string(), n);
            if n <= params.n_sigma {
                supporting.push(det.key().to_string());
                residual_sum += n;
                n_used += 1;
            }
        }
        let _ = det_n.len();
        // 得分：支持检测器数（主）+ 平均接近度（次）
        let support_count = supporting.len();
        let proximity = if n_used > 0 { 1.0 - (residual_sum / n_used as f64 / params.n_sigma).min(1.0) } else { 0.0 };
        let score = support_count as f64 / 3.0 * 0.7 + proximity * 0.3;
        let mut reject = Vec::new();
        if support_count == 1 && observations.len() >= 2 {
            reject.push(format!(
                "仅 {} 支持该整数，与 {} 冲突；不以单一检测器当真值",
                supporting[0],
                observations.keys().filter(|d| !supporting.contains(&d.key().to_string())).map(|d| d.key()).collect::<Vec<_>>().join("/")
            ));
        }
        let state = if support_count == 0 {
            CandidateState::Rejected
        } else if support_count == 1 && observations.len() >= 2 {
            CandidateState::Rejected
        } else {
            CandidateState::Unknown
        };
        rows.push(Candidate {
            id: String::new(),
            offset_cycles: k,
            score,
            supporting,
            detector_nsigma: nsigma,
            state,
            reject_reasons: reject,
        });
    }

    // 在多检测器印证的候选中，取最高分；等分（得分差 <= 0.02）全部保留为 ambiguous
    let corroborated: Vec<&mut Candidate> = rows
        .iter_mut()
        .filter(|c| matches!(c.state, CandidateState::Unknown))
        .collect();
    let status;
    if corroborated.is_empty() {
        // 没有任何多检测器印证
        let any_sig = observations.values().any(|v| v.abs() > params.n_sigma * 0.5);
        if any_sig {
            diag.push("存在显著跳变但无任何整数得到 >=2 个检测器印证：候选互相冲突，全部拒绝并保留记录".to_string());
            status = BoundaryStatus::Inconclusive;
        } else {
            // k=0 被多数支持 => 无周跳
            for c in rows.iter_mut() {
                if c.offset_cycles == 0 {
                    c.state = CandidateState::Active;
                } else if matches!(c.state, CandidateState::Unknown) {
                    c.state = CandidateState::Rejected;
                }
            }
            status = BoundaryStatus::Open;
            finalize_ids(&mut rows, band, gf_jump_m, mw_jump_cyc, td_res_cyc);
            return (rows, status, diag);
        }
    } else {
        let best = corroborated.iter().map(|c| c.score).fold(0.0f64, f64::max);
        let tied: Vec<i32> = corroborated.iter().filter(|c| (c.score - best).abs() <= 0.02).map(|c| c.offset_cycles).collect();
        if tied.len() == 1 {
            for c in rows.iter_mut() {
                if c.offset_cycles == tied[0] {
                    c.state = CandidateState::Active;
                } else if matches!(c.state, CandidateState::Unknown) {
                    c.state = CandidateState::Rejected;
                    c.reject_reasons.push(format!("低于唯一最佳整数 {}", tied[0]));
                }
            }
            status = BoundaryStatus::Candidate;
        } else {
            // 多个等分整周偏移：全部保留为合法候选，无法唯一决定
            for c in rows.iter_mut() {
                if tied.contains(&c.offset_cycles) {
                    c.state = CandidateState::Ambiguous;
                    c.reject_reasons.push(format!(
                        "多个等分整周偏移 {:?} 得分相同，无法唯一决定，保留全部合法候选",
                        tied
                    ));
                } else if matches!(c.state, CandidateState::Unknown) {
                    c.state = CandidateState::Rejected;
                    c.reject_reasons.push("得分低于等分最优集合".to_string());
                }
            }
            diag.push(format!("多个等分整周偏移 {tied:?} 得分相同：保留全部合法候选，等待人工裁定"));
            status = BoundaryStatus::Candidate;
        }
    }

    finalize_ids(&mut rows, band, gf_jump_m, mw_jump_cyc, td_res_cyc);
    (rows, status, diag)
}

fn finalize_ids(rows: &mut [Candidate], band: &str, gf: Option<f64>, mw: Option<f64>, td: Option<f64>) {
    for c in rows.iter_mut() {
        let canon = format!("cand|{band}|k={}|gf={:?}|mw={:?}|td={:?}", c.offset_cycles, gf, mw, td);
        c.id = stable_id("cand", &canon);
    }
}
