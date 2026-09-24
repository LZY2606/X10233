//! 检测器：电离层组合、Melbourne-Wübbena、时间差分。
//! 三者可给出不同候选，不以任一检测器当真值；钟跳与单星周跳分开；
//! 候选、拒绝原因、误差与未知状态都是一等数据。

use crate::domain::*;
use crate::ids;

pub const DETECTOR_ALGO_VERSION: &str = "det-v1";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectorParams {
    pub iono_threshold_cycles: f64,
    pub mw_threshold_cycles: f64,
    pub td_threshold_cycles: f64,
    pub td_max_gap_seconds: f64, // 不等间隔历元：超过该间隔不做时间差分
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            iono_threshold_cycles: 0.5,
            mw_threshold_cycles: 1.0,
            td_threshold_cycles: 0.75,
            td_max_gap_seconds: 120.0,
        }
    }
}

/// 单星单频的时间序列样本（从数据集中抽取）。
#[derive(Debug, Clone)]
pub struct ArcSample {
    pub epoch_seconds: f64,
    pub carrier_cycles: Option<f64>,
    pub code_m: Option<f64>,
    pub lli: Option<u8>,
    pub clock_offset: Option<f64>,
}

/// 检测输出：候选 + 拒绝原因（都是一等数据）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectorOutput {
    pub candidates: Vec<Candidate>,
    pub rejections: Vec<serde_json::Value>, // {detector, epoch, reason}
}

fn get_obs<'a>(sat: &'a SatObs, want_type: char, freq_no: u8) -> Option<&'a ObsValue> {
    sat.records.iter().find_map(|r| {
        let m = parse_code(&r.code)?;
        if m.obs_type == want_type && m.freq_no == freq_no { Some(&r.obs) } else { None }
    })
}

/// 从数据集抽取弧段样本。
pub fn extract_arc_samples(ds: &Dataset, sat: &str, freq_no: u8) -> Vec<ArcSample> {
    ds.epochs.iter().map(|ep| {
        let satobs = ep.sats.iter().find(|s| s.sat == sat);
        let (carrier, code, lli) = match satobs {
            Some(so) => (
                get_obs(so, 'L', freq_no).and_then(|o| o.value),
                get_obs(so, 'C', freq_no).and_then(|o| o.value),
                get_obs(so, 'L', freq_no).and_then(|o| o.lli),
            ),
            None => (None, None, None),
        };
        ArcSample {
            epoch_seconds: ep.epoch_seconds,
            carrier_cycles: carrier,
            code_m: code,
            lli,
            clock_offset: ep.clock_offset,
        }
    }).collect()
}

/// 电离层（几何自由）组合检测：GF = L1 - L2（周），对相邻历元差分。
/// 需要双频；频率缺失时记录拒绝原因。
pub fn detect_iono_combo(
    arc_id: &str, sat: &str,
    samples_f1: &[ArcSample], samples_f2: &[ArcSample],
    params: &DetectorParams,
) -> DetectorOutput {
    let mut out = DetectorOutput { candidates: vec![], rejections: vec![] };
    let mut prev: Option<(f64, f64)> = None; // (epoch, gf)
    for (s1, s2) in samples_f1.iter().zip(samples_f2.iter()) {
        match (s1.carrier_cycles, s2.carrier_cycles) {
            (Some(l1), Some(l2)) => {
                let gf = l1 - l2;
                if let Some((pe, pgf)) = prev {
                    let jump = (gf - pgf).abs();
                    if jump > params.iono_threshold_cycles {
                        out.candidates.push(mk_candidate(
                            arc_id, s1.epoch_seconds, DetectorKind::IonosphereCombo,
                            "cycle_slip", Some(gf - pgf),
                            score(jump, params.iono_threshold_cycles), Some(gf - pgf),
                            serde_json::json!({
                                "basis": "geometry-free carrier combo L1-L2 differenced in time",
                                "prev_epoch": pe, "gf_jump_cycles": gf - pgf,
                                "threshold": params.iono_threshold_cycles,
                                "not_truth": true,
                            }).to_string(),
                        ));
                    }
                }
                prev = Some((s1.epoch_seconds, gf));
            }
            _ => {
                if s1.carrier_cycles.is_some() || s2.carrier_cycles.is_some() {
                    out.rejections.push(serde_json::json!({
                        "detector": "iono_combo", "epoch": s1.epoch_seconds,
                        "reason": "frequency missing: iono combo requires dual-frequency carriers",
                    }));
                }
                prev = None;
            }
        }
    }
    let _ = sat;
    out
}

/// Melbourne-Wübbena 组合：宽巷载波 - 窄巷伪距（周）。
pub fn detect_melbourne_wubbena(
    arc_id: &str, sat: &str,
    samples_f1: &[ArcSample], samples_f2: &[ArcSample],
    params: &DetectorParams,
) -> DetectorOutput {
    let mut out = DetectorOutput { candidates: vec![], rejections: vec![] };
    let f1 = frequency_hz(sat, 1);
    let f2 = frequency_hz(sat, 2);
    let (f1, f2) = match (f1, f2) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            out.rejections.push(serde_json::json!({
                "detector": "melbourne_wubbena",
                "reason": "unknown frequencies for satellite system",
            }));
            return out;
        }
    };
    let lam_w = C_LIGHT / (f1 - f2);
    let mut prev: Option<(f64, f64)> = None;
    for (s1, s2) in samples_f1.iter().zip(samples_f2.iter()) {
        match (s1.carrier_cycles, s2.carrier_cycles, s1.code_m, s2.code_m) {
            (Some(l1), Some(l2), Some(p1), Some(p2)) => {
                let wl = (f1 * l1 - f2 * l2) / (f1 - f2);
                let nl_m = (f1 * p1 + f2 * p2) / (f1 + f2);
                let mw = wl - nl_m / lam_w;
                if let Some((pe, pmw)) = prev {
                    let jump = (mw - pmw).abs();
                    if jump > params.mw_threshold_cycles {
                        out.candidates.push(mk_candidate(
                            arc_id, s1.epoch_seconds, DetectorKind::MelbourneWubbena,
                            "cycle_slip", Some(mw - pmw),
                            score(jump, params.mw_threshold_cycles), Some(mw - pmw),
                            serde_json::json!({
                                "basis": "Melbourne-Wubbena wide-lane minus narrow-lane pseudorange",
                                "prev_epoch": pe, "mw_jump_cycles": mw - pmw,
                                "threshold": params.mw_threshold_cycles,
                                "not_truth": true,
                            }).to_string(),
                        ));
                    }
                }
                prev = Some((s1.epoch_seconds, mw));
            }
            _ => {
                if s1.carrier_cycles.is_some() || s2.carrier_cycles.is_some() {
                    out.rejections.push(serde_json::json!({
                        "detector": "melbourne_wubbena", "epoch": s1.epoch_seconds,
                        "reason": "frequency or pseudorange missing: MW requires dual-frequency carriers+codes",
                    }));
                }
                prev = None;
            }
        }
    }
    out
}

/// 时间差分检测：相邻历元载波差，扣除整机钟跳影响；不等间隔按间隔缩放阈值。
/// 整机钟跳事件单独成候选（kind=clock_jump），不与单星周跳混淆。
pub fn detect_time_difference(
    arc_id: &str, sat: &str, freq_no: u8,
    samples: &[ArcSample],
    clock_jumps: &[ClockJumpEvent],
    params: &DetectorParams,
) -> DetectorOutput {
    let mut out = DetectorOutput { candidates: vec![], rejections: vec![] };
    let wl = match wavelength_m(sat, freq_no) {
        Some(w) => w,
        None => {
            out.rejections.push(serde_json::json!({
                "detector": "time_difference",
                "reason": "unknown wavelength for satellite/frequency",
            }));
            return out;
        }
    };
    let mut prev: Option<ArcSample> = None;
    for s in samples {
        let cur = match s.carrier_cycles {
            Some(c) => c,
            None => {
                if prev.is_some() {
                    out.rejections.push(serde_json::json!({
                        "detector": "time_difference", "epoch": s.epoch_seconds,
                        "reason": "carrier missing at epoch (frequency missing)",
                    }));
                }
                prev = None;
                continue;
            }
        };
        if let Some(p) = &prev {
            let gap = s.epoch_seconds - p.epoch_seconds;
            if gap > params.td_max_gap_seconds {
                out.rejections.push(serde_json::json!({
                    "detector": "time_difference", "epoch": s.epoch_seconds,
                    "reason": format!("irregular epoch gap {:.1}s exceeds {:.1}s; differencing skipped", gap, params.td_max_gap_seconds),
                }));
            } else {
                let pc = p.carrier_cycles.unwrap();
                // 扣除接收机钟差变化（整机钟跳），单位换算：秒 -> 周。
                let dclk_cycles = match (s.clock_offset, p.clock_offset) {
                    (Some(a), Some(b)) => (a - b) * C_LIGHT / wl,
                    _ => 0.0,
                };
                let raw_jump = cur - pc;
                let jump = raw_jump - dclk_cycles;
                let scaled_threshold = params.td_threshold_cycles * (gap / 30.0).max(1.0);
                // 整机钟跳：钟差变化本身显著时，单独给出 clock_jump 候选（与单星周跳分开）。
                if dclk_cycles.abs() > params.td_threshold_cycles {
                    out.candidates.push(mk_candidate(
                        arc_id, s.epoch_seconds, DetectorKind::TimeDifference,
                        "clock_jump", Some(dclk_cycles),
                        score(dclk_cycles.abs(), params.td_threshold_cycles), Some(dclk_cycles),
                        serde_json::json!({
                            "basis": "receiver clock offset changed; whole-receiver clock jump, distinct from per-satellite cycle slip",
                            "clock_jump_cycles": dclk_cycles,
                            "linked_clock_events": clock_jumps.len(),
                            "not_truth": true,
                        }).to_string(),
                    ));
                }
                if jump.abs() > scaled_threshold {
                    // LLI 误报：LLI 置位但差分无跳变时不产生候选；差分有跳变时 LLI 只是佐证。
                    let lli_flag = s.lli.map(|l| l & 1 == 1).unwrap_or(false);
                    out.candidates.push(mk_candidate(
                        arc_id, s.epoch_seconds, DetectorKind::TimeDifference,
                        "cycle_slip", Some(jump.round()),
                        score(jump.abs(), scaled_threshold), Some(jump),
                        serde_json::json!({
                            "basis": "time-differenced carrier minus clock-offset change",
                            "gap_seconds": gap, "raw_jump_cycles": raw_jump,
                            "clock_correction_cycles": dclk_cycles,
                            "lli_flag_set": lli_flag,
                            "threshold": scaled_threshold,
                            "not_truth": true,
                        }).to_string(),
                    ));
                } else if s.lli.map(|l| l & 1 == 1).unwrap_or(false) {
                    out.rejections.push(serde_json::json!({
                        "detector": "time_difference", "epoch": s.epoch_seconds,
                        "reason": "LLI flag set but no carrier jump: treated as possible LLI false alarm",
                    }));
                }
            }
        }
        prev = Some(s.clone());
    }
    out
}

fn score(jump: f64, threshold: f64) -> f64 {
    (jump / threshold).min(10.0)
}

fn mk_candidate(
    arc_id: &str, epoch: f64, det: DetectorKind, kind: &str,
    slip: Option<f64>, sc: f64, residual: Option<f64>, detail: String,
) -> Candidate {
    let id = ids::stable_id("cand", &[
        arc_id, &format!("{:.3}", epoch), det.as_str(), kind,
        &slip.map(|v| format!("{:.4}", v)).unwrap_or_else(|| "unknown".into()),
    ]);
    Candidate {
        candidate_id: id,
        arc_id: arc_id.to_string(),
        epoch_seconds: epoch,
        detector: det,
        kind: kind.to_string(),
        slip_cycles: slip,
        score: sc,
        residual,
        status: "open".to_string(),
        detail,
    }
}
