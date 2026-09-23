//! Pure domain computations: station/satellite/band arcs, inter-epoch steps,
//! whole-machine clock jumps, ionospheric (geometry-free) combination,
//! Melbourne-Wubbena widelane, and per-band time difference.
//!
//! No detector is treated as truth. Each detector votes independently and may
//! disagree; candidate offsets are first-class data with per-detector
//! residuals and statuses.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::SPEED_OF_LIGHT_M_S;

pub const DET_IONO: &str = "iono-gf";
pub const DET_MW: &str = "melbourne-wubbena";
pub const DET_TD: &str = "time-difference";
pub const DET_CLOCK: &str = "receiver-clock";
pub const DET_LLI: &str = "lli-flag";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandValue {
    pub band: String,
    pub code: String,
    pub phase_cycles: Option<f64>,
    pub code_m: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub snr_db_hz: Option<f64>,
    pub lli: Option<String>,
    pub slot_ids: BTreeMap<String, String>, // kind -> slot stable id
    pub wavelength_m: Option<f64>,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub epoch_index: u64,
    pub time_s: f64,
    pub bands: BTreeMap<String, BandValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockJump {
    pub id: String,
    pub epoch_index: u64,
    pub jump_ns: f64,
    pub median_code_m: f64,
    pub median_phase_m: BTreeMap<String, f64>,
    pub supporting_sats: usize,
    pub origin: String, // declared | inferred | declared+inferred
    pub source_clock_event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResidualReport {
    pub iono_m: Option<f64>,
    pub mw_m: Option<f64>,
    pub td_cycles: BTreeMap<String, f64>,
    pub code_rate_m_s: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepNode {
    pub id: String,
    pub sat: SatId,
    pub epoch_index: u64,
    pub prev_epoch_index: u64,
    pub delta_seconds: f64,
    pub irregular_gap: bool,
    pub clock_jump_id: Option<String>,
    pub lli_flags: Vec<String>,
    pub missing_bands: Vec<String>,
    pub boundary_reasons: Vec<String>,
    pub candidates: Vec<Candidate>,
    pub residuals: ResidualReport,
    pub status: String, // candidate | clean | clock-only | missing-band | unknown | error
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArcNode {
    pub id: String,
    pub sat: SatId,
    pub arc_index: usize,
    pub band: String,
    pub start_epoch: u64,
    pub end_epoch: u64,
    pub start_reason: String,
    pub end_reason: String,
    pub point_count: usize,
    pub step_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatSeries {
    pub sat: SatId,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone)]
pub struct ComputeInput {
    pub bundle: SourceBundle,
    pub params: DetectorParams,
    pub slot_ids: BTreeMap<String, String>, // key: epoch:sat:code
    pub sat_state_ids: BTreeMap<String, String>, // key: sat:band:code
    pub clock_source_ids: BTreeMap<u64, String>, // epoch -> event id
    pub scope_id: String,
    pub wavelengths: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputeResult {
    pub steps: Vec<StepNode>,
    pub arcs: Vec<ArcNode>,
    pub clock_jumps: Vec<ClockJump>,
    pub wavelengths: BTreeMap<String, f64>, // sat:band -> m
    pub diagnostics: Vec<String>,
}

fn key_sat_band(sat: &SatId, band: &str) -> String {
    format!("{}:{}", sat.raw(), band)
}

pub fn wavelength_map(input: &ComputeInput) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for st in &input.bundle.sat_states {
        if let Some(f) = st.frequency_hz.value {
            if f > 0.0 {
                out.insert(key_sat_band(&st.sat, &st.band), SPEED_OF_LIGHT_M_S / f);
            }
        } else if let Some(q) = &st.wavelength_m {
            if let Some(w) = q.value {
                out.insert(key_sat_band(&st.sat, &st.band), w);
            }
        }
    }
    out
}

pub fn build_series(input: &ComputeInput) -> BTreeMap<String, SatSeries> {
    let mut by_sat: BTreeMap<String, SatSeries> = BTreeMap::new();
    for epoch in &input.bundle.obs.epochs {
        for rec in &epoch.records {
            let k = rec.sat.raw();
            let entry = by_sat.entry(k.clone()).or_insert_with(|| SatSeries {
                sat: rec.sat.clone(),
                points: Vec::new(),
            });
            let mut bands: BTreeMap<String, BandValue> = BTreeMap::new();
            for slot in &rec.slots {
                let bv = bands.entry(slot.code.band.clone()).or_insert_with(|| BandValue {
                    band: slot.code.band.clone(),
                    code: slot.code.raw.clone(),
                    phase_cycles: None,
                    code_m: None,
                    doppler_hz: None,
                    snr_db_hz: None,
                    lli: None,
                    slot_ids: BTreeMap::new(),
                    wavelength_m: input
                        .wavelengths
                        .get(&key_sat_band(&rec.sat, &slot.code.band))
                        .copied(),
                    missing: true,
                });
                let kind = slot.code.kind_name();
                let skey = format!(
                    "{}:{}:{}",
                    epoch.epoch_index,
                    rec.sat.raw(),
                    slot.code.raw
                );
                if let Some(id) = input.slot_ids.get(&skey) {
                    bv.slot_ids.insert(kind.to_string(), id.clone());
                }
                if !slot.missing && slot.quantity.value.is_some() {
                    bv.missing = false;
                }
                match kind {
                    KIND_PHASE => {
                        bv.phase_cycles = slot.quantity.value;
                        if slot.lli.is_some() {
                            bv.lli = slot.lli.clone();
                        }
                    }
                    KIND_CODE => bv.code_m = slot.quantity.value,
                    KIND_DOPPLER => bv.doppler_hz = slot.quantity.value,
                    KIND_SNR => bv.snr_db_hz = slot.quantity.value,
                    _ => {}
                }
            }
            entry.points.push(Point {
                epoch_index: epoch.epoch_index,
                time_s: epoch.gps_time_s(),
                bands,
            });
        }
    }
    for s in by_sat.values_mut() {
        s.points.sort_by_key(|p| p.epoch_index);
    }
    by_sat
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if v.is_empty() {
        return f64::NAN;
    }
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Detect whole-machine clock jumps at each epoch boundary. Declared events
/// (companion file) and inferred jumps (all satellites jump together) are
/// cross-checked but never turned into per-satellite cycle slips.
pub fn detect_clock_jumps(
    input: &ComputeInput,
    series: &BTreeMap<String, SatSeries>,
) -> Vec<ClockJump> {
    let epochs = &input.bundle.obs.epochs;
    if epochs.len() < 2 {
        return Vec::new();
    }
    let prev_time: BTreeMap<u64, f64> = epochs
        .iter()
        .zip(epochs.iter().skip(1))
        .map(|(a, b)| (b.epoch_index, a.gps_time_s()))
        .collect();
    let mut jumps = Vec::new();
    for ep in epochs.iter().skip(1) {
        let dt = ep.gps_time_s() - *prev_time.get(&ep.epoch_index).unwrap_or(&0.0);
        if dt <= 0.0 {
            continue;
        }
        let mut code_shifts = Vec::new();
        let mut phase_shift_sums: BTreeMap<String, f64> = BTreeMap::new();
        let mut phase_shift_cnt: BTreeMap<String, usize> = BTreeMap::new();
        for s in series.values() {
            let pts = &s.points;
            let Some(idx) = pts.iter().position(|p| p.epoch_index == ep.epoch_index) else {
                continue;
            };
            if idx == 0 {
                continue;
            }
            let (a, b) = (&pts[idx - 1], &pts[idx]);
            for (band, bvb) in &b.bands {
                let Some(bva) = a.bands.get(band) else { continue };
                if let (Some(p0), Some(p1), Some(w)) =
                    (bva.phase_cycles, bvb.phase_cycles, bvb.wavelength_m)
                {
                    let rate = bvb
                        .code_m
                        .zip(bva.code_m)
                        .map(|(c1, c0)| (c1 - c0) / dt)
                        .or_else(|| bvb.doppler_hz.map(|d| d * w));
                    let expected_phase =
                        rate.map(|r| r * dt / w).unwrap_or_default();
                    let shift = (p1 - p0 - expected_phase) * w;
                    phase_shift_sums
                        .entry(band.clone())
                        .and_modify(|v| *v += shift)
                        .or_insert(shift);
                    *phase_shift_cnt.entry(band.clone()).or_insert(0) += 1;
                }
                if let (Some(c0), Some(c1)) = (bva.code_m, bvb.code_m) {
                    let rate = bvb.doppler_hz.zip(bvb.wavelength_m).map(|(d, w)| d * w);
                    let expected_code =
                        rate.map(|r| r * dt).unwrap_or_default();
                    code_shifts.push(c1 - c0 - expected_code);
                }
            }
        }
        if code_shifts.is_empty() {
            continue;
        }
        let med_code = median(code_shifts.clone());
        let n_consensus = code_shifts
            .iter()
            .filter(|v| (**v - med_code).abs() < 30.0)
            .count();
        let jump_ns = med_code / SPEED_OF_LIGHT_M_S * 1e9;
        let inferred = n_consensus >= 2
            && (jump_ns.abs() >= input.params.clock_jump_min_ns
                || phase_shift_cnt.values().any(|c| *c >= 2));
        let declared_id = input.clock_source_ids.get(&ep.epoch_index).cloned();
        if declared_id.is_none() && !(inferred && jump_ns.abs() >= input.params.clock_jump_min_ns)
        {
            continue;
        }
        let mut med_phase = BTreeMap::new();
        for (band, sum) in &phase_shift_sums {
            let cnt = *phase_shift_cnt.get(band).unwrap_or(&1).max(&1);
            med_phase.insert(band.clone(), sum / cnt as f64);
        }
        let origin = match (declared_id.is_some(), inferred) {
            (true, true) => "declared+inferred",
            (true, false) => "declared",
            _ => "inferred",
        };
        let id = crate::util::hid(
            "clk:",
            &format!("{}:{}:{}", input.scope_id, ep.epoch_index, origin),
        );
        jumps.push(ClockJump {
            id,
            epoch_index: ep.epoch_index,
            jump_ns,
            median_code_m: med_code,
            median_phase_m: med_phase,
            supporting_sats: n_consensus.max(phase_shift_cnt.values().copied().max().unwrap_or(0)),
            origin: origin.into(),
            source_clock_event_id: declared_id,
        });
    }
    jumps
}

fn expected_phase_cycles(a: &BandValue, b: &BandValue, dt: f64) -> Option<f64> {
    let w = b.wavelength_m?;
    if let (Some(c0), Some(c1)) = (a.code_m, b.code_m) {
        return Some((c1 - c0) / w);
    }
    if let Some(d) = b.doppler_hz {
        return Some(d * dt);
    }
    None
}

fn bands_sorted(p: &Point) -> Vec<String> {
    let mut v: Vec<String> = p.bands.keys().cloned().collect();
    v.sort();
    v
}

#[derive(Clone)]
struct MapVote {
    map: BTreeMap<String, i64>,
    votes: BTreeSet<String>,
}

fn enumerate_maps(bands: &[String], bound: i64) -> Vec<BTreeMap<String, i64>> {
    let mut out: Vec<BTreeMap<String, i64>> = vec![BTreeMap::new()];
    for b in bands {
        let mut next = Vec::new();
        for m in &out {
            for n in -bound..=bound {
                let mut mm = m.clone();
                mm.insert(b.clone(), n);
                next.push(mm);
            }
        }
        out = next;
    }
    out
}

fn norm(m: &BTreeMap<String, i64>) -> i64 {
    m.values().map(|v| v.unsigned_abs() as i64).sum()
}

fn map_offsets(m: &BTreeMap<String, i64>) -> Vec<OffsetPair> {
    m.iter()
        .map(|(band, cycles)| OffsetPair { band: band.clone(), cycles: *cycles })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn evaluate_step(
    input: &ComputeInput,
    sat: &SatId,
    a: &Point,
    b: &Point,
    clock_map: &BTreeMap<u64, String>,
) -> StepNode {
    let params = &input.params;
    let dt = b.time_s - a.time_s;
    let bands: Vec<String> = bands_sorted(b)
        .into_iter()
        .filter(|band| {
            b.bands.get(band).and_then(|x| x.phase_cycles).is_some()
                && a.bands.get(band).and_then(|x| x.phase_cycles).is_some()
        })
        .collect();

    // Previous nominal interval for irregular-gap test.
    let epochs = &input.bundle.obs.epochs;
    let prev_dt = nominal_prev_dt(epochs, b.epoch_index);
    let irregular = prev_dt.map(|p| dt > p * params.irregular_gap_factor + 1e-9).unwrap_or(false);

    let mut lli_flags = Vec::new();
    let mut missing_bands: Vec<String> = Vec::new();
    for (band, bv) in &b.bands {
        if bv.lli.is_some() {
            lli_flags.push(band.clone());
        }
        let in_a = a.bands.get(band).map(|x| !x.missing && x.phase_cycles.is_some());
        if bv.phase_cycles.is_none() {
            missing_bands.push(band.clone());
        }
        let _ = in_a;
    }
    missing_bands.sort();
    lli_flags.sort();

    let mut boundary_reasons: Vec<String> = Vec::new();
    if irregular {
        boundary_reasons.push("irregular-gap".into());
    }
    if b.epoch_index - a.epoch_index > 1 {
        boundary_reasons.push("missing-observation".into());
    }
    if !lli_flags.is_empty() {
        boundary_reasons.push("lli".into());
    }

    let mut residuals = ResidualReport::default();
    let mut td_res: BTreeMap<String, f64> = BTreeMap::new();
    let mut td_ok: BTreeMap<String, bool> = BTreeMap::new();
    let mut td_unknown = false;

    for band in &bands {
        let bva = a.bands.get(band).unwrap();
        let bvb = b.bands.get(band).unwrap();
        let (l0, l1) = (bva.phase_cycles.unwrap(), bvb.phase_cycles.unwrap());
        if let Some(exp) = expected_phase_cycles(bva, bvb, dt) {
            let r = l1 - l0 - exp;
            td_res.insert(band.clone(), r);
            let nearest = r.round() as i64;
            td_ok.insert(band.clone(), (r - nearest as f64).abs() <= params.td_tol_cycle);
            residuals.td_cycles.insert(
                band.clone(),
                if let (Some(c0), Some(c1)) = (bva.code_m, bvb.code_m) {
                    let _ = (c0, c1);
                    r
                } else {
                    r
                },
            );
            if let (Some(c0), Some(c1), Some(w)) = (bva.code_m, bvb.code_m, bvb.wavelength_m) {
                residuals.code_rate_m_s.insert(band.clone(), (c1 - c0) / dt * w / w);
            }
        } else {
            td_unknown = true;
        }
    }

    // Ionospheric geometry-free combination jump in metres.
    let b1 = bands.first().cloned();
    let b2 = bands.get(1).cloned();
    let mut iono_val: Option<f64> = None;
    let mut mw_val: Option<f64> = None;
    let mut iono_unknown = bands.len() < 2;
    let mut mw_unknown = bands.len() < 2;
    if let (Some(x), Some(y)) = (b1.clone(), b2.clone()) {
        if let (Some(avx), Some(avy), Some(bvx), Some(bvy)) = (
            a.bands.get(&x),
            a.bands.get(&y),
            b.bands.get(&x),
            b.bands.get(&y),
        ) {
            if let (Some(l1a), Some(l2a), Some(l1b), Some(l2b), Some(w1), Some(w2)) = (
                avx.phase_cycles,
                avy.phase_cycles,
                bvx.phase_cycles,
                bvy.phase_cycles,
                bvx.wavelength_m,
                bvy.wavelength_m,
            ) {
                let ia = l1a * w1 - l2a * w2;
                let ib = l1b * w1 - l2b * w2;
                iono_val = Some(ib - ia);
                residuals.iono_m = iono_val;
                if let (Some(p1a), Some(p2a), Some(p1b), Some(p2b)) =
                    (avx.code_m, avy.code_m, bvx.code_m, bvy.code_m)
                {
                    let f1 = SPEED_OF_LIGHT_M_S / w1;
                    let f2 = SPEED_OF_LIGHT_M_S / w2;
                    let wl_phase_a = (l1a * w1 * f1 - l2a * w2 * f2) / (f1 - f2);
                    let wl_phase_b = (l1b * w1 * f1 - l2b * w2 * f2) / (f1 - f2);
                    let nl_a = (f1 * p1a + f2 * p2a) / (f1 + f2);
                    let nl_b = (f1 * p1b + f2 * p2b) / (f1 + f2);
                    let _ = wl_phase_a;
                    mw_val = Some((wl_phase_b - nl_b) - (wl_phase_a - nl_a));
                    residuals.mw_m = mw_val;
                } else {
                    mw_unknown = true;
                }
            } else {
                iono_unknown = true;
                mw_unknown = true;
            }
        }
    }

    let mut votes: Vec<MapVote> = Vec::new();
    let enabled: BTreeSet<String> = params.enabled_detectors.iter().cloned().collect();
    let bound = params.candidate_search_cycles;
    let all_maps = enumerate_maps(&bands, bound);

    // Time-difference votes.
    if enabled.contains(DET_TD) && !td_unknown && bands.iter().all(|x| td_ok.contains_key(x)) {
        for m in &all_maps {
            let ok = bands.iter().all(|band| {
                let r = td_res[band];
                let n = m[band] as f64;
                (r - n).abs() <= params.td_tol_cycle
            });
            if ok {
                votes.push(MapVote { map: m.clone(), votes: BTreeSet::from([DET_TD.into()]) });
            }
        }
    }

    // Ionospheric GF votes.
    if enabled.contains(DET_IONO) && !iono_unknown {
        if let (Some(v), Some(x), Some(y)) = (iono_val, b1.clone(), b2.clone()) {
            let w1 = b.bands.get(&x).unwrap().wavelength_m.unwrap();
            let w2 = b.bands.get(&y).unwrap().wavelength_m.unwrap();
            for m in &all_maps {
                let corrected = v - m[&x] as f64 * w1 + m[&y] as f64 * w2;
                if corrected.abs() <= params.iono_tol_m {
                    votes.push(MapVote {
                        map: m.clone(),
                        votes: BTreeSet::from([DET_IONO.into()]),
                    });
                }
            }
        }
    }

    // Melbourne-Wubbena votes (widelane constraint).
    if enabled.contains(DET_MW) && !mw_unknown {
        if let (Some(v), Some(x), Some(y)) = (mw_val, b1.clone(), b2.clone()) {
            let w1 = b.bands.get(&x).unwrap().wavelength_m.unwrap();
            let w2 = b.bands.get(&y).unwrap().wavelength_m.unwrap();
            let f1 = SPEED_OF_LIGHT_M_S / w1;
            let f2 = SPEED_OF_LIGHT_M_S / w2;
            let lam_w = SPEED_OF_LIGHT_M_S / (f1 - f2);
            for m in &all_maps {
                // widelane integer implied by this offset map
                let w = m[&x] - m[&y];
                let corrected = v - w as f64 * lam_w;
                if corrected.abs() <= params.mw_tol_m {
                    votes.push(MapVote {
                        map: m.clone(),
                        votes: BTreeSet::from([DET_MW.into()]),
                    });
                }
            }
        }
    }

    let mut merged: BTreeMap<String, MapVote> = BTreeMap::new();
    for vote in votes {
        let key = serde_json::to_string(&map_offsets(&vote.map)).unwrap_or_default();
        merged
            .entry(key)
            .and_modify(|e| {
                for v in &vote.votes {
                    e.votes.insert(v.clone());
                }
            })
            .or_insert(vote);
    }
    let _ = &mut merged;

    let step_id = crate::util::hid(
        "step:",
        &format!(
            "{}:{}:{}:{}:{}",
            input.scope_id,
            sat.raw(),
            a.epoch_index,
            b.epoch_index,
            params.fingerprint()
        ),
    );

    let clock_here = clock_map.get(&b.epoch_index).cloned();
    let mut candidates = build_candidates(
        input,
        sat,
        b.epoch_index,
        &bands,
        iono_val,
        mw_val,
        &td_res,
        td_unknown,
        iono_unknown,
        mw_unknown,
        merged,
        &lli_flags,
        clock_here.clone(),
        &step_id,
    );

    candidates.sort_by(|x, y| {
        y.score
            .cmp(&x.score)
            .then_with(|| norm_key(&x.offsets).cmp(&norm_key(&y.offsets)))
            .then_with(|| x.id.cmp(&y.id))
    });

    let top_score = candidates.iter().map(|c| c.score).max().unwrap_or(0);
    let ties = candidates.iter().filter(|c| c.score == top_score).count();
    for c in &mut candidates {
        if c.status == "candidate" {
            c.ambiguous = c.score == top_score && ties > 1;
            c.tie_group_size = if c.score == top_score { ties } else { 0 };
        }
    }

    let status = if clock_here.is_some() {
        "clock-only"
    } else if candidates.iter().any(|c| c.status == "candidate" && c.score > 0)
        || candidates.iter().any(|c| c.lli_evidence)
    {
        "candidate"
    } else if !missing_bands.is_empty() || b.epoch_index - a.epoch_index > 1 {
        "missing-band"
    } else if td_unknown {
        "unknown"
    } else {
        "clean"
    };

    StepNode {
        id: step_id,
        sat: sat.clone(),
        epoch_index: b.epoch_index,
        prev_epoch_index: a.epoch_index,
        delta_seconds: dt,
        irregular_gap: irregular,
        clock_jump_id: clock_here,
        lli_flags,
        missing_bands,
        boundary_reasons,
        candidates,
        residuals,
        status: status.to_string(),
    }
}

fn nominal_prev_dt(epochs: &[RawEpoch], epoch_index: u64) -> Option<f64> {
    let idx = epochs.iter().position(|e| e.epoch_index == epoch_index)?;
    if idx >= 2 {
        Some(epochs[idx - 1].gps_time_s() - epochs[idx - 2].gps_time_s())
    } else {
        None
    }
}

fn td_status(unknown: bool, enabled: bool) -> String {
    if !enabled {
        "error".into()
    } else if unknown {
        "unknown".into()
    } else {
        "candidate".into()
    }
}

fn norm_key(offsets: &[OffsetPair]) -> i64 {
    offsets.iter().map(|o| o.cycles.unsigned_abs() as i64).sum()
}

#[allow(clippy::too_many_arguments)]
fn build_candidates(
    input: &ComputeInput,
    sat: &SatId,
    epoch_index: u64,
    bands: &[String],
    iono_val: Option<f64>,
    mw_val: Option<f64>,
    td_res: &BTreeMap<String, f64>,
    td_unknown: bool,
    iono_unknown: bool,
    mw_unknown: bool,
    merged: BTreeMap<String, MapVote>,
    lli_flags: &[String],
    clock_id: Option<String>,
    step_id: &str,
) -> Vec<Candidate> {
    let params = &input.params;
    let enabled: BTreeSet<String> = params.enabled_detectors.iter().cloned().collect();
    let epoch = input
        .bundle
        .obs
        .epochs
        .iter()
        .find(|e| e.epoch_index == epoch_index);
    let location = epoch
        .map(|e| e.location.clone())
        .unwrap_or_else(|| format!("epoch {epoch_index}"));

    let mut proposals: Vec<DetectorProposal> = Vec::new();
    proposals.push(DetectorProposal {
        detector: DET_TD.into(),
        status: td_status(td_unknown, enabled.contains(DET_TD)),
        residual: (!td_res.is_empty())
            .then(|| td_res.values().map(|v| v.abs()).fold(0f64, f64::max)),
        residual_unit: Some("cycle".into()),
        tolerance: Some(params.td_tol_cycle),
        delta_seconds: None,
        message: if td_unknown {
            Some("no code or Doppler to predict phase increment".into())
        } else if !enabled.contains(DET_TD) {
            Some("detector disabled by parameter revision".into())
        } else {
            None
        },
    });
    proposals.push(DetectorProposal {
        detector: DET_IONO.into(),
        status: td_status(iono_unknown, enabled.contains(DET_IONO)),
        residual: iono_val.map(|v| v.abs()),
        residual_unit: Some("m".into()),
        tolerance: Some(params.iono_tol_m),
        delta_seconds: None,
        message: if iono_unknown {
            Some("geometry-free combination needs two phase bands".into())
        } else if !enabled.contains(DET_IONO) {
            Some("detector disabled by parameter revision".into())
        } else {
            None
        },
    });
    proposals.push(DetectorProposal {
        detector: DET_MW.into(),
        status: td_status(mw_unknown, enabled.contains(DET_MW)),
        residual: mw_val.map(|v| v.abs()),
        residual_unit: Some("m".into()),
        tolerance: Some(params.mw_tol_m),
        delta_seconds: None,
        message: if mw_unknown {
            Some("Melbourne-Wubbena needs two phase bands plus code".into())
        } else if !enabled.contains(DET_MW) {
            Some("detector disabled by parameter revision".into())
        } else {
            None
        },
    });

    let mut candidates: Vec<Candidate> = Vec::new();

    // Whole-machine clock jumps are separated from per-satellite slips: the
    // only emitted "offset" is the zero pair and it is marked rejected-clock.
    if let Some(cid) = &clock_id {
        let zero: Vec<OffsetPair> = bands
            .iter()
            .map(|b| OffsetPair { band: b.clone(), cycles: 0 })
            .collect();
        let id = crate::util::hid("cand:", &format!("{step_id}:clock:zero"));
        candidates.push(Candidate {
            id,
            sat: sat.clone(),
            epoch_index,
            location: location.clone(),
            offsets: zero,
            score: 0,
            ambiguous: false,
            tie_group_size: 0,
            lli_evidence: false,
            clock_event_id: Some(cid.clone()),
            status: "rejected-clock".into(),
            proposals: proposals.clone(),
            reject_reasons: vec![
                "whole-machine clock jump: not a satellite cycle slip".into()
            ],
        });
        return candidates;
    }

    for mv in merged.values() {
        let offsets = map_offsets(&mv.map);
        let is_zero = offsets.iter().all(|o| o.cycles == 0);
        let id_material = format!(
            "{step_id}:{}:{}",
            serde_json::to_string(&offsets).unwrap_or_default(),
            params.fingerprint()
        );
        let id = crate::util::hid("cand:", &id_material);
        let mut cand_proposals = proposals.clone();
        // Mark each detector status according to whether this exact map passed.
        for p in &mut cand_proposals {
            let voted = mv.votes.contains(&p.detector);
            if enabled.contains(&p.detector) && !voted {
                p.status = "none".into();
            }
        }
        let score = mv.votes.len() as i64;
        let lli_here = offsets
            .iter()
            .any(|o| lli_flags.contains(&o.band) && o.cycles != 0)
            || (is_zero && !lli_flags.is_empty());
        candidates.push(Candidate {
            id,
            sat: sat.clone(),
            epoch_index,
            location: location.clone(),
            offsets,
            score,
            ambiguous: false,
            tie_group_size: 0,
            lli_evidence: lli_here,
            clock_event_id: None,
            status: "candidate".into(),
            proposals: cand_proposals,
            reject_reasons: Vec::new(),
        });
    }
    candidates
}

pub fn compute(input: &ComputeInput) -> ComputeResult {
    let mut input = input.clone();
    input.wavelengths = wavelength_map(&input);
    let series = build_series(&input);
    let clock_jumps = detect_clock_jumps(&input, &series);
    let clock_map: BTreeMap<u64, String> =
        clock_jumps.iter().map(|c| (c.epoch_index, c.id.clone())).collect();

    let epochs = &input.bundle.obs.epochs;
    let nominal_dt = if epochs.len() >= 2 {
        let diffs: Vec<f64> = epochs
            .iter()
            .zip(epochs.iter().skip(1))
            .map(|(a, b)| b.gps_time_s() - a.gps_time_s())
            .collect();
        median(diffs)
    } else {
        30.0
    };
    let _ = nominal_dt;

    let mut steps = Vec::new();
    for s in series.values() {
        for w in s.points.windows(2) {
            let step = evaluate_step(&input, &s.sat, &w[0], &w[1], &clock_map);
            steps.push(step);
        }
    }
    steps.sort_by(|a, b| {
        a.sat
            .raw()
            .cmp(&b.sat.raw())
            .then(a.epoch_index.cmp(&b.epoch_index))
            .then(a.id.cmp(&b.id))
    });

    let arcs = build_arcs(&input, &series, &steps);
    ComputeResult {
        steps,
        arcs,
        clock_jumps,
        wavelengths: input.wavelengths.clone(),
        diagnostics: Vec::new(),
    }
}

/// Build one arc per (satellite, band). Arcs split at irregular gaps, missing
/// epochs/bands, LLI flags and clock-jump steps. A detector-only candidate
/// (no raw boundary marker) does NOT split the arc: the repair lives only in
/// the offset layer.
fn build_arcs(
    input: &ComputeInput,
    series: &BTreeMap<String, SatSeries>,
    steps: &[StepNode],
) -> Vec<ArcNode> {
    let mut arcs = Vec::new();
    for s in series.values() {
        let bands_all: BTreeSet<String> = s
            .points
            .iter()
            .flat_map(|p| p.bands.keys().cloned())
            .collect();
        for band in &bands_all {
            let valid: Vec<&Point> = s
                .points
                .iter()
                .filter(|p| p.bands.get(band).and_then(|x| x.phase_cycles).is_some())
                .collect();
            let mut arc_index = 0usize;
            let mut run: Vec<&Point> = Vec::new();
            let mut start_reason = "series-start".to_string();
            let mut arc_step_ids: Vec<String> = Vec::new();
            let mut prev: Option<&Point> = None;
            let mut reasons_at: BTreeMap<u64, Vec<String>> = BTreeMap::new();
            for st in steps.iter().filter(|st| st.sat == s.sat) {
                reasons_at.entry(st.epoch_index).or_default().extend(st.boundary_reasons.clone());
            }
            let clock_epochs: BTreeSet<u64> = steps
                .iter()
                .filter(|st| st.sat == s.sat && st.clock_jump_id.is_some())
                .map(|st| st.epoch_index)
                .collect();
            for point in valid {
                if let Some(p0) = prev {
                    let mut split = false;
                    let mut end_reason = String::new();
                    let dt = point.time_s - p0.time_s;
                    if point.epoch_index - p0.epoch_index > 1 {
                        split = true;
                        end_reason = "missing-observation".into();
                    }
                    if let Some(r) = reasons_at.get(&point.epoch_index) {
                        if r.contains(&"irregular-gap".to_string()) {
                            split = true;
                            end_reason = "irregular-gap".into();
                        }
                        if r.contains(&"lli".to_string()) {
                            split = true;
                            end_reason = "lli".into();
                        }
                    }
                    if clock_epochs.contains(&point.epoch_index) {
                        split = true;
                        end_reason = "clock-jump".into();
                    }
                    let _ = dt;
                    if split {
                        if !run.is_empty() {
                            arcs.push(make_arc(
                                input,
                                &s.sat,
                                band,
                                arc_index,
                                &run,
                                &start_reason,
                                &end_reason,
                                &arc_step_ids,
                            ));
                            arc_index += 1;
                        }
                        run = Vec::new();
                        arc_step_ids.clear();
                        start_reason = end_reason;
                    }
                    if let Some(st) = steps.iter().find(|st| {
                        st.sat == s.sat
                            && st.prev_epoch_index == p0.epoch_index
                            && st.epoch_index == point.epoch_index
                    }) {
                        arc_step_ids.push(st.id.clone());
                    }
                }
                run.push(point);
                prev = Some(point);
            }
            if !run.is_empty() {
                arcs.push(make_arc(
                    input,
                    &s.sat,
                    band,
                    arc_index,
                    &run,
                    &start_reason,
                    "series-end",
                    &arc_step_ids,
                ));
            }
        }
    }
    arcs.sort_by(|a, b| {
        a.sat
            .raw()
            .cmp(&b.sat.raw())
            .then(a.band.cmp(&b.band))
            .then(a.arc_index.cmp(&b.arc_index))
    });
    arcs
}

#[allow(clippy::too_many_arguments)]
fn make_arc(
    input: &ComputeInput,
    sat: &SatId,
    band: &str,
    arc_index: usize,
    run: &[&Point],
    start_reason: &str,
    end_reason: &str,
    step_ids: &[String],
) -> ArcNode {
    let start_epoch = run.first().unwrap().epoch_index;
    let end_epoch = run.last().unwrap().epoch_index;
    let id = crate::util::hid(
        "arc:",
        &format!(
            "{}:{}:{}:{}:{}:{}:{}",
            input.scope_id,
            sat.raw(),
            band,
            arc_index,
            start_epoch,
            end_epoch,
            start_reason
        ),
    );
    ArcNode {
        id,
        sat: sat.clone(),
        arc_index,
        band: band.into(),
        start_epoch,
        end_epoch,
        start_reason: start_reason.into(),
        end_reason: end_reason.into(),
        point_count: run.len(),
        step_ids: step_ids.to_vec(),
    }
}
