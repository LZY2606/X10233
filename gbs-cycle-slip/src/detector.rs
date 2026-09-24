//! 多检测器候选生成：电离层组合（GF）、Melbourne-Wubbena（MW）、时间差分（TD）。
//!
//! 三个检测器独立给出“支持哪些整周假设”，不以任一检测器当真值；
//! 整机钟跳与单星周跳分开处理。结果中的未知/不可判定是一等状态。

use crate::domain::{BandDef, C_LIGHT, DetectParams};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct BandObs {
    pub lambda_m: f64,
    pub code_m: Option<f64>,
    pub phase_cycles: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub lli: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpochBands {
    pub t: f64,
    pub l1: Option<BandObs>,
    pub l2: Option<BandObs>,
}

impl EpochBands {
    fn band(&self, freq: &str) -> Option<BandObs> {
        match freq {
            "L1" => self.l1,
            "L2" => self.l2,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hypothesis {
    pub n_l1: i64,
    pub n_l2: i64,
    pub score: i64,
    pub gf_residual_cycles: Option<f64>,
    pub mw_residual_cycles: Option<f64>,
    pub td_l1_residual_cycles: Option<f64>,
    pub td_l2_residual_cycles: Option<f64>,
    pub supported_by: Vec<String>,
}

/// 单条整周偏移候选（检测器假设 + 元信息）。
#[derive(Debug, Clone, Serialize)]
pub struct BoundaryCandidate {
    pub station: String,
    pub sat: String,
    pub epoch_from_s: f64,
    pub epoch_to_s: f64,
    pub hypotheses: Vec<Hypothesis>,
    pub tied: bool,
    pub max_score: i64,
    pub gf_status: String,
    pub mw_status: String,
    pub td_status: String,
    pub lli_flagged: bool,
    pub clock_event: bool,
    pub clock_metric: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClockFinding {
    pub epoch_from_s: f64,
    pub epoch_to_s: f64,
    pub fraction: f64,
    pub median_code_shift_m: f64,
    pub median_phase_shift_l1_cycles: f64,
    pub source_event_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundaryReport {
    pub candidates: Vec<BoundaryCandidate>,
    pub clock_findings: Vec<ClockFinding>,
}

fn valid_pair(a: &EpochBands, b: &EpochBands) -> Option<(BandObs, BandObs, BandObs, BandObs)> {
    match (a.l1, a.l2, b.l1, b.l2) {
        (Some(al1), Some(al2), Some(bl1), Some(bl2)) => Some((al1, al2, bl1, bl2)),
        _ => None,
    }
}

fn gf_jump_m(p: (BandObs, BandObs, BandObs, BandObs)) -> Option<f64> {
    let (al1, al2, bl1, bl2) = p;
    match (
        al1.phase_cycles,
        al2.phase_cycles,
        bl1.phase_cycles,
        bl2.phase_cycles,
    ) {
        (Some(pa1), Some(pa2), Some(pb1), Some(pb2)) => {
            let before = pa1 * al1.lambda_m - pa2 * al2.lambda_m;
            let after = pb1 * bl1.lambda_m - pb2 * bl2.lambda_m;
            Some(after - before)
        }
        _ => None,
    }
}

fn wl_narrow_code_m(p: (BandObs, BandObs, BandObs, BandObs)) -> Option<f64> {
    let (al1, al2, _, _) = p;
    match (al1.code_m, al2.code_m) {
        (Some(c1), Some(c2)) => {
            let f1 = C_LIGHT / al1.lambda_m;
            let f2 = C_LIGHT / al2.lambda_m;
            Some((f1 * c1 + f2 * c2) / (f1 + f2))
        }
        _ => None,
    }
}

fn wl_phase_m(p: (BandObs, BandObs, BandObs, BandObs)) -> Option<f64> {
    let (al1, al2, _, _) = p;
    match (al1.phase_cycles, al2.phase_cycles) {
        (Some(p1), Some(p2)) => {
            let f1 = C_LIGHT / al1.lambda_m;
            let f2 = C_LIGHT / al2.lambda_m;
            let wl_lambda = C_LIGHT / (f1 - f2);
            Some((p1 - p2) * wl_lambda)
        }
        _ => None,
    }
}

fn mw_jump_wl_cycles(
    before: (BandObs, BandObs, BandObs, BandObs),
    after: (BandObs, BandObs, BandObs, BandObs),
) -> Option<f64> {
    let wl_lambda = {
        let f1 = C_LIGHT / before.0.lambda_m;
        let f2 = C_LIGHT / before.1.lambda_m;
        C_LIGHT / (f1 - f2)
    };
    let nl_before = wl_narrow_code_m(before)?;
    let wl_before = wl_phase_m(before)?;
    let nl_after = wl_narrow_code_m(after)?;
    let wl_after = wl_phase_m(after)?;
    Some(((wl_after - nl_after) - (wl_before - nl_before)) / wl_lambda)
}

fn td_predicted_cycles(a: BandObs, b: BandObs, dt: f64) -> Option<f64> {
    // 两端 Doppler 取均值做中点估计；任一缺失即该频率 TD 不可判定。
    match (a.phase_cycles, b.phase_cycles, a.doppler_hz, b.doppler_hz) {
        (Some(_), Some(_), Some(d0), Some(d1)) => Some(-0.5 * (d0 + d1) * dt),
        _ => None,
    }
}

fn td_residual_cycles(
    a: BandObs,
    b: BandObs,
    dt: f64,
    n: i64,
) -> Option<f64> {
    let pred = td_predicted_cycles(a, b, dt)?;
    match (a.phase_cycles, b.phase_cycles) {
        (Some(pa), Some(pb)) => Some(pb - pa - pred - n as f64),
        _ => None,
    }
}

/// 整机钟跳检测：码在所有星上同向平移（相位随之非整周平移）。
fn detect_clock(
    epochs: &[(f64, std::collections::HashMap<String, EpochBands>)],
    params: &DetectParams,
    source_clock_ms: Option<f64>,
) -> Vec<ClockFinding> {
    let mut findings = Vec::new();
    for w in epochs.windows(2) {
        let (t0, e0) = &w[0];
        let (t1, e1) = &w[1];
        let mut code_shifts = Vec::new();
        let mut phase_shifts = Vec::new();
        for (sat, b1) in e1.iter() {
            if let Some(b0) = e0.get(sat) {
                if let (Some(x0), Some(x1)) = (b0.l1, b1.l1) {
                    if let (Some(c0), Some(c1), Some(p0), Some(p1)) =
                        (x0.code_m, x1.code_m, x0.phase_cycles, x1.phase_cycles)
                    {
                        let expected_phase = -40.0 / x1.lambda_m * (t1 - t0);
                        code_shifts.push(c1 - c0 - (-40.0) * (t1 - t0));
                        phase_shifts.push(p1 - p0 - expected_phase);
                    }
                }
            }
        }
        let total = code_shifts.len() as i64;
        if total < params.clock_min_sats {
            continue;
        }
        code_shifts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        phase_shifts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_code = code_shifts[code_shifts.len() / 2];
        let median_phase = phase_shifts[phase_shifts.len() / 2];
        let same_sign = code_shifts
            .iter()
            .filter(|v| (*v).signum() == median_code.signum() && v.abs() > 50.0)
            .count() as i64;
        let fraction = same_sign as f64 / total as f64;
        let source_finding = source_clock_ms
            .filter(|ms| (median_code - ms * 1e-3 * C_LIGHT).abs() < 0.1 * C_LIGHT * 1e-3);
        if fraction >= params.clock_consensus_fraction && median_code.abs() > 50.0 {
            findings.push(ClockFinding {
                epoch_from_s: *t0,
                epoch_to_s: *t1,
                fraction,
                median_code_shift_m: median_code,
                median_phase_shift_l1_cycles: median_phase,
                source_event_ms: source_finding,
            });
        }
    }
    findings
}

fn analyze_boundary(
    station: &str,
    sat: &str,
    a: &EpochBands,
    b: &EpochBands,
    params: &DetectParams,
    lli_flagged: bool,
    clock_event: bool,
    clock_metric: Option<f64>,
) -> Option<BoundaryCandidate> {
    let dt = b.t - a.t;
    let pair = match valid_pair(a, b) {
        Some(p) => Some(p),
        None => None,
    };

    // GF（米→L1 周）。
    let gf_jump_cycles = pair
        .and_then(gf_jump_m)
        .map(|m| m / b.l1.unwrap().lambda_m);
    // MW（宽巷周）。
    let mw_jump = pair.and_then(|_| mw_jump_wl_cycles(pair.unwrap(), valid_pair(a, b).unwrap()));

    // 各检测器的可用性是一等状态。
    let gf_status = if pair.and_then(gf_jump_m).is_some() {
        "OK"
    } else {
        "UNKNOWN_FREQUENCY_MISSING"
    };
    let mw_status = if mw_jump.is_some() { "OK" } else { "UNKNOWN_FREQUENCY_MISSING" };
    let td_ok = |freq: &str| -> bool {
        match (a.band(freq), b.band(freq)) {
            (Some(x), Some(y)) => {
                x.phase_cycles.is_some()
                    && y.phase_cycles.is_some()
                    && x.doppler_hz.is_some()
                    && y.doppler_hz.is_some()
            }
            _ => false,
        }
    };
    let (td_l1_ok, td_l2_ok) = (td_ok("L1"), td_ok("L2"));
    let td_status = if td_l1_ok || td_l2_ok {
        "OK"
    } else {
        "UNKNOWN_DOPPLER_OR_PHASE_MISSING"
    };

    let mut hypotheses = Vec::new();
    let win = params.search_half_window;
    for n1 in -win..=win {
        for n2 in -win..=win {
            // GF 残差：GF(after) - GF(before) - (n1*λ1 - n2*λ2)，以 L1 周表示。
            let gf_res = gf_jump_cycles.map(|j| {
                let l1 = b.l1.unwrap().lambda_m;
                let l2 = b.l2.unwrap().lambda_m;
                j - n1 as f64 + (l2 / l1) * n2 as f64
            });
            let mw_res = mw_jump.map(|j| j - (n1 - n2) as f64);
            let td1_res = if td_l1_ok {
                td_residual_cycles(a.l1.unwrap(), b.l1.unwrap(), dt, n1)
            } else {
                None
            };
            let td2_res = if td_l2_ok {
                td_residual_cycles(a.l2.unwrap(), b.l2.unwrap(), dt, n2)
            } else {
                None
            };

            let gf_supports = gf_res
                .map(|r| r.abs() <= params.gf_tol_cycles)
                .unwrap_or(false);
            let mw_supports = mw_res
                .map(|r| r.abs() <= params.mw_tol_cycles)
                .unwrap_or(false);
            let td1_supports = td1_res
                .map(|r| r.abs() <= params.td_tol_cycles)
                .unwrap_or(false);
            let td2_supports = td2_res
                .map(|r| r.abs() <= params.td_tol_cycles)
                .unwrap_or(false);
            // TD 只在可用频率上计票。
            let td_votes = (if td_l1_ok { 1 } else { 0 })
                + (if td_l2_ok { 1 } else { 0 });
            let td_yes = (if td_l1_ok && td1_supports { 1 } else { 0 })
                + (if td_l2_ok && td2_supports { 1 } else { 0 });
            let td_supports = td_votes > 0 && td_yes == td_votes;

            let mut score = 0i64;
            let mut supported_by = Vec::new();
            if gf_supports {
                score += 1;
                supported_by.push("GF".to_string());
            }
            if mw_supports {
                score += 1;
                supported_by.push("MW".to_string());
            }
            if td_votes > 0 {
                if td_supports {
                    score += 1;
                    supported_by.push("TD".to_string());
                }
            }
            // 整周钟跳边界不把候选当成星上周跳；仅保留假设供界面解释。
            if clock_event {
                supported_by.push("RECEIVER_CLOCK_JUMP".to_string());
            }
            // 零假设即使没有检测器支持，也必须保留（“没有跳变”永远是合法解释）。
            let nonzero = n1 != 0 || n2 != 0;
            let keep = (n1 == 0 && n2 == 0)
                || gf_supports
                || mw_supports
                || (td_votes > 0 && td_supports)
                || (clock_event);
            let _ = nonzero;
            if keep {
                hypotheses.push(Hypothesis {
                    n_l1: n1,
                    n_l2: n2,
                    score,
                    gf_residual_cycles: gf_res,
                    mw_residual_cycles: mw_res,
                    td_l1_residual_cycles: td1_res,
                    td_l2_residual_cycles: td2_res,
                    supported_by,
                });
            }
        }
    }

    let max_score = hypotheses.iter().map(|h| h.score).max().unwrap_or(0);
    let winners: Vec<&Hypothesis> = hypotheses.iter().filter(|h| h.score == max_score).collect();
    // “多个等分整周偏移”：最高得分上出现两个或以上不同假设。
    let tied = winners.len() > 1;

    // 只有真有信息可看的边界才输出候选：LLI、钟跳、或任一检测器支持非零假设。
    let nonzero_supported = hypotheses
        .iter()
        .any(|h| (h.n_l1 != 0 || h.n_l2 != 0) && h.score > 0);
    let interesting = lli_flagged || clock_event || nonzero_supported || tied;
    if !interesting {
        return None;
    }

    hypotheses.sort_by(|x, y| {
        y.score
            .cmp(&x.score)
            .then(x.n_l1.cmp(&y.n_l1))
            .then(x.n_l2.cmp(&y.n_l2))
    });

    Some(BoundaryCandidate {
        station: station.to_string(),
        sat: sat.to_string(),
        epoch_from_s: a.t,
        epoch_to_s: b.t,
        hypotheses,
        tied,
        max_score,
        gf_status: gf_status.to_string(),
        mw_status: mw_status.to_string(),
        td_status: td_status.to_string(),
        lli_flagged,
        clock_event,
        clock_metric,
    })
}

/// 对一个站（多星、按时间排序的历元映射）运行全部检测器。
/// `source_clock_ms` 为该站显式 CLOCK 事件（如有）。
pub fn analyze_station(
    station: &str,
    epochs: &[(f64, std::collections::HashMap<String, EpochBands>)],
    params: &DetectParams,
    source_clock_ms: Option<f64>,
) -> BoundaryReport {
    let clocks = detect_clock(epochs, params, source_clock_ms);
    let mut candidates = Vec::new();
    for w in epochs.windows(2) {
        let (t0, e0) = &w[0];
        let (t1, e1) = &w[1];
        let clock = clocks
            .iter()
            .find(|c| c.epoch_from_s == *t0 && c.epoch_to_s == *t1);
        for (sat, b1) in e1.iter() {
            if let Some(b0) = e0.get(sat) {
                let lli = (b0.l1.map(|x| x.lli).unwrap_or(0)
                    | b0.l2.map(|x| x.lli).unwrap_or(0)
                    | b1.l1.map(|x| x.lli).unwrap_or(0)
                    | b1.l2.map(|x| x.lli).unwrap_or(0))
                    != 0;
                if let Some(cand) = analyze_boundary(
                    station,
                    sat,
                    b0,
                    b1,
                    params,
                    lli,
                    clock.is_some(),
                    clock.map(|c| c.median_code_shift_m),
                ) {
                    candidates.push(cand);
                }
            }
        }
    }
    BoundaryReport {
        candidates,
        clock_findings: clocks,
    }
}

/// 供引擎直接组装 EpochBands。
pub fn make_band(band: &BandDef, code: Option<f64>, phase: Option<f64>, doppler: Option<f64>, lli: Option<i64>) -> BandObs {
    BandObs {
        lambda_m: band.wavelength_m,
        code_m: code,
        phase_cycles: phase,
        doppler_hz: doppler,
        lli: lli.unwrap_or(0),
    }
}
