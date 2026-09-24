use crate::ids;
use crate::model::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const C_LIGHT: f64 = 299_792_458.0;

pub fn carrier_hz(sys: char, band: u8, _sat: &str) -> Option<f64> {
    match (sys, band) {
        ('G', 1) => Some(1575.42e6),
        ('G', 2) => Some(1227.60e6),
        ('G', 5) => Some(1176.45e6),
        ('E', 1) => Some(1575.42e6),
        ('E', 5) => Some(1176.45e6),
        ('E', 6) => Some(1278.75e6),
        ('E', 7) => Some(1207.14e6),
        ('E', 8) => Some(1191.795e6),
        ('R', 1) => Some(1602.000e6),
        ('R', 2) => Some(1246.000e6),
        ('R', 3) => Some(1202.250e6),
        ('R', 4) => Some(1600.995e6),
        ('C', 1) => Some(1561.098e6),
        ('C', 2) => Some(1207.140e6),
        ('C', 3) => Some(1268.520e6),
        ('C', 5) => Some(1176.450e6),
        ('J', 1) => Some(1575.42e6),
        ('J', 2) => Some(1227.60e6),
        ('J', 5) => Some(1176.45e6),
        _ => None,
    }
}

pub fn wavelength_m(sys: char, band: u8, sat: &str) -> Option<f64> {
    carrier_hz(sys, band, sat).map(|f| C_LIGHT / f)
}

#[derive(Debug, Clone)]
pub struct ObsPoint {
    pub epoch_idx: usize,
    pub tow_s: f64,
    pub obs: RawObservation,
}

#[derive(Debug, Clone)]
pub struct CodeSeries {
    pub sat: String,
    pub sys: char,
    pub band: u8,
    pub code: String,
    pub points: Vec<ObsPoint>,
}

#[derive(Debug, Clone)]
pub struct SatBundle {
    pub sat: String,
    pub sys: char,
    pub phases: BTreeMap<u8, CodeSeries>,
    pub ranges: BTreeMap<u8, CodeSeries>,
    pub dopplers: BTreeMap<u8, CodeSeries>,
    pub snrs: BTreeMap<u8, Vec<ObsPoint>>,
}

pub fn sigma_mad(values: &[f64], floor: f64) -> f64 {
    if values.len() < 3 {
        return floor.max(0.01);
    }
    let mut s: Vec<f64> = values.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = s[s.len() / 2];
    let mut d: Vec<f64> = values.iter().map(|v| (v - med).abs()).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (1.4826 * d[d.len() / 2]).max(floor)
}

pub struct SatStateEntry {
    pub tow_s: f64,
    pub sat: String,
    pub elev_deg: Option<f64>,
    pub azim_deg: Option<f64>,
}

pub fn parse_sat_states(values: &[serde_json::Value]) -> Vec<SatStateEntry> {
    let mut out = Vec::new();
    for v in values {
        let tow_s = v.get("tow_s").and_then(|t| t.as_f64()).unwrap_or(0.0);
        let sat = v.get("sat").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let elev_deg = v.get("elev_deg").and_then(|e| e.as_f64());
        let azim_deg = v.get("azim_deg").and_then(|e| e.as_f64());
        out.push(SatStateEntry { tow_s, sat, elev_deg, azim_deg });
    }
    out
}

pub fn elev_at(states: &[SatStateEntry], sat: &str, tow_s: f64) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for s in states.iter().filter(|s| s.sat == sat) {
        let dt = (s.tow_s - tow_s).abs();
        if best.map(|(b, _)| dt < b).unwrap_or(true) {
            best = s.elev_deg.map(|e| (dt, e));
        }
    }
    best.and_then(|(dt, e)| if dt <= 1.0 { Some(e) } else { None })
}

pub fn bundle_observations(obs: &[RawObservation]) -> BTreeMap<String, SatBundle> {
    let mut by_sat: BTreeMap<String, Vec<&RawObservation>> = BTreeMap::new();
    for o in obs {
        if o.value.is_some() {
            by_sat.entry(o.sat.clone()).or_default().push(o);
        }
    }
    let mut bundles = BTreeMap::new();
    for (sat, list) in by_sat {
        let sys = sat.chars().next().unwrap_or(' ');
        let mut phases: BTreeMap<u8, CodeSeries> = BTreeMap::new();
        let mut ranges: BTreeMap<u8, CodeSeries> = BTreeMap::new();
        let mut dopplers: BTreeMap<u8, CodeSeries> = BTreeMap::new();
        let mut snrs: BTreeMap<u8, Vec<ObsPoint>> = BTreeMap::new();
        for o in list {
            let Some(band) = o.band else { continue };
            let pt = ObsPoint { epoch_idx: o.epoch_idx, tow_s: o.tow_s, obs: o.clone() };
            let bucket = match o.obs_class.as_str() {
                "phase" => &mut phases,
                "code" => &mut ranges,
                "doppler" => &mut dopplers,
                "snr" => {
                    snrs.entry(band).or_default().push(pt);
                    continue;
                }
                _ => continue,
            };
            // deterministic preference: the smallest observation code wins a band slot
            let entry = bucket.entry(band).or_insert_with(|| CodeSeries {
                sat: sat.clone(), sys, band, code: o.code.clone(), points: Vec::new(),
            });
            if o.code < entry.code {
                entry.code = o.code.clone();
            }
        }
        // collect points for the chosen per-band code
        let fill = |bucket: &BTreeMap<u8, CodeSeries>, class: &str| -> BTreeMap<u8, CodeSeries> {
            let mut out_m = BTreeMap::new();
            for (band, cs) in bucket {
                let mut pts: Vec<ObsPoint> = list.iter()
                    .filter(|o| o.band == Some(*band) && o.obs_class == class && o.code == cs.code)
                    .map(|o| ObsPoint { epoch_idx: o.epoch_idx, tow_s: o.tow_s, obs: (*o).clone() })
                    .collect();
                pts.sort_by(|a, b| a.tow_s.partial_cmp(&b.tow_s).unwrap());
                pts.dedup_by(|a, b| a.epoch_idx == b.epoch_idx);
                out_m.insert(*band, CodeSeries { points: pts, ..cs.clone() });
            }
            out_m
        };
        let phases = fill(&phases, "phase");
        let ranges = fill(&ranges, "code");
        let dopplers = fill(&dopplers, "doppler");
        for v in snrs.values_mut() {
            v.sort_by(|a, b| a.tow_s.partial_cmp(&b.tow_s).unwrap());
            v.dedup_by(|a, b| a.epoch_idx == b.epoch_idx);
        }
        bundles.insert(sat.clone(), SatBundle { sat, sys, phases, ranges, dopplers, snrs });
    }
    bundles
}

#[derive(Debug, Clone)]
pub struct Boundary {
    pub sat: String,
    pub band: u8,
    pub code: String,
    pub arc_from: u32,
    pub boundary_epoch: usize,
    pub tow_s: f64,
    pub prev_tow_s: f64,
    pub kind: String,
    pub lli_flag: bool,
}

pub fn find_boundaries(cs: &CodeSeries, gap_threshold_s: f64) -> (Vec<Boundary>, Vec<(u32, usize, usize)>) {
    let mut boundaries = Vec::new();
    let mut arcs: Vec<(u32, usize, usize)> = Vec::new();
    if cs.points.is_empty() {
        return (boundaries, arcs);
    }
    let mut arc: u32 = 0;
    let mut arc_start = 0usize;
    for i in 1..cs.points.len() {
        let prev = &cs.points[i - 1];
        let cur = &cs.points[i];
        let gap = cur.tow_s - prev.tow_s > gap_threshold_s;
        let lli = cur.obs.lli.map(|l| l & 1 == 1).unwrap_or(false);
        if gap || lli {
            arcs.push((arc, arc_start, i - 1));
            arc += 1;
            arc_start = i;
            boundaries.push(Boundary {
                sat: cs.sat.clone(),
                band: cs.band,
                code: cs.code.clone(),
                arc_from: arc - 1,
                boundary_epoch: cur.epoch_idx,
                tow_s: cur.tow_s,
                prev_tow_s: prev.tow_s,
                kind: if gap { "gap".into() } else { "lli".into() },
                lli_flag: lli,
            });
        }
    }
    arcs.push((arc, arc_start, cs.points.len() - 1));
    (boundaries, arcs)
}

#[derive(Debug, Clone)]
pub struct DetectorOutcome {
    pub boundary: Boundary,
    pub results: Vec<DetectorCandidate>,
    pub clock_jump: bool,
    pub td_residual_m: Option<f64>,
}

fn point_at(cs: &CodeSeries, epoch_idx: usize) -> Option<&ObsPoint> {
    cs.points.iter().find(|p| p.epoch_idx == epoch_idx)
}

fn prev_point(cs: &CodeSeries, epoch_idx: usize) -> Option<&ObsPoint> {
    cs.points.iter().filter(|p| p.epoch_idx < epoch_idx).last()
}

pub fn eval_time_difference(
    bundle: &SatBundle,
    b: &Boundary,
    params: &DetectParams,
    sigma_m: f64,
) -> DetectorCandidate {
    let phase_cs = match bundle.phases.get(&b.band) {
        Some(p) => p,
        None => {
            return DetectorCandidate {
                detector: "time_difference".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
                residual: 0.0, sigma: sigma_m, score: 0.0,
                status: "insufficient_data".into(),
                reason: Some(format!("no phase series on band {}", b.band)),
                unit: "m".into(),
            };
        }
    };
    let (Some(q), Some(p)) = (point_at(phase_cs, b.boundary_epoch), prev_point(phase_cs, b.boundary_epoch)) else {
        return DetectorCandidate {
            detector: "time_difference".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
            residual: 0.0, sigma: sigma_m, score: 0.0,
            status: "insufficient_data".into(),
            reason: Some("phase missing at boundary epoch".into()),
            unit: "m".into(),
        };
    };
    let lam = match wavelength_m(bundle.sys, b.band, &b.sat) {
        Some(l) => l,
        None => {
            return DetectorCandidate {
                detector: "time_difference".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
                residual: 0.0, sigma: sigma_m, score: 0.0,
                status: "unknown".into(),
                reason: Some(format!("no carrier frequency model for system {} band {}", bundle.sys, b.band)),
                unit: "m".into(),
            };
        }
    };
    let dl_m = (q.obs.value.unwrap() - p.obs.value.unwrap()) * lam;
    let dt = q.tow_s - p.tow_s;
    let (expected_m, basis) = if let Some(rcs) = bundle.ranges.get(&b.band) {
        match (point_at(rcs, b.boundary_epoch), prev_point(rcs, b.boundary_epoch)) {
            (Some(rq), Some(rp)) => (rq.obs.value.unwrap() - rp.obs.value.unwrap(), "code_range"),
            _ => (f64::NAN, "none"),
        }
    } else {
        (f64::NAN, "none")
    };
    let (expected_m, basis) = if basis == "none" {
        if let Some(dcs) = bundle.dopplers.get(&b.band) {
            match (point_at(dcs, b.boundary_epoch), prev_point(dcs, b.boundary_epoch)) {
                (Some(dq), Some(dp)) => {
                    let d_avg = (dq.obs.value.unwrap() + dp.obs.value.unwrap()) / 2.0;
                    (-d_avg * lam * dt, "doppler")
                }
                _ => (f64::NAN, "none"),
            }
        } else {
            (f64::NAN, "none")
        }
    } else {
        (expected_m, basis)
    };
    if expected_m.is_nan() {
        return DetectorCandidate {
            detector: "time_difference".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
            residual: 0.0, sigma: sigma_m, score: 0.0,
            status: "insufficient_data".into(),
            reason: Some("neither code range nor Doppler available to predict phase change".into()),
            unit: "m".into(),
        };
    }
    let residual = dl_m - expected_m;
    let score = residual.abs() / sigma_m;
    let (status, slip, reason) = if score >= params.td_sigma_threshold {
        let s = (residual / lam).round();
        ("candidate".to_string(), Some(s),
         Some(format!("|ΔL-ΔP|={:.3} m = {:.1}σ (basis {basis}, dt={dt:.1} s)", residual, score)))
    } else {
        ("clean".to_string(), None,
         Some(format!("|ΔL-ΔP|={:.3} m = {:.1}σ below threshold", residual, score)))
    };
    DetectorCandidate {
        detector: "time_difference".into(),
        slip_cycles: slip, slip_pair: None, alternates: Vec::new(),
        residual, sigma: sigma_m, score, status, reason, unit: "m".into(),
    }
}

pub fn eval_geometry_free(
    bundle: &SatBundle,
    b: &Boundary,
    band1: u8,
    band2: u8,
    params: &DetectParams,
    sigma_m: f64,
) -> DetectorCandidate {
    let mk = |status: &str, reason: String| DetectorCandidate {
        detector: "geometry_free".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
        residual: 0.0, sigma: sigma_m, score: 0.0,
        status: status.into(), reason: Some(reason), unit: "m".into(),
    };
    let (Some(l1cs), Some(l2cs)) = (bundle.phases.get(&band1), bundle.phases.get(&band2)) else {
        return mk("insufficient_data", format!("need phase on bands {band1} and {band2}"));
    };
    let (Some(l1), Some(l2)) = (wavelength_m(bundle.sys, band1, &b.sat), wavelength_m(bundle.sys, band2, &b.sat)) else {
        return mk("unknown", "missing carrier frequency model".into());
    };
    let l4_at = |cs1: &CodeSeries, cs2: &CodeSeries, epoch: usize| -> Option<f64> {
        let p1 = point_at(cs1, epoch)?;
        let p2 = point_at(cs2, epoch)?;
        Some(p1.obs.value.unwrap() * l1 - p2.obs.value.unwrap() * l2)
    };
    let l4_prev = |cs1: &CodeSeries, cs2: &CodeSeries, epoch: usize| -> Option<f64> {
        let p1 = prev_point(cs1, epoch)?;
        let p2 = prev_point(cs2, epoch)?;
        Some(p1.obs.value.unwrap() * l1 - p2.obs.value.unwrap() * l2)
    };
    let (Some(a), Some(c)) = (l4_prev(l1cs, l2cs, b.boundary_epoch), l4_at(l1cs, l2cs, b.boundary_epoch)) else {
        return mk("insufficient_data", "phase missing on one band at boundary (frequency gap)".into());
    };
    let residual = c - a;
    let score = residual.abs() / sigma_m;
    if score < params.gf_sigma_threshold {
        return DetectorCandidate {
            detector: "geometry_free".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
            residual, sigma: sigma_m, score,
            status: "clean".into(),
            reason: Some(format!("ΔL4={residual:.3} m = {score:.1}σ below threshold")),
            unit: "m".into(),
        };
    }
    let mut valid: Vec<(i32, i32, f64)> = Vec::new();
    let tol = (3.0 * sigma_m).max(0.05);
    for n1 in -params.max_gf_slip_cycles..=params.max_gf_slip_cycles {
        for n2 in -params.max_gf_slip_cycles..=params.max_gf_slip_cycles {
            if n1 == 0 && n2 == 0 {
                continue;
            }
            let expected = n1 as f64 * l1 - n2 as f64 * l2;
            let err = (residual - expected).abs();
            if err < tol {
                valid.push((n1, n2, err));
            }
        }
    }
    valid.sort_by(|x, y| x.2.partial_cmp(&y.2).unwrap());
    match valid.first() {
        None => DetectorCandidate {
            detector: "geometry_free".into(), slip_cycles: None, slip_pair: None, alternates: Vec::new(),
            residual, sigma: sigma_m, score,
            status: "unexplained".into(),
            reason: Some(format!("ΔL4={residual:.3} m matches no small integer pair (n1,n2)")),
            unit: "m".into(),
        },
        Some(&(n1, n2, err)) => {
            let ambiguous = valid.len() > 1;
            DetectorCandidate {
                detector: "geometry_free".into(), slip_cycles: None,
                slip_pair: Some((n1, n2)),
                alternates: valid.iter().skip(1).map(|v| (v.0, v.1, v.2)).collect(),
                residual, sigma: sigma_m, score: err / sigma_m,
                status: if ambiguous { "ambiguous".into() } else { "candidate".into() },
                reason: Some(if ambiguous {
                    format!("{} equally-divided integer pairs fit ΔL4={:.3} m (e.g. {:?}); all retained",
                        valid.len(), residual, valid.iter().take(4).map(|v| (v.0, v.1)).collect::<Vec<_>>())
                } else {
                    format!("ΔL4={residual:.3} m fits (n1,n2)=({n1},{n2}) within {err:.3} m")
                }),
                unit: "m".into(),
            }
        }
    }
}

pub fn eval_melbourne_wubbena(
    bundle: &SatBundle,
    b: &Boundary,
    band1: u8,
    band2: u8,
    params: &DetectParams,
    sigma_m: f64,
) -> DetectorCandidate {
    let mk = |status: &str, reason: String| DetectorCandidate {
        detector: "melbourne_wubbena".into(), slip_cycles: None, slip_pair: None,
        alternates: Vec::new(), residual: 0.0, sigma: sigma_m, score: 0.0,
        status: status.into(), reason: Some(reason), unit: "m".into(),
    };
    let (Some(f1), Some(f2)) = (carrier_hz(bundle.sys, band1, &b.sat), carrier_hz(bundle.sys, band2, &b.sat)) else {
        return mk("unknown", "missing carrier frequency model".into());
    };
    let (Some(l1cs), Some(l2cs), Some(p1cs), Some(p2cs)) = (
        bundle.phases.get(&band1), bundle.phases.get(&band2),
        bundle.ranges.get(&band1), bundle.ranges.get(&band2),
    ) else {
        return mk("insufficient_data", "MW needs phase and code on both bands".into());
    };
    let lam_w = C_LIGHT / (f1 - f2);
    let f_n = f1 + f2;
    let mw_at = |epoch: usize, prev: bool| -> Option<f64> {
        let get = |cs: &CodeSeries| if prev { prev_point(cs, epoch) } else { point_at(cs, epoch) };
        let (l1, l2, p1, p2) = (get(l1cs)?, get(l2cs)?, get(p1cs)?, get(p2cs)?);
        let l_w = l1.obs.value.unwrap() - l2.obs.value.unwrap();
        let p_n = (f1 * p1.obs.value.unwrap() + f2 * p2.obs.value.unwrap()) / f_n;
        Some(l_w * lam_w - p_n)
    };
    let (Some(a), Some(c)) = (mw_at(b.boundary_epoch, true), mw_at(b.boundary_epoch, false)) else {
        return mk("insufficient_data", "phase/code missing at boundary epoch (frequency gap)".into());
    };
    let residual = c - a;
    let score = residual.abs() / sigma_m;
    if score < params.mw_sigma_threshold {
        return DetectorCandidate {
            detector: "melbourne_wubbena".into(), slip_cycles: None, slip_pair: None,
            alternates: Vec::new(), residual, sigma: sigma_m, score,
            status: "clean".into(),
            reason: Some(format!("ΔMW={residual:.3} m = {score:.1}σ below threshold")),
            unit: "m".into(),
        };
    }
    let tol = (3.0 * sigma_m).max(0.15);
    let mut valid: Vec<(i32, f64)> = Vec::new();
    for nw in -params.max_mw_slip_cycles..=params.max_mw_slip_cycles {
        if nw == 0 {
            continue;
        }
        let err = (residual - nw as f64 * lam_w).abs();
        if err < tol {
            valid.push((nw, err));
        }
    }
    valid.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap());
    match valid.first() {
        None => DetectorCandidate {
            detector: "melbourne_wubbena".into(), slip_cycles: None, slip_pair: None,
            alternates: Vec::new(), residual, sigma: sigma_m, score,
            status: "unexplained".into(),
            reason: Some(format!("ΔMW={residual:.3} m matches no integer widelane cycle")),
            unit: "m".into(),
        },
        Some(&(nw, err)) => DetectorCandidate {
            detector: "melbourne_wubbena".into(),
            slip_cycles: Some(nw as f64),
            slip_pair: None,
            alternates: valid.iter().skip(1).map(|v| (v.0, 0, v.1)).collect(),
            residual, sigma: sigma_m, score: err / sigma_m,
            status: if valid.len() > 1 { "ambiguous".into() } else { "candidate".into() },
            reason: Some(format!("ΔMW={residual:.3} m fits widelane slip n_w={nw} within {err:.3} m (λ_w={lam_w:.3} m)")),
            unit: "m".into(),
        },
    }
}

/// Common-mode receiver clock jump: TD residuals of many satellites jump by the
/// same large amount at the same epoch. Kept strictly separate from per-satellite
/// cycle slips; clock jumps never produce repair offsets.
pub fn detect_clock_jumps(
    outcomes: &mut [DetectorOutcome],
    clock_events: &[ClockEvent],
    params: &DetectParams,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let min_m = C_LIGHT * params.clock_jump_min_s;
    let mut by_epoch: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, o) in outcomes.iter().enumerate() {
        if let Some(r) = o.td_residual_m {
            if r.abs() >= min_m {
                by_epoch.entry(o.boundary.boundary_epoch).or_default().push(i);
            }
        }
    }
    let total_sats: usize = outcomes.iter().map(|o| o.boundary.sat.clone()).collect::<BTreeSet<_>>().len();
    for (epoch, idxs) in by_epoch {
        let residuals: Vec<f64> = idxs.iter().filter_map(|i| outcomes[*i].td_residual_m).collect();
        if residuals.is_empty() {
            continue;
        }
        let mean = residuals.iter().sum::<f64>() / residuals.len() as f64;
        let spread = residuals.iter().map(|r| (r - mean).abs()).fold(0.0, f64::max);
        let n_sats_hit: usize = idxs.iter().map(|i| outcomes[*i].boundary.sat.clone()).collect::<BTreeSet<_>>().len();
        let common = spread < 5.0 && n_sats_hit as f64 >= params.clock_jump_common_ratio * total_sats.max(1) as f64;
        if !common {
            continue;
        }
        let tow = outcomes[idxs[0]].boundary.tow_s;
        let logged = clock_events.iter().any(|e| (e.tow_s - tow).abs() <= 1.0);
        for i in idxs {
            outcomes[i].clock_jump = true;
            for dc in outcomes[i].results.iter_mut() {
                if dc.detector == "time_difference" {
                    dc.status = "clock_jump".into();
                    dc.slip_cycles = None;
                    dc.reason = Some(format!(
                        "common-mode jump {:.1} m across {} satellites at epoch {} ({}); classified as receiver clock jump, not a cycle slip",
                        mean, n_sats_hit, epoch,
                        if logged { "corroborated by logged clock event" } else { "no logged clock event" }
                    ));
                }
            }
        }
        diags.push(diag("info", "receiver_clock_jump",
            format!("epoch {epoch}: common-mode jump {mean:.1} m over {n_sats_hit}/{total_sats} satellites ({})",
                if logged { "logged clock event corroborates" } else { "inferred from observations" })));
    }
    diags
}

fn gf_value(bundle: &SatBundle, pair: (u8, u8), epoch: usize, prev: bool) -> Option<f64> {
    let (b1, b2) = pair;
    let l1 = wavelength_m(bundle.sys, b1, &bundle.sat)?;
    let l2 = wavelength_m(bundle.sys, b2, &bundle.sat)?;
    let get = |band: u8| {
        let cs = bundle.phases.get(&band)?;
        let p = if prev { prev_point(cs, epoch) } else { point_at(cs, epoch) }?;
        p.obs.value
    };
    Some(get(b1)? * l1 - get(b2)? * l2)
}

fn mw_value(bundle: &SatBundle, pair: (u8, u8), epoch: usize, prev: bool) -> Option<f64> {
    let (b1, b2) = pair;
    let f1 = carrier_hz(bundle.sys, b1, &bundle.sat)?;
    let f2 = carrier_hz(bundle.sys, b2, &bundle.sat)?;
    let lam_w = C_LIGHT / (f1 - f2);
    let f_n = f1 + f2;
    let get = |map: &BTreeMap<u8, CodeSeries>, band: u8| {
        let cs = map.get(&band)?;
        let p = if prev { prev_point(cs, epoch) } else { point_at(cs, epoch) }?;
        p.obs.value
    };
    let l_w = get(&bundle.phases, b1)? - get(&bundle.phases, b2)?;
    let p_n = (f1 * get(&bundle.ranges, b1)? + f2 * get(&bundle.ranges, b2)?) / f_n;
    Some(l_w * lam_w - p_n)
}

fn td_prediction(bundle: &SatBundle, band: u8, epoch: usize, dt: f64) -> Option<f64> {
    if let Some(rcs) = bundle.ranges.get(&band) {
        if let (Some(q), Some(p)) = (point_at(rcs, epoch), prev_point(rcs, epoch)) {
            return Some(q.obs.value.unwrap() - p.obs.value.unwrap());
        }
    }
    if let Some(dcs) = bundle.dopplers.get(&band) {
        if let (Some(q), Some(p)) = (point_at(dcs, epoch), prev_point(dcs, epoch)) {
            let lam = wavelength_m(bundle.sys, band, &bundle.sat)?;
            let d_avg = (q.obs.value.unwrap() + p.obs.value.unwrap()) / 2.0;
            return Some(-d_avg * lam * dt);
        }
    }
    None
}

fn arc_of_epoch(cs: &CodeSeries, epoch_idx: usize, gap_threshold_s: f64) -> u32 {
    let mut arc = 0u32;
    for i in 1..cs.points.len() {
        if cs.points[i].epoch_idx > epoch_idx {
            break;
        }
        let gap = cs.points[i].tow_s - cs.points[i - 1].tow_s > gap_threshold_s;
        let lli = cs.points[i].obs.lli.map(|l| l & 1 == 1).unwrap_or(false);
        if gap || lli {
            arc += 1;
        }
    }
    arc
}

fn dependency_fingerprint(bundle: &SatBundle, pair: Option<(u8, u8)>, b: &Boundary) -> String {
    let mut ids: Vec<String> = Vec::new();
    let mut collect = |map: &BTreeMap<u8, CodeSeries>, band: u8| {
        if let Some(cs) = map.get(&band) {
            for p in &cs.points {
                ids.push(p.obs.id.clone());
            }
        }
    };
    let mut bands = vec![b.band];
    if let Some((b1, b2)) = pair {
        bands.push(b1);
        bands.push(b2);
    }
    bands.sort();
    bands.dedup();
    for band in bands {
        collect(&bundle.phases, band);
        collect(&bundle.ranges, band);
        collect(&bundle.dopplers, band);
    }
    ids::digest(&ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())
}

pub struct DetectionOutput {
    pub arcs: Vec<ArcInfo>,
    pub combo_points: Vec<ComboPoint>,
    pub combo_index: Vec<(String, u8, String, u32)>,
    pub evals: Vec<BoundaryEval>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone)]
struct Hyp {
    slip: f64,
    contribution: f64,
    detectors: Vec<String>,
}

pub fn run_detection(
    _project_id: &str,
    _branch: &str,
    run_id: &str,
    params: &DetectParams,
    station: &str,
    observations: &[RawObservation],
    clock_events: &[ClockEvent],
    sat_states: &[SatStateEntry],
) -> DetectionOutput {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let bundles = bundle_observations(observations);
    let mut arcs_out: Vec<ArcInfo> = Vec::new();
    let mut combo_out: Vec<ComboPoint> = Vec::new();
    let mut combo_index: Vec<(String, u8, String, u32)> = Vec::new();
    let mut outcomes: Vec<DetectorOutcome> = Vec::new();
    let mut evals: Vec<BoundaryEval> = Vec::new();

    let mut pairs: HashMap<String, Option<(u8, u8)>> = HashMap::new();
    for (sat, bnd) in &bundles {
        let bands: Vec<u8> = bnd.phases.keys().copied().collect();
        pairs.insert(sat.clone(), bands.get(1).map(|b2| (bands[0], *b2)));
    }

    for (sat, bundle) in bundles.iter() {
        let pair = pairs.get(sat).copied().flatten();

        for (band, cs) in &bundle.phases {
            let (_, arc_ranges) = find_boundaries(cs, params.gap_threshold_s);
            for (arc, i0, i1) in &arc_ranges {
                arcs_out.push(ArcInfo {
                    station: station.into(), sat: sat.clone(), band: *band, code: cs.code.clone(),
                    arc: *arc,
                    start_epoch: cs.points[*i0].epoch_idx,
                    end_epoch: cs.points[*i1].epoch_idx,
                    start_tow_s: cs.points[*i0].tow_s,
                    end_tow_s: cs.points[*i1].tow_s,
                    n_epochs: i1 - i0 + 1,
                });
            }
            let lam = wavelength_m(bundle.sys, *band, sat);
            for (i, pt) in cs.points.iter().enumerate() {
                let mut td_phase_m = None;
                let mut td_code_m = None;
                if i > 0 {
                    let prev = &cs.points[i - 1];
                    if let Some(l) = lam {
                        td_phase_m = Some((pt.obs.value.unwrap() - prev.obs.value.unwrap()) * l);
                    }
                    if let Some(rcs) = bundle.ranges.get(band) {
                        if let (Some(q), Some(p)) = (point_at(rcs, pt.epoch_idx), prev_point(rcs, pt.epoch_idx)) {
                            td_code_m = Some(q.obs.value.unwrap() - p.obs.value.unwrap());
                        }
                    }
                }
                let (l4_m, mw_m) = if pair.map(|(b1, _)| b1 == *band).unwrap_or(false) {
                    (gf_value(bundle, pair.unwrap(), pt.epoch_idx, false),
                     mw_value(bundle, pair.unwrap(), pt.epoch_idx, false))
                } else {
                    (None, None)
                };
                combo_out.push(ComboPoint {
                    epoch_idx: pt.epoch_idx, tow_s: pt.tow_s,
                    l4_m, mw_m, td_phase_m, td_code_m,
                    elev_deg: elev_at(sat_states, sat, pt.tow_s),
                });
                combo_index.push((sat.clone(), *band, cs.code.clone(),
                    arc_of_epoch(cs, pt.epoch_idx, params.gap_threshold_s)));
            }
        }

        let mut band_boundaries: Vec<(u8, Vec<Boundary>)> = Vec::new();
        for (band, cs) in &bundle.phases {
            let (bds, _) = find_boundaries(cs, params.gap_threshold_s);
            band_boundaries.push((*band, bds));
        }

        let mut td_sigmas: HashMap<u8, f64> = HashMap::new();
        for (band, bds) in &band_boundaries {
            let mut residuals = Vec::new();
            if let Some(cs) = bundle.phases.get(band) {
                if let Some(lam) = wavelength_m(bundle.sys, *band, sat) {
                    for b in bds {
                        if let (Some(q), Some(p)) = (point_at(cs, b.boundary_epoch), prev_point(cs, b.boundary_epoch)) {
                            let dl = (q.obs.value.unwrap() - p.obs.value.unwrap()) * lam;
                            if let Some(pred) = td_prediction(bundle, *band, b.boundary_epoch, q.tow_s - p.tow_s) {
                                residuals.push(dl - pred);
                            }
                        }
                    }
                }
            }
            td_sigmas.insert(*band, sigma_mad(&residuals, 0.05));
        }

        let mut gf_res = Vec::new();
        let mut mw_res = Vec::new();
        if let Some(pr) = pair {
            for (_, bds) in &band_boundaries {
                for b in bds {
                    if let (Some(a), Some(c)) = (gf_value(bundle, pr, b.boundary_epoch, true),
                                                 gf_value(bundle, pr, b.boundary_epoch, false)) {
                        gf_res.push(c - a);
                    }
                    if let (Some(a), Some(c)) = (mw_value(bundle, pr, b.boundary_epoch, true),
                                                 mw_value(bundle, pr, b.boundary_epoch, false)) {
                        mw_res.push(c - a);
                    }
                }
            }
        }
        let gf_sigma = sigma_mad(&gf_res, 0.05);
        let mw_sigma = sigma_mad(&mw_res, 0.3);

        for (band, bds) in band_boundaries {
            for b in bds {
                let mut results = Vec::new();
                let td = eval_time_difference(bundle, &b, params, td_sigmas[&band]);
                let td_residual = td.residual;
                results.push(td);
                if let Some((b1, b2)) = pair {
                    if band == b1 || band == b2 {
                        results.push(eval_geometry_free(bundle, &b, b1, b2, params, gf_sigma));
                        results.push(eval_melbourne_wubbena(bundle, &b, b1, b2, params, mw_sigma));
                    }
                } else {
                    for (det, msg) in [
                        ("geometry_free", "single-frequency arc; geometry-free combination unavailable"),
                        ("melbourne_wubbena", "single-frequency arc; Melbourne-Wubbena unavailable"),
                    ] {
                        results.push(DetectorCandidate {
                            detector: det.into(), slip_cycles: None, slip_pair: None,
                            alternates: Vec::new(), residual: 0.0, sigma: 0.0, score: 0.0,
                            status: "insufficient_data".into(),
                            reason: Some(msg.into()), unit: "m".into(),
                        });
                    }
                }
                outcomes.push(DetectorOutcome { boundary: b, results, clock_jump: false, td_residual_m: td_residual });
            }
        }
    }

    diags.extend(detect_clock_jumps(&mut outcomes, clock_events, params));

    for o in &outcomes {
        let b = &o.boundary;
        let bundle = &bundles[&b.sat];
        let pair = pairs.get(&b.sat).copied().flatten();
        let deps = dependency_fingerprint(bundle, pair, b);
        let bkey = BoundaryKey {
            station: station.into(),
            sat: b.sat.clone(),
            band: b.band,
            code: b.code.clone(),
            arc: b.arc_from,
            boundary_epoch: b.boundary_epoch,
        };
        let eval_id = ids::id12("eval", &[run_id, &bkey.key()]);

        let mut hyps: BTreeMap<i64, Hyp> = BTreeMap::new();
        let mut add_hyp = |hyps: &mut BTreeMap<i64, Hyp>, slip: f64, contrib: f64, det: &str| {
            let key = (slip * 1.0).round() as i64;
            let e = hyps.entry(key).or_insert(Hyp { slip, contribution: 0.0, detectors: Vec::new() });
            e.contribution += contrib;
            if !e.detectors.contains(&det.to_string()) {
                e.detectors.push(det.to_string());
            }
        };

        for dc in &o.results {
            match dc.detector.as_str() {
                "time_difference" if dc.status == "candidate" => {
                    if let Some(s) = dc.slip_cycles {
                        add_hyp(&mut hyps, s, params.weight_td * 1.0, "time_difference");
                    }
                }
                "geometry_free" if dc.status == "candidate" || dc.status == "ambiguous" => {
                    let (b1, b2) = pair.expect("gf result requires a pair");
                    let mut pairs_all: Vec<(i32, i32, f64)> = Vec::new();
                    if let Some(p) = dc.slip_pair {
                        pairs_all.push((p.0, p.1, dc.score));
                    }
                    pairs_all.extend(dc.alternates.iter().map(|t| (t.0, t.1, t.2 / dc.sigma.max(1e-9))));
                    for (n1, n2, err_sigma) in pairs_all {
                        let contrib = params.weight_gf / (1.0 + err_sigma.max(0.0));
                        if b.band == b1 && n1 != 0 {
                            add_hyp(&mut hyps, n1 as f64, contrib, "geometry_free");
                        }
                        if b.band == b2 && n2 != 0 {
                            add_hyp(&mut hyps, n2 as f64, contrib, "geometry_free");
                        }
                    }
                }
                _ => {}
            }
        }

        let mw_confirms = |slip: f64| -> bool {
            let dc = match o.results.iter().find(|d| d.detector == "melbourne_wubbena") {
                Some(d) => d,
                None => return false,
            };
            let gf = o.results.iter().find(|d| d.detector == "geometry_free");
            if dc.status != "candidate" && dc.status != "ambiguous" {
                return false;
            }
            let nw = match dc.slip_cycles {
                Some(v) => v as i32,
                None => return false,
            };
            if let Some(g) = gf {
                if let Some(p) = g.slip_pair {
                    let (b1, b2) = pair.unwrap();
                    let this_slip = if b.band == b1 { p.0 } else if b.band == b2 { p.1 } else { return false };
                    let other = if b.band == b1 { p.1 } else { p.0 };
                    let expected_nw = if b.band == b1 { this_slip - other } else { other - this_slip };
                    return expected_nw == nw && this_slip as f64 == slip;
                }
            }
            false
        };
        let mut hyp_list: Vec<Hyp> = hyps.into_values().collect();
        for h in hyp_list.iter_mut() {
            if mw_confirms(h.slip) {
                h.contribution += params.weight_mw * 0.5;
                if !h.detectors.contains(&"melbourne_wubbena".to_string()) {
                    h.detectors.push("melbourne_wubbena".into());
                }
            }
        }
        hyp_list.sort_by(|x, y| y.contribution.partial_cmp(&x.contribution).unwrap()
            .then(x.slip.partial_cmp(&y.slip).unwrap()));

        let mut candidates = Vec::new();
        let max_score = hyp_list.first().map(|h| h.contribution).unwrap_or(0.0);
        for (rank, h) in hyp_list.into_iter().enumerate() {
            let ambiguous = max_score > 0.0
                && (h.contribution - max_score).abs() < 1e-9 && rank > 0;
            let cid = ids::id12("cand", &[&eval_id, &h.slip.to_string(), &rank.to_string()]);
            candidates.push(Candidate {
                id: cid,
                run_id: run_id.into(),
                boundary: bkey.clone(),
                slip_cycles: h.slip,
                unit: "cycle".into(),
                score: h.contribution,
                status: if ambiguous { "ambiguous".into() } else { "proposed".into() },
                supporting: h.detectors.clone(),
                rank: rank as u32,
            });
        }

        let lli_verdict = if b.lli_flag {
            let td = o.results.iter().find(|d| d.detector == "time_difference");
            let confirmed = td.map(|d| d.score >= params.lli_confirm_sigma
                && (d.status == "candidate" || d.status == "clock_jump")).unwrap_or(false);
            Some(if confirmed { "lli_confirmed".into() } else { "lli_false_positive".into() })
        } else {
            None
        };

        let note = if o.clock_jump {
            Some("receiver clock jump separated from satellite-specific cycle slips; no repair offset created".into())
        } else if candidates.is_empty() {
            Some("no candidate survived detector voting; clean, insufficient data or rejected-only boundary".into())
        } else {
            None
        };

        evals.push(BoundaryEval {
            id: eval_id,
            run_id: run_id.into(),
            boundary: bkey,
            deps_fingerprint: deps,
            kind: b.kind.clone(),
            detectors: o.results.clone(),
            candidates,
            clock_jump: o.clock_jump,
            lli_flag: b.lli_flag,
            lli_verdict,
            note,
        });
    }

    DetectionOutput {
        arcs: arcs_out,
        combo_points: combo_out,
        combo_index,
        evals,
        diagnostics: diags,
    }
}
