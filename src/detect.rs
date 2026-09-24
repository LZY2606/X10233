//! Cycle-slip detectors. Geometry-free (ionosphere) combination,
//! Melbourne-Wuebbena and time-differencing may produce *different*
//! candidates; none of them is treated as ground truth. Whole-receiver clock
//! jumps are separated from single-satellite slips via common-mode analysis.

use crate::model::*;

pub const C_MPS: f64 = 299_792_458.0;

pub fn freq_hz(sys: char, band: &str, channel: Option<i32>) -> Option<f64> {
    match (sys, band) {
        ('G', "L1") | ('E', "L1") => Some(1575.42e6),
        ('G', "L2") => Some(1227.60e6),
        ('G', "L5") | ('E', "L5") => Some(1176.45e6),
        ('R', "L1") => Some(1602.0e6 + channel? as f64 * 0.5625e6),
        ('R', "L2") => Some(1246.0e6 + channel? as f64 * 0.4375e6),
        _ => None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct EpochObs {
    pub epoch_ms: i64,
    pub carrier: [Option<f64>; 2],
    pub pseudo: [Option<f64>; 2],
    pub doppler: [Option<f64>; 2],
    pub lli: [u8; 2],
    pub snr: [Option<f64>; 2],
}

#[derive(Debug, Clone)]
pub struct CandidateDraft {
    pub epoch_ms: i64,
    pub detector: String,
    pub combination: String,
    pub jump: f64,
    pub jump_unit: String,
    pub score: f64,
    pub alternatives: Vec<(i32, i32)>,
    pub status: CandidateStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResidualRow {
    pub epoch_ms: i64,
    pub combination: String,
    pub value: f64,
    pub unit: String,
    pub sigma: f64,
}

#[derive(Debug, Clone)]
pub struct ClockDetection {
    pub epoch_ms: i64,
    pub jump_cycles: f64,
    pub sats: Vec<String>,
}

#[derive(Debug, Default)]
pub struct DetectOutput {
    pub candidates: Vec<CandidateDraft>,
    pub residuals: Vec<ResidualRow>,
    pub clock_detections: Vec<ClockDetection>,
    /// (band, epoch_ms, corrected residual in cycles) after common-mode removal
    pub td_residuals: Vec<(usize, i64, f64)>,
}

/// One satellite's epoch series for one station. `band_of_idx`: 0 -> L1, 1 -> L2.
pub fn detect_satellite(
    sys: char,
    channel: Option<i32>,
    epochs: &[EpochObs],
    params: &Params,
) -> DetectOutput {
    let mut out = DetectOutput::default();
    let f1 = freq_hz(sys, "L1", channel);
    let f2 = freq_hz(sys, "L2", channel);
    let lam = |f: f64| C_MPS / f;

    // --- time-differenced carrier vs Doppler, per band ---------------------
    for b in 0..2 {
        for w in epochs.windows(2) {
            let (a, z) = (&w[0], &w[1]);
            let (p0, p1, d0, d1) = (a.carrier[b], z.carrier[b], a.doppler[b], z.doppler[b]);
            if let (Some(p0), Some(p1), Some(d0), Some(d1)) = (p0, p1, d0, d1) {
                let dt = (z.epoch_ms - a.epoch_ms) as f64 / 1000.0;
                let pred = -0.5 * (d0 + d1) * dt; // cycles
                let resid = (p1 - p0) - pred;
                out.td_residuals.push((b, z.epoch_ms, resid));
                out.residuals.push(ResidualRow {
                    epoch_ms: z.epoch_ms,
                    combination: format!("TD_L{}", b + 1),
                    value: resid,
                    unit: "cycles".into(),
                    sigma: params.td_sigma_cycles,
                });
            }
        }
    }

    // --- geometry-free (ionosphere) carrier combination --------------------
    if let (Some(f1), Some(f2)) = (f1, f2) {
        let (l1, l2) = (lam(f1), lam(f2));
        let gf: Vec<Option<(i64, f64)>> = epochs
            .iter()
            .map(|e| match (e.carrier[0], e.carrier[1]) {
                (Some(p1), Some(p2)) => Some((e.epoch_ms, l1 * p1 - l2 * p2)),
                _ => None,
            })
            .collect();
        for i in 0..gf.len() {
            let Some((t, g)) = gf[i] else { continue };
            // linear extrapolation from the two previous available epochs
            let mut prev: Vec<(i64, f64)> = gf[..i].iter().flatten().cloned().collect();
            if prev.len() < 2 {
                continue;
            }
            let (t1, g1) = prev.pop().unwrap();
            let (t0, g0) = prev.pop().unwrap();
            let dt = (t - t1) as f64 / 1000.0;
            let span = (t1 - t0) as f64 / 1000.0;
            if span <= 0.0 {
                continue;
            }
            let pred = g1 + (g1 - g0) / span * dt;
            let resid = g - pred;
            out.residuals.push(ResidualRow {
                epoch_ms: t,
                combination: "GF_M".into(),
                value: resid,
                unit: "m".into(),
                sigma: params.gf_sigma_m,
            });
            if resid.abs() > params.gf_threshold_m {
                out.candidates.push(CandidateDraft {
                    epoch_ms: t,
                    detector: "gf_iono".into(),
                    combination: "GF(L1-L2)".into(),
                    jump: resid,
                    jump_unit: "m".into(),
                    score: resid.abs() / params.gf_sigma_m,
                    alternatives: vec![],
                    status: CandidateStatus::Proposed,
                    reason: None,
                });
            }
        }
    }

    // --- Melbourne-Wuebbena -------------------------------------------------
    if let (Some(f1), Some(f2)) = (f1, f2) {
        let lam_w = C_MPS / (f1 - f2);
        let mut hist: Vec<f64> = Vec::new();
        for e in epochs {
            let mw = match (e.carrier[0], e.carrier[1], e.pseudo[0], e.pseudo[1]) {
                (Some(p1), Some(p2), Some(c1), Some(c2)) => {
                    Some(p1 - p2 - (f1 * c1 + f2 * c2) / ((f1 + f2) * lam_w))
                }
                _ => None,
            };
            if let Some(m) = mw {
                if !hist.is_empty() {
                    let mean = hist.iter().sum::<f64>() / hist.len() as f64;
                    let resid = m - mean;
                    out.residuals.push(ResidualRow {
                        epoch_ms: e.epoch_ms,
                        combination: "MW".into(),
                        value: resid,
                        unit: "cycles".into(),
                        sigma: params.mw_sigma_cycles,
                    });
                    if resid.abs() > params.mw_threshold_cycles {
                        out.candidates.push(CandidateDraft {
                            epoch_ms: e.epoch_ms,
                            detector: "mw".into(),
                            combination: "MW widelane".into(),
                            jump: resid.round(),
                            jump_unit: "cycles".into(),
                            score: resid.abs() / params.mw_sigma_cycles,
                            alternatives: vec![],
                            status: CandidateStatus::Proposed,
                            reason: None,
                        });
                    }
                }
                hist.push(m);
            }
        }
    }

    // --- LLI flags ----------------------------------------------------------
    for e in epochs {
        for b in 0..2 {
            if e.lli[b] & 1 != 0 {
                out.candidates.push(CandidateDraft {
                    epoch_ms: e.epoch_ms,
                    detector: "lli_flag".into(),
                    combination: format!("LLI_L{}", b + 1),
                    jump: 0.0,
                    jump_unit: "cycles".into(),
                    score: 0.5,
                    alternatives: vec![],
                    status: CandidateStatus::Proposed,
                    reason: None,
                });
            }
        }
    }
    out
}

/// Common-mode clock-jump separation: given per-satellite TD residuals at the
/// same station+epoch, a large consistent median is a receiver clock jump,
/// not a per-satellite slip. Returns (per-epoch common mode, detections).
pub fn common_mode(
    per_sat: &[(&str, &[(usize, i64, f64)])],
    threshold_cycles: f64,
) -> (std::collections::BTreeMap<i64, f64>, Vec<ClockDetection>) {
    use std::collections::BTreeMap;
    let mut by_epoch: BTreeMap<i64, Vec<(String, f64)>> = BTreeMap::new();
    for (sat, res) in per_sat {
        for (b, t, r) in res.iter() {
            if *b == 0 {
                by_epoch.entry(*t).or_default().push((sat.to_string(), *r));
            }
        }
    }
    let mut modes = BTreeMap::new();
    let mut dets = Vec::new();
    for (t, v) in by_epoch {
        if v.len() < 2 {
            continue;
        }
        let mut vals: Vec<f64> = v.iter().map(|(_, r)| *r).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = vals[vals.len() / 2];
        if med.abs() > threshold_cycles {
            modes.insert(t, med);
            dets.push(ClockDetection {
                epoch_ms: t,
                jump_cycles: med,
                sats: v.iter().map(|(s, _)| s.clone()).collect(),
            });
        }
    }
    (modes, dets)
}

/// Joint L1/L2 integer solve from GF (m) and MW (cycles) residuals. Keeps
/// every equally-legal integer pair when the evidence cannot decide.
pub fn joint_solve(
    sys: char,
    channel: Option<i32>,
    gf_resid_m: f64,
    mw_resid_cycles: f64,
    params: &Params,
) -> Option<CandidateDraft> {
    let f1 = freq_hz(sys, "L1", channel)?;
    let f2 = freq_hz(sys, "L2", channel)?;
    let (l1, l2) = (C_MPS / f1, C_MPS / f2);
    let nw = mw_resid_cycles.round() as i32;
    let mut scored: Vec<((i32, i32), f64)> = Vec::new();
    for n1 in -12..=12 {
        let n2 = n1 - nw;
        let s = (l1 * n1 as f64 - l2 * n2 as f64 - gf_resid_m).abs() / params.gf_sigma_m
            + ((n1 - n2) as f64 - mw_resid_cycles).abs() / params.mw_sigma_cycles;
        scored.push(((n1, n2), s));
    }
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let best = scored[0].1;
    let alts: Vec<(i32, i32)> = scored
        .iter()
        .filter(|(_, s)| *s <= best + params.ambiguity_tie_epsilon)
        .map(|(n, _)| *n)
        .collect();
    let (n1, _) = alts[0];
    Some(CandidateDraft {
        epoch_ms: 0, // filled by caller
        detector: "joint_l1l2".into(),
        combination: "GF+MW joint".into(),
        jump: n1 as f64,
        jump_unit: "cycles".into(),
        score: 1.0 / (1.0 + best),
        alternatives: alts.clone(),
        status: if alts.len() > 1 { CandidateStatus::Undetermined } else { CandidateStatus::Proposed },
        reason: if alts.len() > 1 {
            Some(format!("{} equally-legal integer offsets retained", alts.len()))
        } else {
            None
        },
    })
}
