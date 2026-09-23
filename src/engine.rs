//! Pure cycle-slip state machine.
//!
//! Three independent detectors contribute evidence, none is treated as truth:
//!   GF   - geometry-free / ionosphere combination (m)
//!   MW   - Melbourne-Wubbena wide-lane (wide-lane cycles)
//!   TD   - time-difference phase vs Doppler prediction (m), per band
//! Receiver (whole-clock) jumps are a station-scoped finding and are kept
//! separate from single-satellite slips. Repairs exist only in an offset layer;
//! raw carrier phases are never rewritten.

use crate::freq::{self, Band};
use crate::model::DetParams;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

pub const ALGO_VERSION: &str = "detector-core-1.0.0";

#[derive(Debug, Clone)]
pub struct EffObs {
    pub obs_id: String,
    pub commit_id: String,
    pub v: f64,
    pub lli: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SatEpoch {
    pub c1: Option<EffObs>,
    pub c2: Option<EffObs>,
    pub l1: Option<EffObs>,
    pub l2: Option<EffObs>,
    pub d1: Option<EffObs>,
    pub d2: Option<EffObs>,
    pub l1_lli: Option<String>,
    pub l2_lli: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EEpoch {
    pub id: String,
    pub ord: i64,
    pub t_s: f64,
    pub time: String,
    pub sats: BTreeMap<String, SatEpoch>,
}

#[derive(Debug, Clone, Default)]
pub struct SatState {
    pub glo_k: Option<i32>,
}

pub struct EffectiveWorld {
    pub epochs: Vec<EEpoch>,
    pub sat_state: HashMap<String, SatState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FFinding {
    pub scope: String,
    pub sat: Option<String>,
    pub band: Option<String>,
    pub epoch_id: String,
    pub epoch_time: String,
    pub detector: String,
    pub status: String,
    pub qty: Option<f64>,
    pub qty_unit: Option<String>,
    pub n1: Option<i64>,
    pub n2: Option<i64>,
    pub distance: Option<f64>,
    pub message: String,
    /// identity key of a finding, stable across export/import
    pub key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FCandidate {
    pub sat: String,
    pub epoch_id: String,
    pub epoch_time: String,
    pub n1: i64,
    pub n2: i64,
    pub score: f64,
    pub votes: Vec<String>,
    pub clock_explained: bool,
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct ArcSeg {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub start_epoch_id: String,
    pub end_epoch_id: String,
    pub reason_open: String, // start | gap | accepted_slip
    pub parent_decision: Option<String>,
}

pub struct PipelineOut {
    pub findings: Vec<FFinding>,
    pub candidates: Vec<FCandidate>,
    pub arcs: Vec<ArcSeg>,
}

fn fkey(detector: &str, scope: &str, sat: Option<&str>, band: Option<&str>, epoch_id: &str) -> String {
    format!("{}|{}|{}|{}|{}", detector, scope, sat.unwrap_or("-"), band.unwrap_or("-"), epoch_id)
}

fn ckey(sat: &str, epoch_id: &str, n1: i64, n2: i64) -> String {
    format!("cand|{}|{}|{}|{}", sat, epoch_id, n1, n2)
}

/// Convert civil "YYYY-MM-DDTHH:MM:SS" to day-relative seconds. Day boundaries
/// are rare in the lab case; a carry is applied when the day changes.
pub fn civil_to_seconds(times: &[String]) -> Vec<f64> {
    let mut out = Vec::with_capacity(times.len());
    let mut base_day = 0i64;
    let mut prev: Option<i64> = None;
    for t in times {
        let day = t[..10].replace('-', "").parse::<i64>().unwrap_or(0);
        let h: i64 = t[11..13].parse().unwrap_or(0);
        let m: i64 = t[14..16].parse().unwrap_or(0);
        let s: i64 = t[17..19].parse().unwrap_or(0);
        if base_day == 0 { base_day = day; }
        let mut secs = (day - base_day) * 86400 + h * 3600 + m * 60 + s;
        if let Some(p) = prev { if secs < p { secs += 86400; } }
        prev = Some(secs);
        out.push(secs as f64);
    }
    out
}

#[derive(Default)]
struct SatHist {
    // band -> (previous effective observation, epoch seconds)
    l: [Option<(f64, f64, Option<String>)>; 2],
    d: [Option<f64>; 2],
    gf: Option<(f64, f64)>,
    mw: Option<(f64, f64)>,
    gf_window: Vec<(f64, f64)>,
    mw_window: Vec<(f64, f64)>,
}

pub fn run_pipeline(world: &EffectiveWorld, p: &DetParams) -> PipelineOut {
    let mut findings: Vec<FFinding> = Vec::new();
    let mut candidates: Vec<FCandidate> = Vec::new();
    let mut hist: HashMap<String, SatHist> = HashMap::new();
    // Arc bookkeeping: sat|band -> current open (start_epoch_id, end set later)
    let mut open: HashMap<String, (String, String, String, Option<String>)> = HashMap::new();
    let mut arcs: Vec<ArcSeg> = Vec::new();

    for ep in &world.epochs {
        // ---- station-level receiver clock classification (all-satellite common mode)
        detect_receiver_clock(world, ep, p, &mut findings, &hist);

        for (sat, se) in &ep.sats {
            let st = world.sat_state.get(sat).cloned().unwrap_or_default();
            let lam1 = freq::wavelength_m(sat, Band::L1, st.glo_k);
            let lam2 = freq::wavelength_m(sat, Band::L2, st.glo_k);
            let h = hist.entry(sat.clone()).or_default();

            // LLI evidence flags (independent of numeric detectors)
            for (band, lli) in [(Band::L1, &se.l1_lli), (Band::L2, &se.l2_lli)] {
                if let Some(flag) = lli {
                    findings.push(lli_finding(sat, band, ep, flag));
                }
            }

            let mut boundary = false;
            if let (Some(l1), Some(l2), Some(lam1), Some(lam2)) =
                (&se.l1, &se.l2, lam1, lam2) {
                let gf_now = freq::gf_m(l1.v, l2.v, lam1, lam2);
                // GF change with linear ionosphere drift de-trending
                if let Some((prev_gf, prev_t)) = h.gf {
                    let dt = ep.t_s - prev_t;
                    if dt > 0.0 {
                        let slope = iono_slope(&h.gf_window, prev_t);
                        let predicted = prev_gf + slope * dt;
                        let jump = gf_now - predicted;
                        let status = if jump.abs() <= p.gf_tol_m { "ok" } else { "candidate" };
                        findings.push(FFinding {
                            scope: "satellite".into(), sat: Some(sat.clone()),
                            band: Some("L1+L2".into()), epoch_id: ep.id.clone(),
                            epoch_time: ep.time.clone(), detector: "gf".into(),
                            status: status.into(), qty: Some(jump),
                            qty_unit: Some("m".into()), n1: None, n2: None,
                            distance: Some(jump.abs()),
                            message: format!("de-trended GF change over {dt:.0} s"),
                            key: fkey("gf", "satellite", Some(sat), Some("L1+L2"), &ep.id),
                        });
                        if status == "candidate" { boundary = true; }
                    }
                }
                h.gf_window.retain(|(_, t)| ep.t_s - t <= p.iono_drift_est_s);
                h.gf_window.push((gf_now, ep.t_s));
                h.gf = Some((gf_now, ep.t_s));

                // MW wide-lane (needs code on both bands)
                if let (Some(c1), Some(c2)) = (&se.c1, &se.c2) {
                    let mw_now = freq::mw_wl_cycle(l1.v, l2.v, c1.v, c2.v, lam1, lam2);
                    if let Some((prev_mw, _prev_t)) = h.mw {
                        let jump = mw_now - prev_mw;
                        let nearest = jump.round() as i64;
                        let dist = (jump - nearest as f64).abs();
                        let status = if dist <= p.wl_tol_cycle { "candidate" } else { "unknown" };
                        findings.push(FFinding {
                            scope: "satellite".into(), sat: Some(sat.clone()),
                            band: Some("L1-L2(WL)".into()), epoch_id: ep.id.clone(),
                            epoch_time: ep.time.clone(), detector: "mw".into(),
                            status: status.into(), qty: Some(jump),
                            qty_unit: Some("WL-cycle".into()),
                            n1: if status == "candidate" { Some(nearest) } else { None },
                            n2: Some(0), distance: Some(dist),
                            message: "MW wide-lane change constrains n1-n2".into(),
                            key: fkey("mw", "satellite", Some(sat), Some("L1-L2(WL)"), &ep.id),
                        });
                        if status == "candidate" { boundary = true; }
                    }
                    h.mw_window.push((mw_now, ep.t_s));
                    h.mw = Some((mw_now, ep.t_s));
                }
            } else {
                // Missing required frequency => unknown, not a silent skip.
                let missing = if lam1.is_none() || lam2.is_none() {
                    "frequency datum unknown (GLONASS frequency number missing)"
                } else { "a required L1/L2 phase observation is absent" };
                findings.push(FFinding {
                    scope: "satellite".into(), sat: Some(sat.clone()),
                    band: Some("L1/L2".into()), epoch_id: ep.id.clone(),
                    epoch_time: ep.time.clone(), detector: "gf".into(),
                    status: "unknown".into(), qty: None, qty_unit: None,
                    n1: None, n2: None, distance: None,
                    message: format!("GF/MW not evaluable: {missing}"),
                    key: fkey("gf", "satellite", Some(sat), Some("L1/L2"), &ep.id),
                });
            }

            // Time-difference detector per band: carrier change vs Doppler.
            for (bi, band) in [Band::L1, Band::L2].into_iter().enumerate() {
                let lam = if bi == 0 { lam1 } else { lam2 };
                let lnow = if bi == 0 { se.l1.as_ref() } else { se.l2.as_ref() };
                let dnow = if bi == 0 { se.d1.as_ref() } else { se.d2.as_ref() };
                let (Some(lnow), Some(lam)) = (lnow, lam) else {
                    if lnow.is_some() && lam.is_none() {
                        findings.push(FFinding {
                            scope: "satellite".into(), sat: Some(sat.clone()),
                            band: Some(freq::band_name(band).into()), epoch_id: ep.id.clone(),
                            epoch_time: ep.time.clone(), detector: "td".into(),
                            status: "unknown".into(), qty: None, qty_unit: None,
                            n1: None, n2: None, distance: None,
                            message: "wavelength unknown; TD cannot be evaluated".into(),
                            key: fkey("td", "satellite", Some(sat), Some(freq::band_name(band)), &ep.id),
                        });
                    }
                    continue;
                };
                if let Some((prev_l, prev_t, _prev_lli)) = h.l[bi] {
                    let dt = ep.t_s - prev_t;
                    if dt > 0.0 {
                        let pred = match (h.d[bi], dnow) {
                            (Some(d0), Some(d1)) =>
                                Some(freq::doppler_predicted_m(d0, d1.v, dt, lam)),
                            _ => None,
                        };
                        let actual = (lnow.v - prev_l) * lam;
                        match pred {
                            Some(pred) => {
                                let res = pred - actual;
                                let nearest = (res / lam).round() as i64;
                                let dist_m = (res - nearest as f64 * lam).abs();
                                let status = if dist_m <= p.td_sigma_m { "ok" } else { "candidate" };
                                findings.push(FFinding {
                                    scope: "satellite".into(), sat: Some(sat.clone()),
                                    band: Some(freq::band_name(band).into()),
                                    epoch_id: ep.id.clone(), epoch_time: ep.time.clone(),
                                    detector: "td".into(), status: status.into(),
                                    qty: Some(res), qty_unit: Some("m".into()),
                                    n1: if status == "candidate" {
                                        Some(if bi == 0 { nearest } else { 0 })
                                    } else { None },
                                    n2: if status == "candidate" {
                                        Some(if bi == 1 { nearest } else { 0 })
                                    } else { None },
                                    distance: Some(dist_m),
                                    message: format!("carrier change minus Doppler prediction, dt={dt:.0}s"),
                                    key: fkey("td", "satellite", Some(sat),
                                        Some(freq::band_name(band)), &ep.id),
                                });
                                if status == "candidate" { boundary = true; }
                            }
                            None => findings.push(FFinding {
                                scope: "satellite".into(), sat: Some(sat.clone()),
                                band: Some(freq::band_name(band).into()),
                                epoch_id: ep.id.clone(), epoch_time: ep.time.clone(),
                                detector: "td".into(), status: "unknown".into(),
                                qty: None, qty_unit: None, n1: None, n2: None,
                                distance: None,
                                message: "Doppler missing at an adjacent epoch; TD unknown".into(),
                                key: fkey("td", "satellite", Some(sat),
                                    Some(freq::band_name(band)), &ep.id),
                            }),
                        }
                    }
                }
                h.l[bi] = Some((lnow.v, ep.t_s, lnow.lli.clone()));
                h.d[bi] = dnow.map(|d| d.v);
            }

            // ---- arc segments (gap-aware) for both bands
            for band in [Band::L1, Band::L2] {
                let has = match band {
                    Band::L1 => se.l1.is_some(),
                    Band::L2 => se.l2.is_some(),
                };
                if has {
                    let bname = freq::band_name(band).to_string();
                    let key = format!("{}|{}", sat, bname);
                    let need_gap = match open.get(&key) {
                        Some((_, last_id, _, _)) => {
                            let last_t = world.epochs.iter().find(|e| &e.id == last_id)
                                .map(|e| e.t_s).unwrap_or(ep.t_s);
                            ep.t_s - last_t > p.arc_gap_s
                        }
                        None => true,
                    };
                    if need_gap {
                        open.insert(key.clone(),
                            (ep.id.clone(), ep.id.clone(), "gap".into(), None));
                    } else if let Some((_, last_id, _, _)) = open.get_mut(&key) {
                        *last_id = ep.id.clone();
                    }
                }
            }

            if boundary {
                emit_candidates(sat, ep, &lam1_in(lam1), &lam2_in(lam2), p,
                    &findings, &mut candidates);
            }
        }
    }

    // close arcs
    for (key, (start, end, reason, dec)) in open.drain() {
        let (sat, band) = key.split_once('|').unwrap();
        arcs.push(ArcSeg {
            id: crate::ids::id("arc", &[sat, band, &start, "gap-start"]),
            sat: sat.into(), band: band.into(), start_epoch_id: start, end_epoch_id: end,
            reason_open: reason, parent_decision: dec,
        });
    }
    PipelineOut { findings, candidates, arcs }
}

fn lam1_in(x: Option<f64>) -> Option<f64> { x }
fn lam2_in(x: Option<f64>) -> Option<f64> { x }

fn iono_slope(window: &[(f64, f64)], now: f64) -> f64 {
    let pts: Vec<&(f64, f64)> = window.iter().filter(|(_, t)| (now - t).abs() > 0.5).collect();
    if pts.is_empty() { return 0.0; }
    let n = pts.len() as f64;
    let mt: f64 = pts.iter().map(|(_, t)| *t).sum::<f64>() / n;
    let mg: f64 = pts.iter().map(|(g, _)| *g).sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (g, t) in pts {
        num += (t - mt) * (g - mg);
        den += (t - mt).powi(2);
    }
    if den > 1e-12 { num / den } else { 0.0 }
}

fn lli_finding(sat: &str, band: Band, ep: &EEpoch, flag: &str) -> FFinding {
    let bits: Vec<&str> = flag.chars().filter_map(|c| match c {
        '1' => Some("1=possible-slip(LLI)"),
        '2' => Some("2=OW-slip(LLI)"),
        '3' => Some("3=LLI-1-and-2"),
        '4' => Some("4=half-cycle-not-resolved"),
        _ => None,
    }).collect();
    let suspected = flag.chars().any(|c| c == '1' || c == '2' || c == '3');
    FFinding {
        scope: "satellite".into(), sat: Some(sat.into()),
        band: Some(freq::band_name(band).into()), epoch_id: ep.id.clone(),
        epoch_time: ep.time.clone(), detector: "lli".into(),
        status: if suspected { "evidence" } else { "ok" }.into(),
        qty: None, qty_unit: Some("RINEX-LLI-flag".into()),
        n1: None, n2: None, distance: None,
        message: format!("LLI flag {:?} preserved as evidence: {}", flag, bits.join(",")),
        key: fkey("lli", "satellite", Some(sat), Some(freq::band_name(band)), &ep.id),
    }
}

fn detect_receiver_clock(
    world: &EffectiveWorld,
    ep: &EEpoch,
    p: &DetParams,
    out: &mut Vec<FFinding>,
    _hist: &HashMap<String, SatHist>,
) {
    let idx = match world.epochs.iter().position(|e| e.id == ep.id) {
        Some(i) if i > 0 => i - 1,
        _ => return,
    };
    let prev = &world.epochs[idx];
    let dt = ep.t_s - prev.t_s;
    if dt <= 0.0 { return; }
    let mut jumps: Vec<(String, f64)> = Vec::new();
    for (sat, se) in &ep.sats {
        let st = world.sat_state.get(sat).cloned().unwrap_or_default();
        let Some(lam1) = freq::wavelength_m(sat, Band::L1, st.glo_k) else { continue; };
        let (Some(now), Some(pv)) = (se.l1.as_ref(), prev.sats.get(sat).and_then(|x| x.l1.as_ref())) else { continue; };
        let (Some(dn), Some(dp)) = (se.d1.as_ref(), prev.sats.get(sat).and_then(|x| x.d1.as_ref())) else { continue; };
        let pred = freq::doppler_predicted_m(dp.v, dn.v, dt, lam1);
        let actual = (now.v - pv.v) * lam1;
        jumps.push((sat.clone(), pred - actual));
    }
    if jumps.is_empty() { return; }
    let n = jumps.len() as f64;
    let mean: f64 = jumps.iter().map(|(_, j)| j).sum::<f64>() / n;
    if mean.abs() < p.clock_common_m {
        return;
    }
    let agreeing = jumps.iter().filter(|(_, j)| {
        j.signum() == mean.signum() && (j - mean).abs() < 30_000.0
    }).count() as f64;
    if agreeing / n >= p.clock_fraction {
        out.push(FFinding {
            scope: "station".into(), sat: None, band: None,
            epoch_id: ep.id.clone(), epoch_time: ep.time.clone(),
            detector: "clock".into(), status: "candidate".into(),
            qty: Some(mean), qty_unit: Some("m(receiver-time-common)".into()),
            n1: None, n2: None, distance: None,
            message: format!(
                "whole-receiver clock jump: {:.3} m common to {}/{} satellites; kept separate from single-satellite slips",
                mean, agreeing as i64, n as i64),
            key: fkey("clock", "station", None, None, &ep.id),
        });
    }
}

fn emit_candidates(
    sat: &str,
    ep: &EEpoch,
    lam1: &Option<f64>,
    lam2: &Option<f64>,
    p: &DetParams,
    findings: &[FFinding],
    out: &mut Vec<FCandidate>,
) {
    let (Some(lam1), Some(lam2)) = (lam1, lam2) else { return; };
    let gf = findings.iter().find(|f| f.detector == "gf" && f.epoch_id == ep.id
        && f.sat.as_deref() == Some(sat) && f.status == "candidate");
    let mw = findings.iter().find(|f| f.detector == "mw" && f.epoch_id == ep.id
        && f.sat.as_deref() == Some(sat) && f.status == "candidate");
    let td1 = findings.iter().find(|f| f.detector == "td" && f.epoch_id == ep.id
        && f.sat.as_deref() == Some(sat) && f.band.as_deref() == Some("L1") && f.status == "candidate");
    let td2 = findings.iter().find(|f| f.detector == "td" && f.epoch_id == ep.id
        && f.sat.as_deref() == Some(sat) && f.band.as_deref() == Some("L2") && f.status == "candidate");
    let Some(gf_jump) = gf.and_then(|f| f.qty) else { return; };
    let wl_delta = mw.and_then(|f| f.qty).map(|q| q.round() as i64);

    // Enumerate every legal (n1,n2) inside search bounds that satisfies the GF
    // grid within tolerance and the MW wide-lane constraint when available.
    let mut rows: Vec<FCandidate> = Vec::new();
    for n1 in p.n_min..=p.n_max {
        for n2 in p.n_min..=p.n_max {
            if let Some(w) = wl_delta {
                if n1 - n2 != w { continue; }
            }
            let g = freq::gf_from_slips(n1, n2, *lam1, *lam2);
            let dist = (g - gf_jump).abs();
            if dist <= p.gf_tol_m {
                let mut votes = Vec::new();
                let mut score = (1.0 - (dist / p.gf_tol).min(1.0)) * 0.45;
                votes.push(format!("gf:d={:.4}m", dist));
                if mw.is_some() {
                    score += 0.30;
                    votes.push("mw:WL-delta".into());
                }
                if let Some(t) = td1 {
                    if t.n1 == Some(n1) { score += 0.25; votes.push("td:L1".into()); }
                }
                if let Some(t) = td2 {
                    if t.n2 == Some(n2) { score += 0.25; votes.push("td:L2".into()); }
                }
                rows.push(FCandidate {
                    sat: sat.into(), epoch_id: ep.id.clone(), epoch_time: ep.time.clone(),
                    n1, n2, score, votes, clock_explained: false,
                    key: ckey(sat, &ep.id, n1, n2),
                });
            }
        }
    }
    rows.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.n1.cmp(&b.n1)).then(a.n2.cmp(&b.n2)));
    out.extend(rows);
}
