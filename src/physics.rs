//! 领域计算：多频组合与周跳候选。
//!
//! 三个检测器是三个独立的“观点”，任何一个都不是真值：
//! * GF（几何无关 / 电离层组合）  L1*λ1 - L2*λ2
//! * MW（Melbourne–Wubbena 宽巷）  L_w - (f1*P1+f2*P2)/(f1+f2)，以宽巷周计
//! * TD（Doppler 辅助时间差分）    ΔL_m - (-D·λ·Δt) 的残差
//!
//! 修复只存偏移层：采纳候选生成 RepairOffset，不改动原始载波。

use crate::domain::*;
use serde::{Deserialize, Serialize};

pub const C_LIGHT: f64 = 299_792_458.0;
pub const ALGORITHM_VERSION: &str = "cycle-slip-engine/v1.3.0";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    Gf,
    Mw,
    Td,
    Lli,
}

impl Detector {
    pub fn as_str(self) -> &'static str {
        match self {
            Detector::Gf => "gf",
            Detector::Mw => "mw",
            Detector::Td => "td",
            Detector::Lli => "lli",
        }
    }
}

/// 每个 (sv, 边界历元) 的候选。整数对 (n1,n2) 表示“从后弧段减去的整周数”
/// （即偏移层 offset = -jump 的取整值）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub candidate_id: String,
    pub sv: String,
    pub epoch_idx: usize,
    pub detector: Detector,
    pub n1: i64,
    pub n2: i64,
    pub residual: f64,
    pub residual_unit: String,
    pub score: f64,
    pub within_tolerance: bool,
    pub status: CandidateStatus,
    pub reject_reasons: Vec<String>,
    pub clock_explained: bool,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Proposed,
    Accepted,
    Rejected,
    ExplainedByClock,
    Ambiguous,
    Unknown,
}

impl CandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateStatus::Proposed => "proposed",
            CandidateStatus::Accepted => "accepted",
            CandidateStatus::Rejected => "rejected",
            CandidateStatus::ExplainedByClock => "explained_by_clock",
            CandidateStatus::Ambiguous => "ambiguous",
            CandidateStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub algorithm_version: String,
    pub params_fingerprint: String,
    pub source_ids: Vec<String>,
    pub basis_event_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RepairOffset {
    pub candidate_id: String,
    pub sv: String,
    pub epoch_idx: usize,
    pub freq_id: String,
    pub offset_cycles: i64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArcBoundary {
    pub arc_id: String,
    pub sv: String,
    pub start_epoch: usize,
    pub end_epoch: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SeriesPoint {
    pub epoch_idx: usize,
    pub value: Option<f64>,
    pub unit: String,
    pub missing: bool,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComboSeries {
    pub sv: String,
    pub detector: Detector,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DerivedSnapshot {
    pub candidates: Vec<Candidate>,
    pub repairs: Vec<RepairOffset>,
    pub arcs: Vec<ArcBoundary>,
    pub gf_series: Vec<ComboSeries>,
    pub mw_series: Vec<ComboSeries>,
    pub td_series: Vec<ComboSeries>,
    pub clock_classification: Vec<ClockCall>,
    pub rejections: Vec<Rejection>,
    pub unknowns: Vec<UnknownState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockCall {
    pub epoch_idx: usize,
    pub declared_ms: Option<f64>,
    pub common_sv: Vec<String>,
    pub classified: bool,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rejection {
    pub candidate_id: String,
    pub reason_code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnknownState {
    pub sv: String,
    pub epoch_idx: usize,
    pub scope: String,
    pub reason: String,
}

/// 派生层结果头：带依赖版本，可整体过期。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Derivation {
    pub derivation_id: String,
    pub basis_event_seq: u64,
    pub algorithm_version: String,
    pub params_fingerprint: String,
    pub snapshot: DerivedSnapshot,
    pub is_current: bool,
    pub superseded_at_seq: Option<u64>,
}

#[derive(Clone)]
pub struct View<'a> {
    pub freqs: &'a [Frequency],
    pub epochs: &'a [RawEpoch],
    pub meas: Vec<RawMeas>,
    pub clock_events: &'a [ClockEvent],
}

fn freq_by_id<'a>(freqs: &'a [Frequency], id: &str) -> Option<&'a Frequency> {
    freqs.iter().find(|f| f.id == id)
}

fn meas_at<'a>(view: &'a [RawMeas], sv: &str, epoch: usize, freq_id: &str) -> Option<&'a RawMeas> {
    view.iter().find(|m| m.sv == sv && m.epoch_idx == epoch && m.freq_id == freq_id)
}

/// 稳定身份：测量源卡 id（原始层，补丁后出现 :r<seq> 版本）。
pub fn meas_source_id(sv: &str, epoch: usize, freq_id: &str, rev: u32) -> String {
    if rev == 0 {
        format!("m_{sv}_e{epoch:03}_{freq_id}")
    } else {
        format!("m_{sv}_e{epoch:03}_{freq_id}:r{rev}")
    }
}

pub fn gf_value(m1: &RawMeas, m2: &RawMeas, f1: &Frequency, f2: &Frequency) -> Option<f64> {
    match (m1.carrier_cycles.value, m2.carrier_cycles.value) {
        (Some(l1), Some(l2)) => Some(l1 * f1.wavelength_m - l2 * f2.wavelength_m),
        _ => None,
    }
}

/// MW 宽巷组合，单位宽巷周。
/// N_w = L1 - L2 - (f1 P1 + f2 P2)/(f1+f2)/λ_w ，λ_w = c/(f1-f2)
pub fn mw_value(m1: &RawMeas, m2: &RawMeas, f1: &Frequency, f2: &Frequency) -> Option<f64> {
    let (Some(l1), Some(l2), Some(p1), Some(p2)) =
        (m1.carrier_cycles.value, m2.carrier_cycles.value, m1.pseudorange.value, m2.pseudorange.value)
    else {
        return None;
    };
    let lw = C_LIGHT / (f1.freq_hz - f2.freq_hz);
    let code_wl = (f1.freq_hz * p1 + f2.freq_hz * p2) / (f1.freq_hz + f2.freq_hz);
    Some(l1 - l2 - code_wl / lw)
}

/// TD 残差（米）：实际载波增量相对 Doppler 预测增量的偏差。
/// 预测 ΔL_m = -D_mid · Δt；残差 = (L_cur-L_prev)·λ - 预测。
pub fn td_residual(
    prev: &RawMeas,
    cur: &RawMeas,
    f: &Frequency,
    dt_s: f64,
) -> Option<f64> {
    let (Some(lp), Some(lc), Some(dp), Some(dc)) =
        (prev.carrier_cycles.value, cur.carrier_cycles.value, prev.doppler_hz.value, cur.doppler_hz.value)
    else {
        return None;
    };
    let d_mid = (dp + dc) / 2.0;
    let predicted_m = -d_mid * f.wavelength_m * dt_s;
    Some((lc - lp) * f.wavelength_m - predicted_m)
}

/// 时间序列（不等间隔、缺频都直接体现在点上）。
pub fn build_series(view: &View) -> (Vec<ComboSeries>, Vec<ComboSeries>, Vec<ComboSeries>) {
    let mut svs: Vec<String> = view.meas.iter().map(|m| m.sv.clone()).collect();
    svs.sort();
    svs.dedup();
    let mut epochs: Vec<&RawEpoch> = view.epochs.iter().collect();
    epochs.sort_by_key(|e| e.idx);
    let f1 = freq_by_id(view.freqs, "L1");
    let f2 = freq_by_id(view.freqs, "L2");

    let mut gf_out = Vec::new();
    let mut mw_out = Vec::new();
    let mut td_out = Vec::new();
    for sv in &svs {
        let (mut gf_pts, mut mw_pts, mut td_pts) = (Vec::new(), Vec::new(), Vec::new());
        let mut prev: Option<(usize, RawMeas)> = None;
        for ep in &epochs {
            let mk = |v: Option<f64>, unit: &str, missing: bool, diag: Option<String>| SeriesPoint {
                epoch_idx: ep.idx,
                value: v,
                unit: unit.into(),
                missing,
                diagnostic: diag,
            };
            let m1 = f1.and_then(|f| meas_at(&view.meas, sv, ep.idx, &f.id));
            let m2 = f2.and_then(|f| meas_at(&view.meas, sv, ep.idx, &f.id));

            match (m1, m2, f1, f2) {
                (Some(a), Some(b), Some(fa), Some(fb)) => {
                    let diag = diag_of(a, b);
                    gf_pts.push(mk(gf_value(a, b, fa, fb), "m", false, diag.clone()));
                    mw_pts.push(mk(mw_value(a, b, fa, fb), "cycle_wl", false, diag));
                }
                _ => {
                    gf_pts.push(mk(None, "m", true, Some("频率缺失：电离层组合不可计算".into())));
                    mw_pts.push(mk(None, "cycle_wl", true, Some("频率缺失：MW 不可计算".into())));
                }
            }

            if let (Some(cur), Some(fa)) = (m1, f1) {
                match &prev {
                    Some((pe, p)) => {
                        let prev_ep = view.epochs.iter().find(|e| e.idx == *pe);
                        let dt = match (prev_ep, ep.interval_s_from_prev) {
                            (_, Some(dt)) => Some(dt),
                            (Some(pe2), None) => match (pe2.timestamp_epoch_s, ep.timestamp_epoch_s) {
                                (Some(a), Some(c)) => Some((c - a) as f64),
                                _ => None,
                            },
                            _ => None,
                        };
                        match dt {
                            Some(dt) if dt > 0.0 => {
                                let v = td_residual(p, cur, fa, dt);
                                td_pts.push(mk(
                                    v,
                                    "m",
                                    v.is_none(),
                                    if v.is_none() { Some("观测值无法转换，TD 不可计算".into()) } else { None },
                                ));
                            }
                            _ => td_pts.push(mk(None, "m", true, Some("历元间隔未知，TD 不可计算".into()))),
                        }
                    }
                    None => td_pts.push(mk(None, "m", true, Some("首历元无差分".into()))),
                }
                prev = Some((ep.idx, cur.clone()));
            } else {
                td_pts.push(mk(None, "m", true, Some("L1 缺卡，TD 断链".into())));
                prev = None;
            }
        }
        gf_out.push(ComboSeries { sv: sv.clone(), detector: Detector::Gf, points: gf_pts });
        mw_out.push(ComboSeries { sv: sv.clone(), detector: Detector::Mw, points: mw_pts });
        td_out.push(ComboSeries { sv: sv.clone(), detector: Detector::Td, points: td_pts });
    }
    (gf_out, mw_out, td_out)
}

fn diag_of(a: &RawMeas, b: &RawMeas) -> Option<String> {
    let mut msgs = Vec::new();
    for m in [a, b] {
        for q in [&m.pseudorange, &m.carrier_cycles, &m.doppler_hz] {
            if let Some(d) = &q.diagnostic {
                msgs.push(format!("{}/{}: {d}", m.sv, m.freq_id));
            }
        }
    }
    if msgs.is_empty() { None } else { Some(msgs.join("; ")) }
}
