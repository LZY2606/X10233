//! 领域计算引擎（纯函数，可重放）。
//!
//! 电离层组合(GF)、Melbourne-Wubbena(MW) 与时间差分(TD) 各自给出候选，
//! 不以任一检测器当真值；钟跳与单星周跳分开；修复只存“偏移层”。
//! 局部输入变化只会改变依赖它的派生身份（见各对象 dependency_keys）。

use crate::model::*;
use crate::params::{DetectorParams, ALGORITHM_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionInput {
    pub event_seq: i64,
    pub candidate_id: String,
    pub accept: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInput {
    pub event_seq: i64,
    pub boundary_key: String,
    pub winner_candidate_id: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct ProjectInput<'a> {
    pub input: &'a InputVersion,
    pub params: &'a DetectorParams,
    pub decisions: Vec<DecisionInput>,
    pub conflict_resolutions: Vec<ConflictInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectedResult {
    pub algorithm_version: String,
    pub params_fingerprint: String,
    pub input_version_id: String,
    pub arcs: Vec<ArcView>,
    pub clock_jumps: Vec<ClockJumpView>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockJumpView {
    pub epoch_index: i64,
    pub t_gps_s: f64,
    pub shift_m: f64,
    pub shift_s: f64,
    pub evidence_sats: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArcState {
    Active,
    Broken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcEpoch {
    pub value_id: String,
    pub epoch_index: i64,
    pub t_gps_s: f64,
    pub dt_prev_s: Option<f64>,
    pub pseudorange_m: Option<f64>,
    pub carrier_phase_m: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub snr_raw: Option<f64>,
    pub lli: Option<u8>,
    pub codes: Vec<String>,
    pub unusable: Vec<String>,
    /// 应用已接受“偏移层”后的相位（米）；原始 carrier_phase_m 永不修改。
    pub repaired_phase_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BoundaryKind {
    ArcStart,
    ArcEnd,
    IntervalGap,
    FrequencyMissing,
    ReceiverClockJump,
    Slip,
    LliFalseAlarm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryView {
    pub boundary_key: String,
    pub after_epoch_index: i64,
    pub t_gps_s: f64,
    pub kind: BoundaryKind,
    pub detail: String,
    pub candidate_ids: Vec<String>,
    pub disposition: Disposition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorVote {
    pub state: String,
    pub residual: Option<f64>,
    pub unit: String,
    pub estimate_text: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateView {
    pub candidate_id: String,
    pub boundary_key: String,
    pub after_epoch_index: i64,
    pub offsets_cycles: BTreeMap<String, i64>,
    pub family_step: BTreeMap<String, i64>,
    pub family_k: i64,
    pub gf: DetectorVote,
    pub mw: DetectorVote,
    pub td: DetectorVote,
    pub doppler: DetectorVote,
    pub score: f64,
    pub score_components: BTreeMap<String, f64>,
    pub legal: bool,
    pub reject_reasons: Vec<String>,
    pub status: CandidateStatus,
    pub disposition: Disposition,
    pub decision_event_seq: Option<i64>,
    pub decision_reason: Option<String>,
    pub dependency_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcView {
    pub arc_id: String,
    pub content_key: String,
    pub sat: String,
    pub band: String,
    pub frequency_hz: f64,
    pub wavelength_m: f64,
    pub start_epoch_index: i64,
    pub end_epoch_index: i64,
    pub epochs: Vec<ArcEpoch>,
    pub boundaries: Vec<BoundaryView>,
    pub candidates: Vec<CandidateView>,
    pub state: ArcState,
    pub dependency_keys: Vec<String>,
}

// ---------- 观测提取（波段层；LLI/码仍保留具体观测码） ----------

#[derive(Debug, Clone)]
struct Sig {
    p_m: Option<f64>,
    code_p: Option<String>,
    l_cyc: Option<f64>,
    code_l: Option<String>,
    d_hz: Option<f64>,
    code_d: Option<String>,
    s_raw: Option<f64>,
    code_s: Option<String>,
    lli: Option<u8>,
    unusable: Vec<String>,
    codes: Vec<String>,
}

impl Sig {
    fn empty() -> Self {
        Self {
            p_m: None,
            code_p: None,
            l_cyc: None,
            code_l: None,
            d_hz: None,
            code_d: None,
            s_raw: None,
            code_s: None,
            lli: None,
            unusable: vec![],
            codes: vec![],
        }
    }
}

fn extract_epoch(ep: &EpochRecord) -> BTreeMap<String, BTreeMap<String, Sig>> {
    let mut out: BTreeMap<String, BTreeMap<String, Sig>> = BTreeMap::new();
    for se in &ep.sats {
        let bands = out.entry(se.sat.clone()).or_default();
        for it in &se.items {
            let Some((kind, band, _attr)) = parse_code(&it.code) else { continue };
            let sig = bands.entry(band.clone()).or_insert_with(Sig::empty);
            sig.codes.push(it.code.clone());
            sig.lli = sig.lli.or(it.lli);
            if !it.quant.is_usable() {
                sig.unusable
                    .push(format!("{}: {}", it.code, it.quant.diagnostic.clone().unwrap_or_default()));
                continue;
            }
            let v = it.quant.value.unwrap();
            match kind {
                ObsKind::Pseudorange if sig.p_m.is_none() => {
                    sig.p_m = Some(v);
                    sig.code_p = Some(it.code.clone());
                }
                ObsKind::Carrier if sig.l_cyc.is_none() => {
                    sig.l_cyc = Some(v);
                    sig.code_l = Some(it.code.clone());
                }
                ObsKind::Doppler if sig.d_hz.is_none() => {
                    sig.d_hz = Some(v);
                    sig.code_d = Some(it.code.clone());
                }
                ObsKind::Snr if sig.s_raw.is_none() => {
                    sig.s_raw = Some(v);
                    sig.code_s = Some(it.code.clone());
                }
                _ => {}
            }
        }
    }
    out
}

fn wavelength(sat: &str, band: &str, params: &DetectorParams) -> Option<f64> {
    let k = params.glo_frequency_number.get(sat).copied();
    band_frequency_hz(sat, band, k).map(|f| C_LIGHT_M_S / f)
}

// ---------- 整机钟跳：要求多颗卫星同向码跳变 ----------

fn detect_clock_jumps(
    input: &InputVersion,
    params: &DetectorParams,
) -> BTreeMap<i64, (f64, Vec<String>)> {
    let mut jumps = BTreeMap::new();
    if input.epochs.len() < 2 {
        return jumps;
    }
    let per_epoch: Vec<BTreeMap<String, BTreeMap<String, Sig>>> =
        input.epochs.iter().map(extract_epoch).collect();
    let sat_set: Vec<String> = per_epoch
        .iter()
        .flat_map(|m| m.keys().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for j in 1..input.epochs.len() {
        let dt = input.epochs[j].t_gps_s - input.epochs[j - 1].t_gps_s;
        if !dt.is_finite() || dt <= 0.0 {
            continue;
        }
        let mut shifts: Vec<(String, f64)> = vec![];
        for sat in &sat_set {
            let Some(cur) = per_epoch[j].get(sat) else { continue };
            let Some(prev) = per_epoch[j - 1].get(sat) else { continue };
            let mut vals = vec![];
            for (band, s_now) in cur {
                let Some(s_prev) = prev.get(band) else { continue };
                if let (Some(p1), Some(p0), Some(d0)) = (s_now.p_m, s_prev.p_m, s_prev.d_hz) {
                    let lam = wavelength(sat, band, params).unwrap_or(0.19);
                    vals.push((p1 - p0) - (-d0 * lam * dt));
                }
            }
            if !vals.is_empty() {
                vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
                shifts.push((sat.clone(), vals[vals.len() / 2]));
            }
        }
        let total = sat_set.len().max(1);
        let big: Vec<(String, f64)> =
            shifts.iter().filter(|(_, s)| s.abs() >= params.clock_min_m).cloned().collect();
        if big.is_empty() {
            continue;
        }
        let pos = big.iter().filter(|(_, s)| *s > 0.0).count();
        let neg = big.len() - pos;
        let aligned = pos.max(neg);
        if aligned as f64 / total as f64 >= params.clock_satellite_fraction {
            let mut sv: Vec<f64> = big
                .iter()
                .filter(|(_, s)| if pos >= neg { *s > 0.0 } else { *s < 0.0 })
                .map(|(_, s)| *s)
                .collect();
            sv.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let shift = sv[sv.len() / 2];
            jumps.insert(
                input.epochs[j].index,
                (shift, big.into_iter().map(|(s, _)| s).collect()),
            );
        }
    }
    jumps
}

// ---------- 候选与边界内部结构 ----------

#[derive(Clone)]
struct Cand {
    offsets: Vec<(String, i64)>, // bands 有序
    family_step: Vec<(String, i64)>,
    family_k: i64,
}

struct SatBoundary {
    after_index: i64,
    t_gps_s: f64,
    dt: f64,
    kind: BoundaryKind,
    detail: String,
    bands: Vec<String>,
    candidates: Vec<Cand>,
    // 零偏移下的各检测器量（用于 LLI 误报/滑变判定与展示）
    zero_gf: Option<f64>,
    zero_mw: Option<f64>,
    zero_td: Option<f64>,
    zero_dop: Option<f64>,
    gf_slope_m_s: f64,
    dm: Option<f64>,
    lli_sat: bool,
    lli_band: Option<String>,
    gf_est: Option<f64>,
    mw_est: Option<f64>,
}

fn gaussian_score(residual: f64, sigma: f64) -> f64 {
    let x = residual / sigma;
    (-0.5 * x * x).exp()
}

/// 生成卫星级边界（钟跳之外的边界分析）。
fn build_sat_boundaries(
    input: &InputVersion,
    params: &DetectorParams,
    clock_epochs: &BTreeSet<i64>,
    per_epoch: &[BTreeMap<String, BTreeMap<String, Sig>>],
    sat: &str,
) -> BTreeMap<i64, SatBoundary> {
    let mut out = BTreeMap::new();
    let n = input.epochs.len();
    for j in 1..n {
        let after = input.epochs[j].index;
        let dt = input.epochs[j].t_gps_s - input.epochs[j - 1].t_gps_s;
        let prev = per_epoch[j - 1].get(sat);
        let cur = per_epoch[j].get(sat);
        if cur.is_none() {
            continue; // 卫星尚未升起 / 当历元缺失整星，弧段在别处体现
        }
        let cur = cur.unwrap();
        let gap = (dt - params.nominal_interval_s).abs() > params.interval_tolerance_s;

        if clock_epochs.contains(&after) {
            out.insert(after, SatBoundary {
                after_index: after,
                t_gps_s: input.epochs[j].t_gps_s,
                dt,
                kind: BoundaryKind::ReceiverClockJump,
                detail: "整机接收机钟跳：多颗卫星码/相同向跳变，单星周跳检测器已挂起".to_string(),
                bands: vec![],
                candidates: vec![],
                zero_gf: None, zero_mw: None, zero_td: None, zero_dop: None,
                gf_slope_m_s: 0.0, dm: None, lli_sat: false, lli_band: None,
                gf_est: None, mw_est: None,
            });
            continue;
        }

        // 收集该星所有出现过的波段（有序）
        let mut bands: BTreeSet<String> = BTreeSet::new();
        if let Some(p) = prev {
            bands.extend(p.keys().cloned());
        }
        bands.extend(cur.keys().cloned());
        let bands: Vec<String> = bands.into_iter().collect();
        let carrier_ok = |m: Option<&BTreeMap<String, Sig>>, b: &str| {
            m.and_then(|mm| mm.get(b)).and_then(|s| s.l_cyc.map(|_| ())).is_some()
        };
        let lli_band = |m: &BTreeMap<String, Sig>| -> Option<String> {
            for b in &bands {
                if let Some(s) = m.get(b) {
                    if s.lli == Some(1) && s.code_l.is_some() {
                        return Some(b.clone());
                    }
                }
            }
            None
        };
        let lli_on_cur = lli_band(cur);

        // 频率缺失：上一历元某波段载波可用，本历元该星在但该波段无可用载波
        let missing: Vec<String> = bands
            .iter()
            .filter(|b| carrier_ok(prev, b) && !carrier_ok(Some(cur), b))
            .cloned()
            .collect();
        if !missing.is_empty() {
            out.insert(after, SatBoundary {
                after_index: after,
                t_gps_s: input.epochs[j].t_gps_s,
                dt,
                kind: BoundaryKind::FrequencyMissing,
                detail: format!("频率缺失：波段 {} 在本历元无可用载波（值留在原始层并附诊断）", missing.join(",")),
                bands: vec![],
                candidates: vec![],
                zero_gf: None, zero_mw: None, zero_td: None, zero_dop: None,
                gf_slope_m_s: 0.0, dm: None, lli_sat: lli_on_cur.is_some(),
                lli_band: lli_on_cur, gf_est: None, mw_est: None,
            });
            continue;
        }

        let common: Vec<String> = bands
            .iter()
            .filter(|b| carrier_ok(prev, b) && carrier_ok(Some(cur), b))
            .cloned()
            .collect();
        if common.is_empty() {
            continue;
        }

        let prevm = prev.unwrap();
        let mut bnd = SatBoundary {
            after_index: after,
            t_gps_s: input.epochs[j].t_gps_s,
            dt,
            kind: BoundaryKind::Slip,
            detail: String::new(),
            bands: common.clone(),
            candidates: vec![],
            zero_gf: None, zero_mw: None, zero_td: None, zero_dop: None,
            gf_slope_m_s: 0.0, dm: None, lli_sat: lli_on_cur.is_some(),
            lli_band: lli_on_cur,
            gf_est: None, mw_est: None,
        };

        if common.contains(&"1".to_string()) && common.contains(&"2".to_string()) {
            analyze_dual(input, params, per_epoch, sat, j, &dt, &mut bnd);
        } else {
            analyze_single(params, prevm, cur, &dt, &common, &mut bnd);
        }

        let slip = bnd.zero_gf.map(|r| r.abs() > params.gf_sigma_m).unwrap_or(false)
            || bnd.zero_mw.map(|r| r.abs() > params.mw_sigma_cycles).unwrap_or(false)
            || bnd.zero_td.map(|r| r.abs() > params.td_sigma_m).unwrap_or(false)
            || bnd.zero_dop.map(|r| r.abs() > params.doppler_sigma_m).unwrap_or(false);

        if !slip {
            if bnd.lli_sat {
                bnd.kind = BoundaryKind::LliFalseAlarm;
                bnd.detail = format!(
                    "LLI=1 但 GF/MW/TD/Doppler 残差均未超阈（GF={:?}, MW={:?}, TD={:?}, D={:?}）：记为误报",
                    fmt(bnd.zero_gf), fmt(bnd.zero_mw), fmt(bnd.zero_td), fmt(bnd.zero_dop)
                );
            } else if gap {
                bnd.kind = BoundaryKind::IntervalGap;
                bnd.detail = format!("不等间隔历元：实际间隔 {:.1}s，未检出周跳", dt);
                bnd.candidates.clear();
            } else {
                continue; // 无事件：不产生边界
            }
        } else {
            bnd.kind = BoundaryKind::Slip;
            let mut ev = vec![];
            if bnd.zero_gf.map(|r| r.abs() > params.gf_sigma_m).unwrap_or(false) {
                ev.push("GF");
            }
            if bnd.zero_mw.map(|r| r.abs() > params.mw_sigma_cycles).unwrap_or(false) {
                ev.push("MW");
            }
            if bnd.zero_td.map(|r| r.abs() > params.td_sigma_m).unwrap_or(false) {
                ev.push("TD");
            }
            if bnd.zero_dop.map(|r| r.abs() > params.doppler_sigma_m).unwrap_or(false) {
                ev.push("Doppler");
            }
            bnd.detail = format!(
                "周跳证据：{}；{}实际间隔 {:.1}s",
                ev.join("+"),
                if bnd.lli_sat { "LLI=1 佐证；" } else { "" },
                dt
            );
            if gap && !ev.is_empty() {
                bnd.detail.push_str("（同时为不等间隔）");
            }
        }
        out.insert(after, bnd);
    }
    out
}

fn fmt(o: Option<f64>) -> String {
    match o {
        Some(v) => format!("{:.4}", v),
        None => "NA".to_string(),
    }
}

// ---------- 双频分析：GF / MW / TD 三检测器 + Doppler ----------

fn gf_series(
    input: &InputVersion,
    params: &DetectorParams,
    per_epoch: &[BTreeMap<String, BTreeMap<String, Sig>>],
    sat: &str,
    up_to: usize,
) -> Vec<(f64, f64)> {
    let lam1 = wavelength(sat, "1", params);
    let lam2 = wavelength(sat, "2", params);
    let (Some(lam1), Some(lam2)) = (lam1, lam2) else { return vec![] };
    let mut v = vec![];
    for i in 0..=up_to.min(input.epochs.len() - 1) {
        let Some(m) = per_epoch[i].get(sat) else { continue };
        let (Some(s1), Some(s2)) = (m.get("1"), m.get("2")) else { continue };
        let (Some(l1), Some(l2)) = (s1.l_cyc, s2.l_cyc) else { continue };
        v.push((input.epochs[i].t_gps_s, l1 * lam1 - l2 * lam2));
    }
    v
}

fn trend_slope(series: &[(f64, f64)]) -> f64 {
    let pts: Vec<(f64, f64)> = series.iter().rev().skip(1).take(4).rev().cloned().collect();
    if pts.len() < 2 {
        return 0.0;
    }
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (x, y) in pts {
        num += (x - mx) * (y - my);
        den += (x - mx).powi(2);
    }
    if den > 0.0 { num / den } else { 0.0 }
}

fn mw_value(s1: &Sig, s2: &Sig, lam1: f64, lam2: f64, f1: f64, f2: f64) -> Option<f64> {
    let (l1, l2, p1, p2) = (s1.l_cyc?, s2.l_cyc?, s1.p_m?, s2.p_m?);
    let alpha = (f1 - f2) / (f1 + f2);
    Some((l1 - l2) - alpha * (p1 / lam1 + p2 / lam2))
}

fn round_half(x: f64) -> i64 {
    if x >= 0.0 { (x + 0.5).floor() as i64 } else { -((-x + 0.5).floor() as i64) }
}

#[allow(clippy::too_many_arguments)]
fn analyze_dual(
    input: &InputVersion,
    params: &DetectorParams,
    per_epoch: &[BTreeMap<String, BTreeMap<String, Sig>>],
    sat: &str,
    j: usize,
    dt: &f64,
    bnd: &mut SatBoundary,
) {
    let lam1 = wavelength(sat, "1", params).unwrap();
    let lam2 = wavelength(sat, "2", params).unwrap();
    let f1 = C_LIGHT_M_S / lam1;
    let f2 = C_LIGHT_M_S / lam2;
    let prev = per_epoch[j - 1].get(sat).unwrap();
    let cur = per_epoch[j].get(sat).unwrap();
    let s1p = prev.get("1").unwrap();
    let s2p = prev.get("2").unwrap();
    let s1c = cur.get("1").unwrap();
    let s2c = cur.get("2").unwrap();

    // --- GF：去电离层线性趋势的无几何组合跳变（米） ---
    let series = gf_series(input, params, per_epoch, sat, j);
    let slope = trend_slope(&series);
    let Some((gp, gc)) = s1p.l_cyc.zip(s2p.l_cyc).map(|(a, b)| a * lam1 - b * lam2)
        .zip(s1c.l_cyc.zip(s2c.l_cyc).map(|(a, b)| a * lam1 - b * lam2))
    else { return; };
    let gf_delta = gc - gp - slope * dt;
    bnd.gf_slope_m_s = slope;
    bnd.zero_gf = Some(gf_delta);
    bnd.gf_est = Some(gf_delta);

    // --- MW：宽巷组合历元差分（周） ---
    if let (Some(mp), Some(mc)) = (
        mw_value(s1p, s2p, lam1, lam2, f1, f2),
        mw_value(s1c, s2c, lam1, lam2, f1, f2),
    ) {
        let d = mc - mp;
        bnd.zero_mw = Some(d);
        bnd.mw_est = Some(d);
    }

    // --- TD：码约束相位增量；Doppler 独立佐证 ---
    let band_res = |a: &Sig, b: &Sig, lam: f64, d_hz: Option<f64>| -> (f64, f64) {
        let dp = b.p_m.unwrap_or(0.0) - a.p_m.unwrap_or(0.0);
        let dphi = (b.l_cyc.unwrap_or(0.0) - a.l_cyc.unwrap_or(0.0)) * lam;
        let dop = dphi - d_hz.map(|d| -d * lam * dt).unwrap_or(dp);
        (dphi - dp, dop)
    };
    let (td1, dop1) = band_res(s1p, s1c, lam1, s1p.d_hz);
    let (td2, dop2) = band_res(s2p, s2c, lam2, s2p.d_hz);
    bnd.zero_td = Some(td1.abs().max(td2.abs()));
    bnd.zero_dop = Some(dop1.abs().max(dop2.abs()));
    bnd.dm = Some((td1, td2));

    let gf_n = round_half(gf_delta / (lam1 - lam2));
    let mw_n = bnd.mw_est.map(round_half);
    let td_n1 = round_half(td1 / lam1);
    let td_n2 = round_half(td2 / lam2);

    // 各检测器分别产生候选族；同一偏移若由多个族支持，保留全部投票，不以任一检测器当真值。
    let mut cands: BTreeMap<(i64, i64), Cand> = BTreeMap::new();
    let mut push = |map: &mut BTreeMap<(i64, i64), Cand>, n1: i64, n2: i64,
                    step: (i64, i64), k: i64| {
        map.entry((n1, n2)).or_insert_with(|| Cand {
            offsets: vec![("1".to_string(), n1), ("2".to_string(), n2)],
            family_step: vec![("1".to_string(), step.0), ("2".to_string(), step.1)],
            family_k: k,
        });
    };

    // TD 族：(1,1) 等分整周偏移，码不可见；|k| 由码阈值界定。
    let k_code = ((params.td_sigma_m / (lam1 - lam2)).floor() as i64).min(params.family_max_k);
    for k in -k_code..=k_code {
        push(&mut cands, td_n1 + k, td_n2 + k, (1, 1), k);
    }
    // GF 族：GF 零空间 (77,60) 等分偏移，以 TD 中心为起点。
    let (g1, g2) = (round_half(lam1 / (lam1 - lam2)), round_half(lam2 / (lam1 - lam2)));
    for k in -params.family_max_k..=params.family_max_k {
        push(&mut cands, td_n1 + k * g1, td_n2 + k * g2, (g1, g2), k);
    }
    // MW 族：宽巷约束 n1-n2=mw_n，沿 (1,1) 枚举。
    if let Some(mw_n) = mw_n {
        for k in -params.family_max_k..=params.family_max_k {
            push(&mut cands, td_n2 + k + mw_n, td_n2 + k, (1, 1), k);
        }
    }

    bnd.candidates = cands.into_values().collect();
    bnd.candidates.sort_by_key(|c| (c.family_k, c.offsets[0].1, c.offsets[1].1));
}

fn analyze_single(
    params: &DetectorParams,
    prev: &BTreeMap<String, Sig>,
    cur: &BTreeMap<String, Sig>,
    dt: &f64,
    bands: &[String],
    bnd: &mut SatBoundary,
) {
    let b = &bands[0];
    let lam = wavelength_any(b, params);
    let a = prev.get(b).unwrap();
    let c = cur.get(b).unwrap();
    if let (Some(p0), Some(p1), Some(l0), Some(l1)) = (a.p_m, c.p_m, a.l_cyc, c.l_cyc) {
        let td = (l1 - l0) * lam - (p1 - p0);
        bnd.zero_td = Some(td.abs());
        let dop = a.d_hz.map(|d| (l1 - l0) * lam - (-d * lam * dt));
        bnd.zero_dop = dop.map(|x| x.abs());
        bnd.dm = Some((td, 0.0));
        let n = round_half(td / lam);
        bnd.candidates = vec![Cand {
            offsets: vec![(b.clone(), n)],
            family_step: vec![(b.clone(), 0)],
            family_k: 0,
        }];
    }
}

fn wavelength_any(band: &str, params: &DetectorParams) -> f64 {
    // 单频弧段不区分系统时用 GPS 表（GLO 星在 glo 表中有频率号）
    for sys in ['G', 'E', 'R', 'C'] {
        let sat = format!("{}00", sys);
        if let Some(lam) = wavelength(&sat, band, params) {
            // GLO 对 G00 给基频，近似；实际单频 GLO 分析时卫星名已知（此处兜底）
            return lam;
        }
    }
    C_LIGHT_M_S / 1575.42e6
}

// ---------- 弧段构建 ----------

fn build_segments(
    boundaries: &BTreeMap<i64, SatBoundary>,
    present: &[i64],
    band: &str,
) -> Vec<(i64, i64)> {
    let mut segs: Vec<(i64, i64)> = vec![];
    let mut start = match present.first() {
        Some(i) => *i,
        None => return segs,
    };
    for (idx, epoch_index) in present.iter().enumerate() {
        if idx == 0 {
            continue;
        }
        if let Some(b) = boundaries.get(epoch_index) {
            let breaks = match b.kind {
                BoundaryKind::ReceiverClockJump => true,
                BoundaryKind::FrequencyMissing => band_missing_here(b, band),
                BoundaryKind::Slip => b.bands.iter().any(|x| x == band),
                _ => false,
            };
            if breaks {
                segs.push((start, epoch_index - 1));
                start = *epoch_index;
            }
        }
    }
    if let Some(last) = present.last() {
        if *last >= start {
            segs.push((start, *last));
        }
    }
    segs
}

fn band_missing_here(b: &SatBoundary, band: &str) -> bool {
    let tail = b.detail.split("波段 ").nth(1).unwrap_or("");
    let nums = tail.split(" 在").next().unwrap_or("");
    nums.split([',', ' ']).any(|tok| tok.trim() == band)
}

fn missing_bands_here(b: &SatBoundary) -> Vec<String> {
    let tail = match b.detail.split("波段 ").nth(1) {
        Some(t) => t,
        None => return vec![],
    };
    let nums = tail.split(" 在").next().unwrap_or("");
    nums.split([',', ' '])
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

// ---------- 投影入口 ----------

pub fn project(inp: &ProjectInput) -> ProjectedResult {
    let input = inp.input;
    let params = inp.params;
    let pf = params.fingerprint();
    let per_epoch: Vec<BTreeMap<String, BTreeMap<String, Sig>>> =
        input.epochs.iter().map(extract_epoch).collect();

    let clock = detect_clock_jumps(input, params);
    let clock_epochs: BTreeSet<i64> = clock.keys().copied().collect();

    let mut clock_views = vec![];
    for (idx, (shift, sats)) in &clock {
        let ep = input.epochs.iter().find(|e| e.index == *idx);
        let mut sats = sats.clone();
        sats.sort();
        // 原始钟跳事件的支持（如有）
        let raw = input.clock_events.iter().any(|ce| ce.epoch_index == *idx);
        clock_views.push(ClockJumpView {
            epoch_index: *idx,
            t_gps_s: ep.map(|e| e.t_gps_s).unwrap_or(0.0),
            shift_m: *shift,
            shift_s: shift / C_LIGHT_M_S,
            evidence_sats: sats,
            source: if raw { "detector+clock_event_record".to_string() } else { "detector".to_string() },
        });
    }
    clock_views.sort_by_key(|c| c.epoch_index);

    // 全部卫星 × 波段
    let mut sat_bands: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for m in &per_epoch {
        for (sat, bands) in m {
            for (band, sig) in bands {
                if sig.l_cyc.is_some() || sig.code_l.is_some() {
                    sat_bands.entry(sat.clone()).or_default().insert(band.clone());
                }
            }
        }
    }

    // 每星边界
    let sat_boundaries: BTreeMap<String, BTreeMap<i64, SatBoundary>> = sat_bands
        .keys()
        .map(|sat| {
            (
                sat.clone(),
                build_sat_boundaries(input, params, &clock_epochs, &per_epoch, sat),
            )
        })
        .collect();

    // 决定：candidate_id -> 最新一条
    let mut accepted: BTreeMap<String, DecisionInput> = BTreeMap::new();
    let mut rejected: BTreeMap<String, DecisionInput> = BTreeMap::new();
    for d in &inp.decisions {
        if d.accept {
            accepted.insert(d.candidate_id.clone(), d.clone());
            rejected.remove(&d.candidate_id);
        } else {
            rejected.insert(d.candidate_id.clone(), d.clone());
            accepted.remove(&d.candidate_id);
        }
    }
    // 冲突解决：每个边界只保留胜者接受
    let mut loser_accepted: BTreeSet<String> = BTreeSet::new();
    for cr in &inp.conflict_resolutions {
        for cid in accepted.keys() {
            if cid.starts_with(&format!("{}::", cr.boundary_key)) && cid != &cr.winner_candidate_id
            {
                loser_accepted.insert(cid.clone());
            }
        }
    }
    for cid in &loser_accepted {
        accepted.remove(cid);
    }

    let mut arcs: Vec<ArcView> = vec![];
    for (sat, bandset) in &sat_bands {
        for band in bandset {
            let boundaries = sat_boundaries.get(sat).cloned().unwrap_or_default();
            let f_hz = band_frequency_hz(sat, band, params.glo_frequency_number.get(sat).copied())
                .unwrap_or(0.0);
            let lam = if f_hz > 0.0 { C_LIGHT_M_S / f_hz } else { 0.0 };

            // 该星该波段出现载波的历元（有序）
            let present: Vec<i64> = input
                .epochs
                .iter()
                .filter_map(|e| {
                    per_epoch[e.index as usize]
                        .get(sat)
                        .and_then(|m| m.get(band))
                        .filter(|s| s.l_cyc.is_some())
                        .map(|_| e.index)
                })
                .collect();
            let segs = build_segments(&boundaries, &present, band);

            // 已接受偏移的全局累积（修复只存偏移层，永不改原始相位）
            let mut accepted_offset_cycles: BTreeMap<i64, i64> = BTreeMap::new();
            for (after, b) in &boundaries {
                for c in &b.candidates {
                    if !c.offsets.iter().any(|(bb, _)| bb == band) {
                        continue;
                    }
                    let cid = candidate_id(input.id, sat, b, c);
                    if accepted.contains_key(&cid) {
                        if let Some((_, n)) = c.offsets.iter().find(|(bb, _)| bb == band) {
                            *accepted_offset_cycles.entry(*after).or_insert(0) += n;
                        }
                    }
                }
            }
            for (si, (seg_start, seg_end)) in segs.iter().enumerate() {
                let mut cum_cycles: i64 = 0;
                let mut arc_epochs = vec![];
                for e in &input.epochs {
                    if e.index < *seg_start || e.index > *seg_end {
                        continue;
                    }
                    let Some(m) = per_epoch[e.index as usize].get(sat) else { continue };
                    let Some(sig) = m.get(band) else { continue };
                    let Some(l) = sig.l_cyc else { continue };
                    // 进入该历元前应用在该历元边界上的已接受偏移
                    cum_cycles += accepted_offset_cycles.get(&e.index).copied().unwrap_or(0);
                    let dt_prev = input
                        .epochs
                        .iter()
                        .position(|x| x.index == e.index)
                        .filter(|&i| i > 0)
                        .map(|i| e.t_gps_s - input.epochs[i - 1].t_gps_s);
                    let dep = raw_value_deps(input, e.index, sat, band);
                    let value_id = crate::ids::content_id(
                        "value",
                        &crate::ids::canonical_json(&serde_json::json!({
                            "input": input.id, "sat": sat, "band": band,
                            "epoch": e.index, "raw_deps": dep,
                        })),
                    );
                    let repaired_m = l * lam + cum_cycles as f64 * lam;
                    arc_epochs.push(ArcEpoch {
                        value_id,
                        epoch_index: e.index,
                        t_gps_s: e.t_gps_s,
                        dt_prev_s: dt_prev,
                        pseudorange_m: sig.p_m,
                        carrier_phase_m: Some(l * lam),
                        doppler_hz: sig.d_hz,
                        snr_raw: sig.s_raw,
                        lli: sig.lli,
                        codes: sig.codes.clone(),
                        unusable: sig.unusable.clone(),
                        repaired_phase_m: Some(repaired_m),
                    });
                }
                if arc_epochs.is_empty() {
                    continue;
                }
                let arc_id = crate::ids::content_id(
                    "arc",
                    &crate::ids::canonical_json(&serde_json::json!({
                        "input": input.id, "sat": sat, "band": band,
                        "start": seg_start, "end": seg_end, "alg": ALGORITHM_VERSION, "pf": pf,
                    })),
                );
                let content_key = crate::ids::content_id(
                    "arcc",
                    &crate::ids::canonical_json(&serde_json::json!({
                        "sat": sat, "band": band, "start": seg_start, "end": seg_end,
                        "raw": arc_epochs.iter().map(|a| a.value_id.clone())
                            .collect::<Vec<_>>(),
                    })),
                );

                // 弧段上的边界视图（start/end + 落在范围内的非断裂边界）
                let mut bviews = vec![BoundaryView {
                    boundary_key: boundary_key(input.id, sat, seg_start, "start"),
                    after_epoch_index: seg_start,
                    t_gps_s: input.epochs.iter().find(|e| e.index == *seg_start)
                        .map(|e| e.t_gps_s).unwrap_or(0.0),
                    kind: BoundaryKind::ArcStart,
                    detail: "弧段开始".to_string(),
                    candidate_ids: vec![],
                    disposition: Disposition::Accepted,
                }];
                for (after, b) in &boundaries {
                    if *after < *seg_start || *after > seg_end + 1 {
                        continue;
                    }
                    if matches!(b.kind, BoundaryKind::Slip | BoundaryKind::LliFalseAlarm
                        | BoundaryKind::IntervalGap | BoundaryKind::ReceiverClockJump
                        | BoundaryKind::FrequencyMissing)
                    {
                        let ids: Vec<String> = b.candidates.iter()
                            .map(|c| candidate_id(input.id, sat, b, c)).collect();
                        let disp = boundary_disposition(b, band, &ids, &accepted, &rejected);
                        bviews.push(BoundaryView {
                            boundary_key: boundary_key(input.id, sat, after, &kind_tag(&b.kind)),
                            after_epoch_index: *after,
                            t_gps_s: b.t_gps_s,
                            kind: b.kind.clone(),
                            detail: b.detail.clone(),
                            candidate_ids: ids,
                            disposition: disp,
                        });
                    }
                }
                bviews.push(BoundaryView {
                    boundary_key: boundary_key(input.id, sat, seg_end, "end"),
                    after_epoch_index: seg_end,
                    t_gps_s: input.epochs.iter().find(|e| e.index == *seg_end)
                        .map(|e| e.t_gps_s).unwrap_or(0.0),
                    kind: BoundaryKind::ArcEnd,
                    detail: if si + 1 < segs.len() {
                        "弧段在此结束（后续为新弧段）".to_string()
                    } else {
                        "数据结束".to_string()
                    },
                    candidate_ids: vec![],
                    disposition: Disposition::Accepted,
                });

                let mut dep_keys: Vec<String> = arc_epochs.iter().map(|a| a.value_id.clone()).collect();
                dep_keys.push(format!("params:{}", pf));
                dep_keys.sort();
                dep_keys.dedup();

                let state = if si + 1 < segs.len() { ArcState::Broken } else { ArcState::Active };
                arcs.push(ArcView {
                    arc_id: arc_id.clone(),
                    content_key,
                    sat: sat.clone(),
                    band: band.clone(),
                    frequency_hz: f_hz,
                    wavelength_m: lam,
                    start_epoch_index: *seg_start,
                    end_epoch_index: *seg_end,
                    epochs: arc_epochs,
                    boundaries: bviews,
                    candidates: vec![],
                    state,
                    dependency_keys: dep_keys,
                });
            }

            // 候选视图（边界维度）挂到边界所属弧段
            for (after, b) in &boundaries {
                if !matches!(b.kind, BoundaryKind::Slip | BoundaryKind::LliFalseAlarm) {
                    continue;
                }
                let cand_views = build_candidate_views(input, params, sat, b, &accepted, &rejected);
                if cand_views.is_empty() {
                    continue;
                }
                let owner_start = segs
                    .iter()
                    .take_while(|(s, _e)| s <= after)
                    .last()
                    .map(|(s, _)| *s)
                    .unwrap_or(0);
                if let Some(arc) = arcs
                    .iter_mut()
                    .find(|a| a.sat == *sat && a.band == *band && a.start_epoch_index == owner_start)
                {
                    arc.candidates.extend(cand_views);
                }
            }
        }
    }

    arcs.sort_by(|a, b| {
        a.sat.cmp(&b.sat).then(a.band.cmp(&b.band)).then(a.start_epoch_index.cmp(&b.start_epoch_index))
    });
    for a in &mut arcs {
        a.boundaries.sort_by_key(|x| (x.after_epoch_index, kind_rank(&x.kind)));
        a.candidates.sort_by(|x, y| {
            x.after_epoch_index
                .cmp(&y.after_epoch_index)
                .then_with(|| y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| x.offsets_cycles.values().cmp(y.offsets_cycles.values()))
        });
    }

    ProjectedResult {
        algorithm_version: ALGORITHM_VERSION.to_string(),
        params_fingerprint: pf,
        input_version_id: input.id.clone(),
        arcs,
        clock_jumps: clock_views,
        diagnostics: input.diagnostics.iter().map(|d| format!("[{}] {}", d.scope, d.message)).collect(),
    }
}

fn kind_rank(k: &BoundaryKind) -> u8 {
    match k {
        BoundaryKind::ArcStart => 0,
        BoundaryKind::IntervalGap => 1,
        BoundaryKind::FrequencyMissing => 1,
        BoundaryKind::ReceiverClockJump => 1,
        BoundaryKind::Slip => 1,
        BoundaryKind::LliFalseAlarm => 1,
        BoundaryKind::ArcEnd => 2,
    }
}

fn kind_tag(k: &BoundaryKind) -> String {
    format!("{:?}", k).to_lowercase()
}

// ---------- 稳定身份 ----------

pub fn boundary_key(input_id: &str, sat: &str, after: i64, tag: &str) -> String {
    crate::ids::content_id(
        "bnd",
        &crate::ids::canonical_json(&serde_json::json!({
            "input": input_id, "sat": sat, "after": after, "tag": tag,
        })),
    )
}

pub fn candidate_id(input_id: &str, sat: &str, b: &SatBoundary, c: &Cand) -> String {
    let bk = boundary_key(input_id, sat, b.after_index, "slip");
    format!(
        "{}::{}",
        bk,
        crate::ids::content_id(
            "cand",
            &crate::ids::canonical_json(&serde_json::json!({
                "sat": sat, "after": b.after_index,
                "offsets": c.offsets, "step": c.family_step, "k": c.family_k,
            }))
        )
    )
}

/// 每个显示值的直接来源：历元、该星该波段原始观测码文本（内容变 -> 身份变）。
fn raw_value_deps(input: &InputVersion, epoch: i64, sat: &str, band: &str) -> Vec<String> {
    let Some(ep) = input.epochs.iter().find(|e| e.index == epoch) else { return vec![] };
    let Some(se) = ep.sats.iter().find(|s| s.sat == sat) else { return vec![] };
    let mut deps = vec![ep.raw_line.clone()];
    for it in &se.items {
        if let Some((_, b, _)) = parse_code(&it.code) {
            if b == band {
                deps.push(format!("{}={}|{}|lli={:?}", it.code, it.quant.raw, it.quant.unit, it.lli));
            }
        }
    }
    deps.sort();
    deps
}

// ---------- 决定/冲突重放 ----------

fn boundary_disposition(
    b: &SatBoundary,
    band: &str,
    ids: &[String],
    accepted: &BTreeMap<String, DecisionInput>,
    rejected: &BTreeMap<String, DecisionInput>,
) -> Disposition {
    let band_cands: Vec<&Cand> =
        b.candidates.iter().filter(|c| c.offsets.iter().any(|(bb, _)| bb == band)).collect();
    let n = band_cands.len();
    let any_accepted = ids.iter().any(|id| accepted.contains_key(id));
    if any_accepted {
        return Disposition::Accepted;
    }
    let all_rejected = !band_cands.is_empty()
        && band_cands
            .iter()
            .zip(ids.iter())
            .all(|(_, id)| rejected.contains_key(id));
    if all_rejected || (n == 1 && ids.iter().all(|id| rejected.contains_key(id))) {
        return Disposition::Rejected;
    }
    let _ = b;
    Disposition::Proposed
}

// ---------- 候选投票与评分 ----------

fn build_candidate_views(
    input: &InputVersion,
    params: &DetectorParams,
    sat: &str,
    b: &SatBoundary,
    accepted: &BTreeMap<String, DecisionInput>,
    rejected: &BTreeMap<String, DecisionInput>,
) -> Vec<CandidateView> {
    let lam1 = wavelength(sat, "1", params).unwrap_or(C_LIGHT_M_S / 1575.42e6);
    let lam2 = wavelength(sat, "2", params).unwrap_or(C_LIGHT_M_S / 1227.6e6);
    let dual = b.bands.len() >= 2;
    let (td1, td2) = b.dm.unwrap_or((b.zero_td.unwrap_or(0.0), 0.0));

    // 先计算合法性（TD 码约束两波段同时满足），确定 Ambiguous/Unique。
    let mut evaluated: Vec<(Cand, bool, Vec<String>, f64, [DetectorVote; 4], BTreeMap<String, f64>)> =
        vec![];
    for c in b.candidates.iter().cloned() {
        let mut reasons = vec![];
        let n1 = c.offsets.iter().find(|(x, _)| x == "1").map(|(_, v)| *v).unwrap_or(0);
        let n2 = c.offsets.iter().find(|(x, _)| x == "2").map(|(_, v)| *v).unwrap_or(0);
        let r1 = td1 - n1 as f64 * lam1;
        let r2 = if dual { td2 - n2 as f64 * lam2 } else { 0.0 };
        let td_res = r1.abs().max(if dual { r2.abs() } else { 0.0 });
        let legal = td_res <= params.td_sigma_m;
        if !legal {
            reasons.push(format!(
                "TD 码约束残差 {:.3f}m 超过阈值 {:.3f}m（检测器候选仍保留，但不合法）",
                td_res, params.td_sigma_m
            ));
        }

        // GF 投票（米）
        let gf_res = if dual {
            b.zero_gf.unwrap_or(0.0) - (n1 as f64 * lam1 - n2 as f64 * lam2)
        } else {
            f64::NAN
        };
        let gf = vote(
            if dual { gf_res } else { f64::NAN },
            params.gf_sigma_m,
            "m",
            format!(
                "GF 残差 {:.4}m（估计 ΔN1-ΔN2={}）",
                gf_res,
                b.gf_est.map(|g| format!("{:.2}", g)).unwrap_or_else(|| "NA".into())
            ),
        );

        // MW 投票（周）
        let mw_res = if dual {
            b.zero_mw.unwrap_or(0.0) - (n1 - n2) as f64
        } else {
            f64::NAN
        };
        let mw = vote(
            if dual { mw_res } else { f64::NAN },
            params.mw_sigma_cycles,
            "cycles",
            format!(
                "MW 残差 {:.4} 周（估计 ΔN1-ΔN2={}）",
                mw_res,
                b.mw_est.map(|g| format!("{:.2}", g)).unwrap_or_else(|| "NA".into())
            ),
        );

        // TD 投票（米）
        let td = DetectorVote {
            state: if legal { "ok".to_string() } else { "reject".to_string() },
            residual: Some(td_res),
            unit: "m".to_string(),
            estimate_text: format!("TD 残差 ({:.4},{:.4})m", r1, r2),
            reason: if legal { None } else { Some(reasons.join("; ")) },
        };

        // Doppler：整周偏移对相邻历元 Doppler 一致性无补偿作用，独立打分
        let dop_res = b.zero_dop.unwrap_or(f64::NAN);
        let doppler = if dop_res.is_nan() {
            DetectorVote {
                state: "unknown".to_string(),
                residual: None,
                unit: "m".to_string(),
                estimate_text: "Doppler 不可用".to_string(),
                reason: Some("缺少 Doppler 或伪距".to_string()),
            }
        } else {
            DetectorVote {
                state: if dop_res <= params.doppler_sigma_m { "ok".to_string() } else { "reject".to_string() },
                residual: Some(dop_res),
                unit: "m".to_string(),
                estimate_text: format!("Doppler 外推残差 {:.4}m", dop_res),
                reason: None,
            }
        };

        let s_gf = if dual { gaussian_score(gf_res, params.gf_sigma_m) } else { f64::NAN };
        let s_mw = if dual { gaussian_score(mw_res, params.mw_sigma_cycles) } else { f64::NAN };
        let s_td = gaussian_score(td_res, params.td_sigma_m);
        let s_dop = if dop_res.is_nan() { f64::NAN }
            else { gaussian_score(dop_res, params.doppler_sigma_m) };

        let mut comps = BTreeMap::new();
        let mut weighted = 0.0f64;
        let mut wsum = 0.0f64;
        if s_gf.is_finite() { comps.insert("gf".to_string(), s_gf); weighted += 0.30 * s_gf; wsum += 0.30; }
        if s_mw.is_finite() { comps.insert("mw".to_string(), s_mw); weighted += 0.30 * s_mw; wsum += 0.30; }
        comps.insert("td".to_string(), s_td); weighted += 0.30 * s_td; wsum += 0.30;
        if s_dop.is_finite() { comps.insert("doppler".to_string(), s_dop); weighted += 0.10 * s_dop; wsum += 0.10; }
        let score = if wsum > 0.0 { weighted / wsum } else { 0.0 };
        if gf.state == "reject" {
            reasons.push(format!("GF 残差 {:.3f}m 超阈 {:.3f}m", gf_res, params.gf_sigma_m));
        }
        if mw.state == "reject" {
            reasons.push(format!("MW 残差 {:.3f} 周超阈 {:.3f}", mw_res, params.mw_sigma_cycles));
        }
        evaluated.push((c, legal, reasons, score, [gf, mw, td, doppler], comps));
    }

    let n_legal = evaluated.iter().filter(|(_, legal, _, _, _, _)| *legal).count();
    let ambiguous = n_legal > 1;

    let mut views = vec![];
    for (c, legal, reasons, score, votes, comps) in evaluated {
        let bk = boundary_key(input.id, sat, b.after_index, "slip");
        let cid = candidate_id(input.id, sat, b, &c);
        let disposition;
        let dec_seq;
        let dec_reason;
        if let Some(d) = accepted.get(&cid) {
            disposition = Disposition::Accepted;
            dec_seq = Some(d.event_seq);
            dec_reason = d.reason.clone();
        } else if let Some(d) = rejected.get(&cid) {
            disposition = Disposition::Rejected;
            dec_seq = Some(d.event_seq);
            dec_reason = d.reason.clone();
        } else {
            disposition = Disposition::Proposed;
            dec_seq = None;
            dec_reason = None;
        }
        let status = if disposition == Disposition::Rejected {
            CandidateStatus::Rejected
        } else if !legal {
            CandidateStatus::Rejected
        } else if ambiguous {
            CandidateStatus::Ambiguous
        } else {
            CandidateStatus::Unique
        };
        let offsets: BTreeMap<String, i64> = c.offsets.into_iter().collect();
        let step: BTreeMap<String, i64> = c.family_step.into_iter().collect();
        let dep_key = bk.clone();
        views.push(CandidateView {
            candidate_id: cid,
            boundary_key: bk,
            after_epoch_index: b.after_index,
            offsets_cycles: offsets,
            family_step: step,
            family_k: c.family_k,
            gf: votes[0].clone(),
            mw: votes[1].clone(),
            td: votes[2].clone(),
            doppler: votes[3].clone(),
            score,
            score_components: comps,
            legal,
            reject_reasons: reasons,
            status,
            disposition,
            decision_event_seq: dec_seq,
            decision_reason: dec_reason,
            dependency_keys: vec![dep_key, format!("params:{}", params.fingerprint())],
        });
    }
    views
}

fn vote(residual: f64, sigma: f64, unit: &str, text: String) -> DetectorVote {
    if residual.is_nan() {
        return DetectorVote {
            state: "unknown".to_string(),
            residual: None,
            unit: unit.to_string(),
            estimate_text: "该检测器在此边界不可用（单频/缺码）".to_string(),
            reason: Some("需要双频伪距与载波".to_string()),
        };
    }
    DetectorVote {
        state: if residual.abs() <= sigma { "ok".to_string() } else { "reject".to_string() },
        residual: Some(residual),
        unit: unit.to_string(),
        estimate_text: text,
        reason: None,
    }
}
