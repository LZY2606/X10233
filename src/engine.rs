//! Domain state machine: arcs, clock vs slip separation, multi-detector
//! candidates, decisions/repair layer and partial invalidation.
//!
//! The engine is pure: given the current input snapshot, parameters, a set of
//! accepted repair offsets and virtual time, it produces a fully identified
//! set of findings. Stability rules:
//!
//! * every row has a content-derived stable id, prefixed by kind;
//! * clock jumps and single-satellite slips are distinct findings;
//! * repairs exist only in the offset layer, raw phase is never rewritten;
//! * GF, MW and TD can disagree; all legal (n1,n2) pairs are retained.
use crate::detector::{self, Bands, BandSample, Hypothesis, PairResiduals, PairSample};
use crate::hash::{fingerprint_ser, sha256_short};
use crate::model::{ImportedFile, Params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochView {
    pub seq: i64,
    pub time_raw: String,
    pub t_seconds: f64,
    pub unit: String,
    pub time_diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcView {
    pub id: String,
    pub station: String,
    pub satellite: String,
    pub band: String,
    pub start_epoch: String,
    pub end_epoch: String,
    pub start_seq: i64,
    pub end_seq: i64,
    pub boundary_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateView {
    pub id: String,
    pub n1: i64,
    pub n2: i64,
    pub score: f64,
    pub legal: bool,
    pub td_passes: bool,
    pub gf_z: Option<f64>,
    pub mw_z: Option<f64>,
    pub td1_z: Option<f64>,
    pub td2_z: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualView {
    pub dt_s: f64,
    pub irregular: bool,
    pub gf_jump_m: Option<f64>,
    pub mw_jump_m: Option<f64>,
    pub td_cycles_l1: Option<f64>,
    pub td_cycles_l2: Option<f64>,
    pub code_common_jump_m: Option<f64>,
    pub gf_series_m: Option<f64>,
    pub mw_series_m: Option<f64>,
    pub unusable_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingView {
    pub id: String,
    pub kind: String, // clock_jump | cycle_slip | lli_only | frequency_missing
    pub station: String,
    pub satellite: String,
    pub band: String,
    pub epoch: String,
    pub epoch_seq: i64,
    pub subject_key: String,
    pub source_direct: Vec<String>,
    pub algo_version: String,
    pub param_fingerprint: String,
    pub input_fingerprint: String,
    pub residuals: Option<ResidualView>,
    pub candidates: Vec<CandidateView>,
    pub td_pick: Option<(i64, i64)>,
    pub unique_pair: Option<(i64, i64)>,
    pub lli_flags: Vec<i64>,
    pub status: String,
    pub reject_reason: Option<String>,
    pub clock_event_us: Option<f64>,
    pub clock_event_m: Option<f64>,
    pub message: String,
    pub accepted: Option<(i64, i64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionState {
    /// subject_key -> accepted pair (n1,n2)
    pub accepted: std::collections::BTreeMap<String, (i64, i64)>,
    /// subject_key -> rejected marker
    pub rejected: std::collections::BTreeSet<String>,
    /// clock subjects explicitly acknowledged (not a repair)
    pub acknowledged: std::collections::BTreeSet<String>,
}

impl DecisionState {
    pub fn new() -> Self {
        Self {
            accepted: Default::default(),
            rejected: Default::default(),
            acknowledged: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComputeInput<'a> {
    pub file: &'a ImportedFile,
    pub params: &'a Params,
    pub input_fingerprint: String,
    pub decisions: &'a DecisionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectView {
    pub station: String,
    pub datum: String,
    pub units: crate::model::Units,
    pub epochs: Vec<EpochView>,
    pub arcs: Vec<ArcView>,
    pub findings: Vec<FindingView>,
    pub satellites: Vec<String>,
    pub bands: Vec<(f64, f64)>,
    pub diagnostics: Vec<crate::model::ParseDiagnostic>,
}

const CODE_L1: &str = "C1C";
const PHASE_L1: &str = "L1C";
const DOP_L1: &str = "D1C";
const CODE_L2: &str = "C2W";
const PHASE_L2: &str = "L2W";
const DOP_L2: &str = "D2W";

fn station_id(f: &ImportedFile) -> String {
    if f.station.id.is_empty() {
        "ST01".into()
    } else {
        f.station.id.clone()
    }
}

fn bands(f: &ImportedFile) -> Bands {
    let mut l1 = 0.190_293_672_798_404_04;
    let mut l2 = 0.244_210_213_424_651_2;
    for (band, lam) in &f.wavelengths {
        match band.as_str() {
            "L1" => l1 = *lam,
            "L2" => l2 = *lam,
            _ => {}
        }
    }
    Bands { l1, l2 }
}

fn times(f: &ImportedFile) -> Vec<f64> {
    let base = f.epochs.first().and_then(|e| e.time_s).unwrap_or(0.0);
    f.epochs.iter().map(|e| e.time_s.unwrap_or(base) - base).collect()
}

fn nominal_dt(ts: &[f64]) -> f64 {
    let mut diffs: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    diffs.first().copied().unwrap_or(30.0)
}

/// Stable subject identity shared across recomputes / export-import.
pub fn slip_subject(station: &str, sat: &str, epoch: &str) -> String {
    format!("slip:{station}:{sat}:{epoch}")
}
pub fn clock_subject(station: &str, epoch: &str) -> String {
    format!("clock:{station}:{epoch}")
}
pub fn lli_subject(station: &str, sat: &str, epoch: &str) -> String {
    format!("lli:{station}:{sat}:{epoch}")
}
pub fn missing_subject(station: &str, sat: &str, epoch: &str, band: &str) -> String {
    format!("missing:{station}:{sat}:{epoch}:{band}")
}

fn sample_of(file: &ImportedFile, ei: usize, sat: &str, bands_used: bool) -> Option<PairSample> {
    let e = file.epochs.get(ei)?;
    let so = e.sat(sat)?;
    let _ = bands_used;
    let pick = |code: &str| -> Option<f64> {
        so.find(code).and_then(|m| m.value_f64).filter(|_| {
            so.find(code).map(|m| !m.missing && m.diagnostic.is_none()).unwrap_or(false)
        })
    };
    let lli = |band: &str| -> Option<i64> {
        let code = format!("LLI{band}");
        so.find(&code).and_then(|m| m.value_int)
    };
    Some(PairSample {
        a: BandSample {
            code_m: pick(CODE_L1),
            phase_cyc: pick(PHASE_L1),
            doppler_hz: pick(DOP_L1),
            lli: lli("1"),
        },
        b: BandSample {
            code_m: pick(CODE_L2),
            phase_cyc: pick(PHASE_L2),
            doppler_hz: pick(DOP_L2),
            lli: lli("2"),
        },
    })
}

fn has_l2(file: &ImportedFile, ei: usize, sat: &str) -> bool {
    file.epochs
        .get(ei)
        .and_then(|e| e.sat(sat))
        .and_then(|so| so.find(PHASE_L2))
        .map(|m| !m.missing && m.value_f64.is_some())
        .unwrap_or(false)
}
fn has_l1(file: &ImportedFile, ei: usize, sat: &str) -> bool {
    file.epochs
        .get(ei)
        .and_then(|e| e.sat(sat))
        .and_then(|so| so.find(PHASE_L1))
        .map(|m| !m.missing && m.value_f64.is_some())
        .unwrap_or(false)
}

/// Median *anomalous* common code advance across satellites at one boundary.
/// Normal motion (thousands of metres over 30 s) is predicted from mean
/// Doppler; the residual is near zero except for a receiver clock jump, which
/// is identical on every satellite. This is what separates a whole-unit clock
/// event from a single-satellite cycle slip.
fn common_clock_jump(file: &ImportedFile, i: usize, ts: &[f64], bandsv: &Bands) -> Option<f64> {
    let dt = ts[i] - ts[i - 1];
    let mut jumps = vec![];
    let sats = satellites(file);
    for sat in &sats {
        let p = sample_of(file, i - 1, sat, true)?;
        let c = sample_of(file, i, sat, true)?;
        if let Some(res) = code_anomaly(&p, &c, dt, bandsv) {
            if res.is_finite() {
                jumps.push(res);
            }
        }
    }
    if jumps.is_empty() {
        return None;
    }
    jumps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(jumps[jumps.len() / 2])
}

/// Code increment not explained by mean Doppler (mean of both bands).
fn code_anomaly(p: &PairSample, c: &PairSample, dt: f64, b: &Bands) -> Option<f64> {
    let band_anom = |prev: detector::BandSample, cur: detector::BandSample, lam: f64| -> Option<f64> {
        let predicted_advance = -(prev.doppler_hz? + cur.doppler_hz?) * 0.5 * dt * lam;
        Some(cur.code_m? - prev.code_m? - predicted_advance)
    };
    let a1 = band_anom(p.a, c.a, b.l1)?;
    let a2 = band_anom(p.b, c.b, b.l2)?;
    Some((a1 + a2) / 2.0)
}

fn satellites(f: &ImportedFile) -> Vec<String> {
    let mut v: Vec<String> = f
        .epochs
        .iter()
        .flat_map(|e| e.sats.iter().map(|s| s.satellite.clone()))
        .collect();
    v.sort();
    v.dedup();
    v
}

fn source_keys_for_pair(file: &ImportedFile, i: usize, sat: &str) -> Vec<String> {
    let mut keys = vec![];
    for ei in [i - 1, i] {
        for code in [CODE_L1, PHASE_L1, DOP_L1, CODE_L2, PHASE_L2, DOP_L2, "LLI1", "LLI2"] {
            keys.push(meas_key(file, ei, sat, code));
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn meas_key(file: &ImportedFile, ei: usize, sat: &str, code: &str) -> String {
    let time = file.epochs.get(ei).map(|e| e.time_raw.clone()).unwrap_or_else(|| format!("e{ei}"));
    let raw = file
        .epochs
        .get(ei)
        .and_then(|e| e.sat(sat))
        .and_then(|so| so.find(code))
        .map(|m| m.raw.clone())
        .unwrap_or("ABSENT".into());
    format!("meas:{sat}:{time}:{code}={raw}")
}

/// Recompute the full view for one (input version, param version, decision
/// state) triple. Deterministic; safe to call at any historical point.
pub fn compute(input: ComputeInput) -> ProjectView {
    let file = input.file;
    let params = input.params;
    let station = station_id(file);
    let bandsv = bands(file);
    let ts = times(file);
    let nominal = nominal_dt(&ts);
    let sats = satellites(file);

    let epochs: Vec<EpochView> = file
        .epochs
        .iter()
        .map(|e| EpochView {
            seq: e.seq,
            time_raw: e.time_raw.clone(),
            t_seconds: e.time_s.unwrap_or(0.0),
            unit: e.time_unit.clone(),
            time_diagnostic: e.time_diagnostic.clone(),
        })
        .collect();

    let mut findings: Vec<FindingView> = vec![];

    // Epoch-boundary findings: frequency gaps, LLI flags and clock events.
    // Pair-wise slip findings are evaluated across every adjacent boundary.
    for i in 1..file.epochs.len() {
        let dt = ts[i] - ts[i - 1];
        let irregular = dt > nominal * 1.5 + 1e-9 || dt < nominal * 0.5 - 1e-9;
        let epoch_now = file.epochs[i].time_raw.clone();

        // Whole-unit receiver clock jump (common across all satellites).
        let common = common_clock_jump(file, i, &ts, &bandsv).unwrap_or(0.0);
        let explicit_clock = file.epochs[i].clock_events.first();
        let clock_detected = common.abs() > params.clock_sigma_m;
        if clock_detected || explicit_clock.is_some() {
            let subject = clock_subject(&station, &epoch_now);
            let status = if input.decisions.acknowledged.contains(&subject) {
                "acknowledged".into()
            } else {
                "open".into()
            };
            let mut src = vec![format!("clock-event:{epoch_now}")];
            for sat in &sats {
                for code in [CODE_L1, CODE_L2, PHASE_L1, PHASE_L2] {
                    src.push(meas_key(file, i, sat, code));
                }
            }
            src.sort();
            src.dedup();
            findings.push(FindingView {
                id: stable_id("finding-clock", &[&station, &epoch_now]),
                kind: "clock_jump".into(),
                station: station.clone(),
                satellite: "*".into(),
                band: "*".into(),
                epoch: epoch_now.clone(),
                epoch_seq: i as i64,
                subject_key: subject,
                source_direct: src,
                algo_version: params.algo_version.clone(),
                param_fingerprint: fingerprint_ser(params),
                input_fingerprint: input.input_fingerprint.clone(),
                residuals: None,
                candidates: vec![],
                td_pick: None,
                unique_pair: None,
                lli_flags: vec![],
                status,
                reject_reason: None,
                clock_event_us: explicit_clock.and_then(|e| e.jump_us),
                clock_event_m: Some(common),
                message: format!(
                    "receiver clock jump: common code/phase jump {:.1} m ({:.1} us); kept separate from single-satellite slips",
                    common,
                    common / detector::C_LIGHT * 1e6
                ),
                accepted: None,
            });
        }

        for sat in &sats {
            let now = file.epochs[i].sat(sat);
            // Missing frequency -> boundary finding + arc split reason.
            for (present, band_lbl, code) in [
                (has_l1(file, i, sat), "L1", PHASE_L1),
                (has_l2(file, i, sat), "L2", PHASE_L2),
            ] {
                if !present {
                    let subject = missing_subject(&station, sat, &epoch_now, band_lbl);
                    findings.push(FindingView {
                        id: stable_id("finding-missing", &[&station, sat, &epoch_now, band_lbl]),
                        kind: "frequency_missing".into(),
                        station: station.clone(),
                        satellite: sat.clone(),
                        band: band_lbl.into(),
                        epoch: epoch_now.clone(),
                        epoch_seq: i as i64,
                        subject_key: subject,
                        source_direct: vec![meas_key(file, i, sat, code)],
                        algo_version: params.algo_version.clone(),
                        param_fingerprint: fingerprint_ser(params),
                        input_fingerprint: input.input_fingerprint.clone(),
                        residuals: None,
                        candidates: vec![],
                        td_pick: None,
                        unique_pair: None,
                        lli_flags: vec![],
                        status: "diagnostic".into(),
                        reject_reason: None,
                        clock_event_us: None,
                        clock_event_m: None,
                        message: format!("{sat} {band_lbl} missing at {epoch_now}; arc boundary, pair tests skipped"),
                        accepted: None,
                    });
                }
            }

            // LLI false alarm: flag raised but continuous code/phase.
            let lli_vals: Vec<i64> = now
                .map(|so| so.lli_flags.iter().map(|(_, v)| *v).collect())
                .unwrap_or_default();
            let lli_raised = lli_vals.iter().any(|v| *v != 0);
            if lli_raised {
                let subject = lli_subject(&station, sat, &epoch_now);
                let prev_s = sample_of(file, i - 1, sat, true);
                let cur_s = sample_of(file, i, sat, true);
                let r = match (prev_s, cur_s) {
                    (Some(p), Some(c)) => Some(detector::residuals(&p, &c, dt, irregular, &bandsv, common)),
                    _ => None,
                };
                let continuous = r
                    .as_ref()
                    .map(|rr| {
                        rr.gf_jump_m.map(|v| v.abs() <= params.gf_sigma_m).unwrap_or(true)
                            && rr.mw_jump_m.map(|v| v.abs() <= params.mw_sigma_m).unwrap_or(true)
                    })
                    .unwrap_or(true);
                let (status, reject) = if input.decisions.acknowledged.contains(&subject) {
                    ("acknowledged_lli", None)
                } else if continuous {
                    ("rejected_continuous", Some("LLI flag raised but GF/MW remain continuous".to_string()))
                } else {
                    ("open", None)
                };
                findings.push(FindingView {
                    id: stable_id("finding-lli", &[&station, sat, &epoch_now]),
                    kind: "lli_only".into(),
                    station: station.clone(),
                    satellite: sat.clone(),
                    band: "L1/L2".into(),
                    epoch: epoch_now.clone(),
                    epoch_seq: i as i64,
                    subject_key: subject,
                    source_direct: source_keys_for_pair(file, i, sat),
                    algo_version: params.algo_version.clone(),
                    param_fingerprint: fingerprint_ser(params),
                    input_fingerprint: input.input_fingerprint.clone(),
                    residuals: r.as_ref().map(|rr| residual_view(rr)),
                    candidates: vec![],
                    td_pick: None,
                    unique_pair: None,
                    lli_flags: lli_vals.clone(),
                    status: status.into(),
                    reject_reason: reject,
                    clock_event_us: None,
                    clock_event_m: None,
                    message: "LLI bit set; detector result retained even when judged a false alarm".into(),
                    accepted: None,
                });
            }

            // Cycle-slip pair test (GF / MW / TD). Clock jump does NOT enter
            // GF and is fully common to MW/TD, so slips remain separable.
            if has_l1(file, i - 1, sat)
                && has_l2(file, i - 1, sat)
                && has_l1(file, i, sat)
                && has_l2(file, i, sat)
            {
                let p = sample_of(file, i - 1, sat, true).expect("samples");
                let c = sample_of(file, i, sat, true).expect("samples");
                let r = detector::residuals(&p, &c, dt, irregular, &bandsv, common);
                let hyps = detector::evaluate_hypotheses(
                    &r,
                    &bandsv,
                    params.gf_sigma_m,
                    params.mw_sigma_m,
                    params.td_sigma_cycles,
                    params.max_lattice,
                );
                let gf_mw_trigger = r
                    .gf_jump_m
                    .map(|v| v.abs() > params.gf_sigma_m)
                    .unwrap_or(false)
                    || r
                        .mw_jump_m
                        .map(|v| v.abs() > params.mw_sigma_m)
                        .unwrap_or(false);
                let td_trigger = r
                    .td_cycles_l1
                    .map(|v| v.abs() > params.td_sigma_cycles)
                    .unwrap_or(false)
                    || r
                        .td_cycles_l2
                        .map(|v| v.abs() > params.td_sigma_cycles)
                        .unwrap_or(false);
                if (gf_mw_trigger || td_trigger) && !hyps.is_empty() {
                    let subject = slip_subject(&station, sat, &epoch_now);
                    let candidates: Vec<CandidateView> = hyps
                        .iter()
                        .map(|h| CandidateView {
                            id: stable_id(
                                "cand",
                                &[&station, sat, &epoch_now, &h.n1.to_string(), &h.n2.to_string()],
                            ),
                            n1: h.n1,
                            n2: h.n2,
                            score: h.score,
                            legal: h.is_legal(),
                            td_passes: h.td_passes(),
                            gf_z: h.gf.as_ref().map(|s| s.residual),
                            mw_z: h.mw.as_ref().map(|s| s.residual),
                            td1_z: h.td1.as_ref().map(|s| s.residual),
                            td2_z: h.td2.as_ref().map(|s| s.residual),
                        })
                        .collect();
                    let td_pick = Hypothesis::td_pick(&r);
                    let nonzero: Vec<Hypothesis> = hyps.into_iter().filter(|h| !(h.n1 == 0 && h.n2 == 0)).collect();
                    let hyps = nonzero;
                    let unique_pair = unique_identified(&hyps);
                    let status = if let Some(acc) = input.decisions.accepted.get(&subject) {
                        format!("accepted:{}:{}", acc.0, acc.1)
                    } else if input.decisions.rejected.contains(&subject) {
                        "rejected".into()
                    } else {
                        "open".into()
                    };
                    let accepted = input.decisions.accepted.get(&subject).copied();
                    findings.push(FindingView {
                        id: stable_id("finding-slip", &[&station, sat, &epoch_now]),
                        kind: "cycle_slip".into(),
                        station: station.clone(),
                        satellite: sat.clone(),
                        band: "L1/L2".into(),
                        epoch: epoch_now.clone(),
                        epoch_seq: i as i64,
                        subject_key: subject,
                        source_direct: source_keys_for_pair(file, i, sat),
                        algo_version: params.algo_version.clone(),
                        param_fingerprint: fingerprint_ser(params),
                        input_fingerprint: input.input_fingerprint.clone(),
                        residuals: Some(residual_view(&r)),
                        candidates,
                        td_pick,
                        unique_pair,
                        lli_flags: lli_vals,
                        status,
                        reject_reason: None,
                        clock_event_us: None,
                        clock_event_m: None,
                        message: detector_message(&r),
                        accepted,
                    });
                }
            }
        }
    }

    let arcs = build_arcs(input, &sats, &ts, nominal);
    findings.sort_by(|a, b| {
        a.epoch_seq
            .cmp(&b.epoch_seq)
            .then(a.kind.cmp(&b.kind))
            .then(a.satellite.cmp(&b.satellite))
    });

    ProjectView {
        station,
        datum: file.station.datum.clone(),
        units: file.units.clone(),
        epochs,
        arcs,
        findings,
        satellites: sats,
        bands: vec![(bandsv.l1, bandsv.l2)],
        diagnostics: file.diagnostics.clone(),
    }
}

fn residual_view(r: &PairResiduals) -> ResidualView {
    ResidualView {
        dt_s: r.dt_s,
        irregular: r.irregular,
        gf_jump_m: r.gf_jump_m,
        mw_jump_m: r.mw_jump_m,
        td_cycles_l1: r.td_cycles_l1,
        td_cycles_l2: r.td_cycles_l2,
        code_common_jump_m: r.code_common_jump_m,
        gf_series_m: r.gf_series_m,
        mw_series_m: r.mw_series_m,
        unusable_reasons: r.unusable_reasons.clone(),
    }
}

fn detector_message(r: &PairResiduals) -> String {
    format!(
        "GF jump {:?} m; MW jump {:?} m; TD cycles L1 {:?} / L2 {:?}{}",
        r.gf_jump_m.map(|v| (v * 1000.0).round() / 1000.0),
        r.mw_jump_m.map(|v| (v * 1000.0).round() / 1000.0),
        r.td_cycles_l1.map(|v| (v * 1000.0).round() / 1000.0),
        r.td_cycles_l2.map(|v| (v * 1000.0).round() / 1000.0),
        if r.irregular { " [irregular epoch spacing]" } else { "" }
    )
}

/// A pair is uniquely identified only when exactly one legal hypothesis also
/// passes the independent Doppler time-difference test.
fn unique_identified(hyps: &[Hypothesis]) -> Option<(i64, i64)> {
    let passing: Vec<(i64, i64)> = hyps
        .iter()
        .filter(|h| h.is_legal() && h.td_passes())
        .map(|h| (h.n1, h.n2))
        .collect();
    if passing.len() == 1 {
        passing.first().copied()
    } else {
        None
    }
}

pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut joined = String::new();
    for p in parts {
        joined.push_str(p);
        joined.push('|');
    }
    format!("{prefix}-{}", sha256_short(joined.as_bytes()))
}

/// Phase-continuity arcs per (satellite, band). Boundaries are created at
/// missing frequencies and at accepted single-satellite slip offsets (the
/// repair lives in the offset layer and starts a new arc).
fn build_arcs(
    input: ComputeInput,
    sats: &[String],
    ts: &[f64],
    nominal: f64,
) -> Vec<ArcView> {
    let file = input.file;
    let station = station_id(file);
    let mut arcs = vec![];
    for sat in sats {
        for (band, present_fn, code) in [
            ("L1", has_l1 as fn(&ImportedFile, usize, &str) -> bool, PHASE_L1),
            ("L2", has_l2 as fn(&ImportedFile, usize, &str) -> bool, PHASE_L2),
        ] {
            let _ = code;
            let mut start: Option<usize> = None;
            let mut boundary_reasons: Vec<String> = vec![];
            for i in 0..file.epochs.len() {
                let present = present_fn(file, i, sat);
                if present && start.is_none() {
                    start = Some(i);
                    boundary_reasons = vec!["arc_start".into()];
                    if i > 0 {
                        boundary_reasons.push(format!("after gap: {} missing", band));
                    }
                }
                let mut close = false;
                let mut reason = String::new();
                if !present {
                    close = start.is_some();
                    reason = format!("{} missing at {}", band, file.epochs[i].time_raw);
                } else if i > 0 {
                    let epoch = &file.epochs[i].time_raw;
                    let subject = slip_subject(&station, sat, epoch);
                    if let Some((n1, n2)) = input.decisions.accepted.get(&subject) {
                        let n = if band == "L1" { n1 } else { n2 };
                        close = start.is_some();
                        reason = format!("accepted repair offset {band}={n} cycles at {epoch}");
                    }
                    let dt = ts[i] - ts[i - 1];
                    if dt > nominal * 1.5 + 1e-9 || dt < nominal * 0.5 - 1e-9 {
                        // irregular spacing alone is flagged on each pair but
                        // does not split the arc.
                    }
                }
                if close {
                    if let Some(s) = start.take() {
                        if !reason.is_empty() {
                            boundary_reasons.push(reason.clone());
                        }
                        arcs.push(make_arc(&station, sat, band, file, s, i - 1, &boundary_reasons));
                    }
                }
            }
            if let Some(s) = start {
                boundary_reasons.push("arc_end".into());
                arcs.push(make_arc(&station, sat, band, file, s, file.epochs.len() - 1, &boundary_reasons));
            }
        }
    }
    arcs.sort_by(|a, b| {
        a.satellite
            .cmp(&b.satellite)
            .then(a.band.cmp(&b.band))
            .then(a.start_seq.cmp(&b.start_seq))
    });
    arcs
}

fn make_arc(
    station: &str,
    sat: &str,
    band: &str,
    file: &ImportedFile,
    s: usize,
    e: usize,
    reasons: &[String],
) -> ArcView {
    let start_epoch = file.epochs[s].time_raw.clone();
    let end_epoch = file.epochs[e].time_raw.clone();
    ArcView {
        id: stable_id("arc", &[station, sat, band, &s.to_string(), &e.to_string()]),
        station: station.into(),
        satellite: sat.into(),
        band: band.into(),
        start_epoch,
        end_epoch,
        start_seq: s as i64,
        end_seq: e as i64,
        boundary_reasons: reasons.to_vec(),
    }
}
