//! Domain analysis engine (pure, deterministic).
//!
//! Three independent detectors propose cycle-slip candidates; no single
//! detector is treated as ground truth:
//!   * GF  - geometry-free / ionosphere combination   (L1 - (f1/f2)L2)
//!   * MW  - Melbourne-Wubbena wide lane
//!   * TD  - time-difference phase vs Doppler prediction
//! Receiver clock jumps (common to all satellites) are separated from
//! single-satellite slips. Repair lives only in an offset layer; the raw
//! observations and detector residuals are never rewritten.

use crate::model::{
    parse_time, Band, CandidateStatus, ClockJumpEvent, DetectorName, EpochRecord, Quantity,
    SatObs, C_LIGHT_M_S, L1_HZ, L2_HZ,
};
use crate::params::Params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlipPair {
    pub n1: i64,
    pub n2: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorEvidence {
    pub detector: String,
    /// Residual magnitude in the detector's own units (cycles / wide-lane
    /// cycles), null when the detector could not evaluate this edge.
    pub residual: Option<f64>,
    pub unit: String,
    /// Best integer pair (or components) this detector proposes, if any.
    pub pair: Option<SlipPair>,
    pub verdict: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub pair: SlipPair,
    pub score: f64,
    /// Per-detector support/rejection evidence keyed GF/MW/TD/LLI.
    pub evidence: Vec<DetectorEvidence>,
    pub reject_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub project_id: String,
    pub input_version_id: String,
    pub rule_id: String,
    pub result_id: String,
    pub sat: String,
    pub time: String,
    pub prev_time: String,
    pub status: String,
    /// All legal hypotheses retained, including the non-adopted ones.
    pub hypotheses: Vec<Hypothesis>,
    /// Reasons this edge cannot be uniquely/fully decided.
    pub unknown: Vec<String>,
    pub clock_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockFinding {
    pub id: String,
    pub time: String,
    pub prev_time: String,
    pub jump_s: f64,
    pub inferred_l1_cycles: f64,
    pub source: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcRec {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub start_time: String,
    pub end_time: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorRow {
    pub id: String,
    pub detector: String,
    pub sat: String,
    pub band: String,
    pub time: String,
    pub prev_time: String,
    pub residual: Option<f64>,
    pub unit: String,
    pub verdict: String,
    pub detail: String,
    pub source_epoch_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisOutput {
    pub result_id: String,
    pub candidates: Vec<Candidate>,
    pub clocks: Vec<ClockFinding>,
    pub arcs: Vec<ArcRec>,
    pub detector_rows: Vec<DetectorRow>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct BandObs {
    code_m: Option<f64>,
    phase_cycles: Option<f64>,
    doppler_hz: Option<f64>,
    lli: String,
}

#[derive(Debug, Clone)]
struct EpochView {
    time: String,
    t: i64,
    sats: BTreeMap<String, [BandObs; 2]>,
    clock_events: Vec<ClockJumpEvent>,
    epoch_id: String,
}

#[derive(Debug, Clone, Default)]
struct MwState {
    mean: Option<f64>,
    count: usize,
}

fn qval(q: &Quantity) -> Option<f64> {
    q.value
}

fn band_obs(so: &SatObs, band: Band) -> BandObs {
    let idx = match band {
        Band::L1 => 0,
        Band::L2 => 1,
    };
    let mut out = BandObs {
        code_m: None,
        phase_cycles: None,
        doppler_hz: None,
        lli: String::new(),
    };
    for o in &so.obs {
        if o.band.as_deref() == Some(match band {
            Band::L1 => "L1",
            Band::L2 => "L2",
        }) {
            // First observation on a band wins; duplicates were rejected earlier.
            out.code_m = qval(&o.pseudorange_m);
            out.phase_cycles = qval(&o.carrier_cycles);
            out.doppler_hz = qval(&o.doppler_hz);
            out.lli = o.lli.clone();
        }
        let _ = idx;
    }
    out
}

fn epoch_id(project_id: &str, time: &str) -> String {
    crate::id::id("ep", &[project_id, time])
}

fn detect(project_id: &str, input_version_id: &str, rule_id: &str,
    result_id: &str, epochs: &[EpochRecord], params: &Params) -> AnalysisOutput {
    let mut views: Vec<EpochView> = Vec::new();
    let mut diagnostics = Vec::new();
    for e in epochs {
        let t = parse_time(&e.time).unwrap_or(0);
        let mut sats = BTreeMap::new();
        for so in &e.sats {
            sats.insert(
                so.sat.clone(),
                [band_obs(so, Band::L1), band_obs(so, Band::L2)],
            );
        }
        views.push(EpochView {
            time: e.time.clone(),
            t,
            sats,
            clock_events: e.clock_events.clone(),
            epoch_id: epoch_id(project_id, &e.time),
        });
    }

    let ratio = L1_HZ / L2_HZ; // ~1.28333
    let wl_wavelength = C_LIGHT_M_S / (L1_HZ - L2_HZ);
    let nl_factor = (L1_HZ - L2_HZ) / L2_HZ;

    let mut clocks: Vec<ClockFinding> = Vec::new();
    let mut detector_rows: Vec<DetectorRow> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();

    let mut mw: BTreeMap<String, MwState> = BTreeMap::new();

    for i in 1..views.len() {
        let prev = &views[i - 1];
        let cur = &views[i];
        let dt = (cur.t - prev.t) as f64;
        let gap = dt > params.gap_tolerance_s + 1e-9;

        // ---- receiver clock jump (stated event + common-mode TD check) ----
        let stated = cur.clock_events.iter().map(|c| c.jump_s).next();
        let clock_id = stated.map(|_| {
            crate::id::id("clk", &[project_id, &cur.time])
        });
        if let Some(jump_s) = stated {
            clocks.push(ClockFinding {
                id: crate::id::id("clk", &[project_id, &cur.time]),
                time: cur.time.clone(),
                prev_time: prev.time.clone(),
                jump_s,
                inferred_l1_cycles: jump_s * L1_HZ,
                source: cur.clock_events[0].source.clone(),
                note: cur.clock_events[0].note.clone(),
            });
        }

        // Common-mode TD estimate across sats present both epochs (L1 cycles),
        // excluding edges with missing phase.
        let mut common: Vec<f64> = Vec::new();
        let mut td_common: BTreeMap<String, f64> = BTreeMap::new();
        for (sat, cur_b) in &cur.sats {
            if let Some(prev_b) = prev.sats.get(sat) {
                if let (Some(lp), Some(cp), Some(dop)) = (
                    prev_b[0].phase_cycles,
                    cur_b[0].phase_cycles,
                    cur_b[0].doppler_hz,
                ) {
                    let predicted = lp - dop * dt;
                    let meas = cp;
                    td_common.insert(sat.clone(), meas - predicted);
                    common.push(meas - predicted);
                }
            }
        }
        let common_shift = if common.len() >= params.clock_min_sats {
            let mut sorted = common.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Some(sorted[sorted.len() / 2])
        } else {
            None
        };
        let clock_mode: Option<f64> = match stated {
            Some(j) => Some(j * L1_HZ),
            None => common_shift.filter(|m| m.abs() >= params.clock_common_fraction),
        };

        let sat_set: BTreeSet<String> = cur
            .sats
            .keys()
            .chain(prev.sats.keys())
            .cloned()
            .collect();
        for sat in &sat_set {
            let cur_b = cur.sats.get(sat);
            let prev_b = prev.sats.get(sat);
            let both = match (prev_b, cur_b) {
                (Some(p), Some(c)) => Some((p, c)),
                _ => None,
            };
            let mut ev: Vec<DetectorEvidence> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            let mut rows: Vec<DetectorRow> = Vec::new();
            let mut td_pair: Option<SlipPair> = None;
            let mut gf_pair: Option<SlipPair> = None;
            let mut mw_pair: Option<SlipPair> = None;
            let mut lli_flag_l1 = false;
            let mut lli_flag_l2 = false;
            let mut any_detector = false;
            let mut clock_explained = false;

            if let Some((p, c)) = both {
                // ---------- TD ----------
                any_detector = true;
                let mut td_components: [Option<f64>; 2] = [None, None];
                let freqs = [L1_HZ, L2_HZ];
                for bi in 0..2 {
                    let band = if bi == 0 { Band::L1 } else { Band::L2 };
                    if let (Some(lp), Some(cp), Some(dop)) =
                        (p[bi].phase_cycles, c[bi].phase_cycles, c[bi].doppler_hz)
                    {
                        let predicted = lp - dop * dt;
                        let raw_delta = cp - predicted;
                        let common_cycles = clock_mode.unwrap_or(0.0) * freqs[bi] / L1_HZ;
                        let residual_band = raw_delta - common_cycles;
                        td_components[bi] = Some(residual_band);
                        let over = residual_band.abs() > params.td_threshold_cycles;
                        if over && clock_mode.is_some() && stated.is_none() {
                            // common-mode clock removed; remaining per-band term
                        }
                        rows.push(DetectorRow {
                            id: crate::id::id("det", &[
                                project_id, "TD", sat, &cur.time, band.code(),
                            ]),
                            detector: "TD".into(),
                            sat: sat.clone(),
                            band: band.code().into(),
                            time: cur.time.clone(),
                            prev_time: prev.time.clone(),
                            residual: Some(residual_band),
                            unit: "cycle".into(),
                            verdict: if over { "slip".into() } else { "ok".into() },
                            detail: format!(
                                "phase-Doppler edge {}s, common-mode removed = {:.4} L1 cycles",
                                dt,
                                clock_mode.unwrap_or(0.0)
                            ),
                            source_epoch_ids: vec![prev.epoch_id.clone(), cur.epoch_id.clone()],
                        });
                    } else {
                        rows.push(DetectorRow {
                            id: crate::id::id("det", &[
                                project_id, "TD", sat, &cur.time,
                                if bi == 0 { "L1" } else { "L2" },
                            ]),
                            detector: "TD".into(),
                            sat: sat.clone(),
                            band: if bi == 0 { "L1" } else { "L2" }.into(),
                            time: cur.time.clone(),
                            prev_time: prev.time.clone(),
                            residual: None,
                            unit: "cycle".into(),
                            verdict: "unknown".into(),
                            detail: "phase or Doppler missing; TD cannot evaluate".into(),
                            source_epoch_ids: vec![prev.epoch_id.clone(), cur.epoch_id.clone()],
                        });
                        unknown.push(format!(
                            "TD unavailable on {} (missing phase or Doppler)",
                            if bi == 0 { "L1" } else { "L2" }
                        ));
                    }
                }
                let n1 = td_components[0].map(|v| v.round() as i64);
                let n2 = td_components[1].map(|v| v.round() as i64);
                if let (Some(a), Some(b)) = (n1, n2) {
                    td_pair = Some(SlipPair { n1: a, n2: b });
                }
                ev.push(DetectorEvidence {
                    detector: "TD".into(),
                    residual: td_components[0],
                    unit: "cycle(L1)".into(),
                    pair: td_pair.clone(),
                    verdict: match td_pair {
                        Some(pr) if pr.n1 != 0 || pr.n2 != 0 => "slip".into(),
                        Some(_) => "ok".into(),
                        None => "unknown".into(),
                    },
                    note: "time-difference carrier vs Doppler prediction".into(),
                });
                if clock_mode.is_some()
                    && td_components.iter().all(|x| x.map(|v| v.abs() <= params.td_threshold_cycles).unwrap_or(true))
                    && stated.is_none()
                {
                    clock_explained = true;
                }

                // ---------- LLI (hint only, never ground truth) ----------
                lli_flag_l1 = c[0].lli.contains('1') || c[0].lli.contains('2');
                lli_flag_l2 = c[1].lli.contains('1') || c[1].lli.contains('2');
                if lli_flag_l1 || lli_flag_l2 {
                    ev.push(DetectorEvidence {
                        detector: "LLI".into(),
                        residual: None,
                        unit: "flag".into(),
                        pair: None,
                        verdict: "flagged".into(),
                        note: format!(
                            "LLI bit set (L1={:?}, L2={:?}); hint only, may be a false alarm",
                            c[0].lli, c[1].lli
                        ),
                    });
                    rows.push(DetectorRow {
                        id: crate::id::id("det", &[project_id, "LLI", sat, &cur.time, "ALL"]),
                        detector: "LLI".into(),
                        sat: sat.clone(),
                        band: "L1/L2".into(),
                        time: cur.time.clone(),
                        prev_time: prev.time.clone(),
                        residual: None,
                        unit: "flag".into(),
                        verdict: "flagged".into(),
                        detail: format!("LLI L1={:?} L2={:?}", c[0].lli, c[1].lli),
                        source_epoch_ids: vec![cur.epoch_id.clone()],
                    });
                }

                // ---------- GF (L1 - ratio*L2 geometry-free ionosphere) ----------
                let gf_values = |b: &[BandObs; 2]| -> Option<f64> {
                    match (b[0].phase_cycles, b[1].phase_cycles) {
                        (Some(l1), Some(l2)) => Some(l1 - ratio * l2),
                        _ => None,
                    }
                };
                if let (Some(g_prev), Some(g_cur)) = (gf_values(p), gf_values(c)) {
                    let jump = g_cur - g_prev;
                    let over = jump.abs() > params.gf_threshold_cycles;
                    let nearest = jump.round() as i64;
                    if over {
                        // every (n1, n2) with n1 - ratio*n2 = jump is compatible;
                        // enumerate around the nearest L1 fit. Equal ties are
                        // retained explicitly (no detector is ground truth).
                        gf_pair = Some(SlipPair { n1: nearest, n2: 0 });
                    }
                    rows.push(DetectorRow {
                        id: crate::id::id("det", &[project_id, "GF", sat, &cur.time, "L1L2"]),
                        detector: "GF".into(),
                        sat: sat.clone(),
                        band: "L1/L2".into(),
                        time: cur.time.clone(),
                        prev_time: prev.time.clone(),
                        residual: Some(jump),
                        unit: "cycle".into(),
                        verdict: if over { "jump" } else { "ok" }.into(),
                        detail: format!(
                            "geometry-free ionosphere combination jump = {j_prev:.4}->{j_cur:.4}",
                            j_prev = g_prev,
                            j_cur = g_cur
                        ),
                        source_epoch_ids: vec![prev.epoch_id.clone(), cur.epoch_id.clone()],
                    });
                    ev.push(DetectorEvidence {
                        detector: "GF".into(),
                        residual: Some(jump),
                        unit: "cycle".into(),
                        pair: gf_pair.clone(),
                        verdict: if over { "jump" } else { "ok" }.into(),
                        note: "only the n1 - (f1/f2)n2 combination is observed".into(),
                    });
                } else {
                    rows.push(DetectorRow {
                        id: crate::id::id("det", &[project_id, "GF", sat, &cur.time, "L1L2"]),
                        detector: "GF".into(),
                        sat: sat.clone(),
                        band: "L1/L2".into(),
                        time: cur.time.clone(),
                        prev_time: prev.time.clone(),
                        residual: None,
                        unit: "cycle".into(),
                        verdict: "unknown".into(),
                        detail: "L1 or L2 carrier missing on an edge; GF unavailable".into(),
                        source_epoch_ids: vec![prev.epoch_id.clone(), cur.epoch_id.clone()],
                    });
                    ev.push(DetectorEvidence {
                        detector: "GF".into(),
                        residual: None,
                        unit: "cycle".into(),
                        pair: None,
                        verdict: "unknown".into(),
                        note: "frequency missing on edge; GF gives no constraint".into(),
                    });
                    unknown.push("GF unavailable (L1/L2 carrier missing on edge)".into());
                }

                // ---------- MW wide lane ----------
                let mw_value = |b: &[BandObs; 2]| -> Option<f64> {
                    match (
                        b[0].phase_cycles,
                        b[1].phase_cycles,
                        b[0].code_m,
                        b[1].code_m,
                    ) {
                        (Some(p1), Some(p2), Some(c1), Some(c2)) => {
                            let phase_wl_m = wl_wavelength * (p1 - p2);
                            let code_nl_m = nl_factor * c1 + (1.0 - nl_factor) * c2;
                            Some((phase_wl_m - code_nl_m) / wl_wavelength)
                        }
                        _ => None,
                    }
                };
                let mw_prev_v = mw_value(p);
                let mw_cur_v = mw_value(c);
                if let (Some(v_prev), Some(v_cur)) = (mw_prev_v, mw_cur_v) {
                    let state = mw.entry(sat.clone()).or_insert_with(MwState::default);
                    let base = state.mean.unwrap_or(v_prev);
                    let jump = v_cur - base;
                    let over = jump.abs() > params.mw_jump_tol_cycles;
                    let nw = jump.round() as i64;
                    if over {
                        mw_pair = Some(SlipPair { n1: nw, n2: 0 });
                        // reset running mean after the break
                        state.mean = Some(v_cur);
                        state.count = 1;
                    } else {
                        let n = state.count as f64;
                        state.mean = Some((base * n + v_cur) / (n + 1.0));
                        state.count += 1;
                    }
                    rows.push(DetectorRow {
                        id: crate::id::id("det", &[project_id, "MW", sat, &cur.time, "WL"]),
                        detector: "MW".into(),
                        sat: sat.clone(),
                        band: "L1/L2".into(),
                        time: cur.time.clone(),
                        prev_time: prev.time.clone(),
                        residual: Some(jump),
                        unit: "wide-lane-cycle".into(),
                        verdict: if over { "jump" } else { "ok" }.into(),
                        detail: format!(
                            "Melbourne-Wubbena Nw estimate {nw}, running mean before edge"
                        ),
                        source_epoch_ids: vec![prev.epoch_id.clone(), cur.epoch_id.clone()],
                    });
                    ev.push(DetectorEvidence {
                        detector: "MW".into(),
                        residual: Some(jump),
                        unit: "wide-lane-cycle".into(),
                        pair: mw_pair.clone(),
                        verdict: if over { "jump" } else { "ok" }.into(),
                        note: "only n1-n2 wide-lane ambiguity is observed".into(),
                    });
                } else {
                    rows.push(DetectorRow {
                        id: crate::id::id("det", &[project_id, "MW", sat, &cur.time, "WL"]),
                        detector: "MW".into(),
                        sat: sat.clone(),
                        band: "L1/L2".into(),
                        time: cur.time.clone(),
                        prev_time: prev.time.clone(),
                        residual: None,
                        unit: "wide-lane-cycle".into(),
                        verdict: "unknown".into(),
                        detail: "code or phase missing; MW unavailable".into(),
                        source_epoch_ids: vec![prev.epoch_id.clone(), cur.epoch_id.clone()],
                    });
                    ev.push(DetectorEvidence {
                        detector: "MW".into(),
                        residual: None,
                        unit: "wide-lane-cycle".into(),
                        pair: None,
                        verdict: "unknown".into(),
                        note: "MW unavailable (missing pseudorange or carrier)".into(),
                    });
                    unknown.push("MW unavailable (missing pseudorange or carrier)".into());
                }
            } else if let Some(c) = cur_b {
                // satellite appeared at this epoch: an arc starts, no edge slip
                let _ = c;
                unknown.push("satellite appears at this epoch (new arc, no preceding edge)".into());
            } else if let Some(p) = prev_b {
                let _ = p;
                unknown.push("satellite observation missing at this epoch (frequency/arc gap)".into());
            }

            let gf_jump = ev
                .iter()
                .find(|e| e.detector == "GF")
                .and_then(|e| e.residual);
            let gf_state = ev
                .iter()
                .find(|e| e.detector == "GF")
                .map(|e| e.verdict.as_str())
                .unwrap_or("unknown");
            let mw_jump = ev
                .iter()
                .find(|e| e.detector == "MW")
                .and_then(|e| e.residual);
            let mw_state = ev
                .iter()
                .find(|e| e.detector == "MW")
                .map(|e| e.verdict.as_str())
                .unwrap_or("unknown");
            let td_verdict = ev
                .iter()
                .find(|e| e.detector == "TD")
                .map(|e| e.verdict.clone())
                .unwrap_or_else(|| "unknown".into());

            let interesting = gf_state == "jump"
                || mw_state == "jump"
                || td_verdict == "slip"
                || lli_flag_l1
                || lli_flag_l2;
            if interesting && !clock_explained {
                if gap {
                    unknown.push(format!(
                        "unequal epoch spacing: edge spans {dt}s (> {}s); detector values degraded",
                        params.gap_tolerance_s
                    ));
                }

                // Always include the no-slip hypothesis for human negation.
                let mut raw_pairs: Vec<SlipPair> = vec![SlipPair { n1: 0, n2: 0 }];
                if let Some(j) = gf_jump {
                    if gf_state == "jump" {
                        // all integer pairs dividing the same GF jump:
                        // n1 - (f1/f2)n2 = j  -> retained as equal legal options
                        for n2 in -params.equal_pair_bound..=params.equal_pair_bound {
                            let n1 = (j + ratio * n2 as f64).round() as i64;
                            let pr = SlipPair { n1, n2 };
                            if !raw_pairs.contains(&pr) {
                                raw_pairs.push(pr);
                            }
                        }
                    }
                } else if let Some(p) = td_pair.clone() {
                    if !raw_pairs.contains(&p) {
                        raw_pairs.push(p);
                    }
                }

                let mw_round = mw_jump.filter(|_| mw_state == "jump").map(|j| j.round() as i64);
                let mut hypotheses: Vec<Hypothesis> = Vec::new();
                for pr in &raw_pairs {
                    let mut score = 0.0;
                    let mut reasons: Vec<String> = Vec::new();
                    if let Some(j) = gf_jump {
                        let pred = pr.n1 as f64 - ratio * pr.n2 as f64;
                        let d = j - pred;
                        if d.abs() <= params.gf_round_tol_cycles {
                            score += 2.0;
                        } else {
                            score -= 2.0;
                            reasons.push(format!(
                                "GF rejects: ionosphere combination mismatch {d:+.3} cycle > {}",
                                params.gf_round_tol_cycles
                            ));
                        }
                    }
                    if let Some(nw) = mw_round {
                        let d = (pr.n1 - pr.n2) - nw;
                        if d.abs() == 0 {
                            score += 2.0;
                        } else {
                            score -= 2.0;
                            reasons.push(format!(
                                "MW rejects: wide-lane difference {} != {nw}",
                                pr.n1 - pr.n2
                            ));
                        }
                    }
                    if let Some(tp) = &td_pair {
                        if tp == pr {
                            score += 2.0;
                        } else {
                            score -= 2.0;
                            reasons.push(format!(
                                "TD rejects: Doppler time-difference gives ({},{})",
                                tp.n1, tp.n2
                            ));
                        }
                    }
                    if (lli_flag_l1 || lli_flag_l2) && !(pr.n1 == 0 && pr.n2 == 0) {
                        score += params.lli_weight;
                    }
                    if pr.n1 == 0 && pr.n2 == 0 && (lli_flag_l1 || lli_flag_l2) {
                        reasons.push("LLI flagged but physical detectors see no slip; possible LLI false alarm".into());
                    }
                    hypotheses.push(Hypothesis {
                        pair: pr.clone(),
                        score,
                        evidence: ev.clone(),
                        reject_reasons: reasons,
                    });
                }
                // stable order: adopted-eligible first, then n1,n2
                hypotheses.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.pair.n1.cmp(&b.pair.n1))
                        .then(a.pair.n2.cmp(&b.pair.n2))
                });
                let top = hypotheses.first().map(|h| h.score).unwrap_or(0.0);
                let ties = hypotheses.iter().filter(|h| h.score == top && h.score > 0.0).count();
                if ties > 1 {
                    unknown.push(format!(
                        "{ties} hypotheses tie at score {top}; no single detector is ground truth - all legal candidates retained"
                    ));
                }
                if gf_state == "unknown" && mw_state == "unknown" && td_verdict == "unknown" {
                    unknown.push("all physical detectors unknown; LLI alone cannot decide".into());
                }

                // Stable identity is the (project, sat, edge) only: the same
                // slip edge keeps one identity across rule revisions even when
                // the top hypothesis changes.
                let cid = crate::id::id("cand", &[project_id, sat, &cur.time]);
                candidates.push(Candidate {
                    id: cid,
                    project_id: project_id.into(),
                    input_version_id: input_version_id.into(),
                    rule_id: rule_id.into(),
                    result_id: result_id.into(),
                    sat: sat.clone(),
                    time: cur.time.clone(),
                    prev_time: prev.time.clone(),
                    status: CandidateStatus::Open.as_str().into(),
                    hypotheses,
                    unknown,
                    clock_event_id: clock_id.clone(),
                });
            }
            detector_rows.extend(rows);
        }
    }

    // dedupe detector rows by stable id (defensive). Arcs are built by the
    // public `analyze` entry once accepted slips are known.
    let mut seen = BTreeSet::new();
    detector_rows.retain(|r| seen.insert(r.id.clone()));

    AnalysisOutput {
        result_id: result_id.into(),
        candidates,
        clocks,
        arcs: Vec::new(),
        detector_rows,
        diagnostics,
    }
}

fn build_arcs(
    project_id: &str,
    views: &[EpochView],
    params: &Params,
    accepted: &BTreeSet<(String, String)>,
) -> Vec<ArcRec> {
    let mut arcs = Vec::new();
    let mut sats: BTreeSet<String> = BTreeSet::new();
    for v in views {
        for s in v.sats.keys() {
            sats.insert(s.clone());
        }
    }
    for sat in &sats {
        for bi in 0..2usize {
            let band = if bi == 0 { Band::L1 } else { Band::L2 };
            let mut start: Option<&EpochView> = None;
            let mut prev_present: Option<&EpochView> = None;
            for v in views {
                let present = v.sats.get(sat).map(|b| b[bi].phase_cycles.is_some()).unwrap_or(false);
                if present {
                    let mut reason = String::new();
                    if start.is_none() {
                        start = Some(v);
                        reason = format!("arc start (first {} observation)", band.code());
                    } else if let Some(pv) = prev_present {
                        let dt = v.t - pv.t;
                        if dt as f64 > params.gap_tolerance_s + 1e-9 {
                            arcs.push(close_arc(project_id, sat, band.code(), start.unwrap(), pv, "unequal epoch spacing: long gap"));
                            start = Some(v);
                        } else if pv.sats.get(sat).map(|b| b[bi].phase_cycles.is_none()).unwrap_or(true) {
                            // shouldn't happen: prev_present tracks presence
                        }
                    }
                    // split after an accepted single-satellite slip at this epoch
                    if let Some(pv) = prev_present {
                        if accepted.contains(&(sat.clone(), v.time.clone())) && start.is_some() {
                            arcs.push(close_arc(project_id, sat, band.code(), start.unwrap(), pv, "accepted single-satellite cycle slip"));
                            start = Some(v);
                        }
                    }
                    prev_present = Some(v);
                    let _ = reason;
                } else if let Some(pv) = prev_present {
                    // frequency missing this epoch: close arc at previous present
                    arcs.push(close_arc(project_id, sat, band.code(), start.unwrap(), pv, "frequency missing: observation gap"));
                    start = None;
                    prev_present = None;
                }
            }
            if let Some(st) = start {
                if let Some(last) = views
                    .iter()
                    .rev()
                    .find(|v| v.sats.get(sat).map(|b| b[bi].phase_cycles.is_some()).unwrap_or(false))
                {
                    arcs.push(close_arc(project_id, sat, band.code(), st, last, "arc end (last observation)"));
                }
            }
        }
    }
    arcs.sort_by(|a, b| {
        a.sat.cmp(&b.sat).then(a.band.cmp(&b.band)).then(a.start_time.cmp(&b.start_time))
    });
    arcs
}

fn close_arc(project_id: &str, sat: &str, band: &str, st: &EpochView, en: &EpochView, reason: &str) -> ArcRec {
    ArcRec {
        id: crate::id::id("arc", &[project_id, sat, band, &st.time, &en.time]),
        sat: sat.into(),
        band: band.into(),
        start_time: st.time.clone(),
        end_time: en.time.clone(),
        reason: reason.into(),
    }
}

/// Pure analysis entry. `accepted_slips` are (sat, time) edges already adopted
/// on this branch; they split arcs. Clock jumps never split arcs.
pub fn analyze(
    project_id: &str,
    input_version_id: &str,
    rule_id: &str,
    epochs: &[EpochRecord],
    params: &Params,
    accepted_slips: &BTreeSet<(String, String)>,
) -> AnalysisOutput {
    let result_id = crate::id::id("res", &[project_id, input_version_id, rule_id, &slip_fp(accepted_slips)]);
    let mut out = detect(project_id, input_version_id, rule_id, &result_id, epochs, params);
    // rebuild arcs with accepted slips applied
    let mut vs = Vec::new();
    for e in epochs {
        let t = parse_time(&e.time).unwrap_or(0);
        let mut sats = BTreeMap::new();
        for so in &e.sats {
            sats.insert(so.sat.clone(), [band_obs(so, Band::L1), band_obs(so, Band::L2)]);
        }
        vs.push(EpochView {
            time: e.time.clone(),
            t,
            sats,
            clock_events: e.clock_events.clone(),
            epoch_id: epoch_id(project_id, &e.time),
        });
    }
    out.arcs = build_arcs(project_id, &vs, params, accepted_slips);
    // statuses reflect current accepted set
    for c in &mut out.candidates {
        if accepted_slips.contains(&(c.sat.clone(), c.time.clone())) {
            c.status = CandidateStatus::Accepted.as_str().into();
        }
    }
    out
}

fn slip_fp(set: &BTreeSet<(String, String)>) -> String {
    let mut s = String::new();
    for (a, b) in set {
        s.push_str(a);
        s.push('=');
        s.push_str(b);
        s.push(';');
    }
    crate::id::content_fp(&s)
}
