use crate::hash::stable_id;
use crate::model::{band, RuleParams, ALGORITHM_VERSION};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Frequency {
    pub system: String,
    pub carrier_code: String,
    pub mhz: f64,
}

#[derive(Debug, Clone)]
pub struct ObsField {
    pub field_id: String,
    pub epoch: String,
    pub t: f64,
    pub sat: String,
    pub code: String,
    pub value: Option<f64>,
    pub lli: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct Clock {
    pub epoch: String,
    pub t: f64,
    pub kind: String,
    pub delta_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Repair {
    pub sat: String,
    pub epoch: String,
    pub band: String,
    pub cycles: f64,
}

pub struct EngineInput<'a> {
    pub project_id: &'a str,
    pub input_version_id: &'a str,
    pub observations: &'a [ObsField],
    pub frequencies: &'a [Frequency],
    pub clocks: &'a [Clock],
    pub params: &'a RuleParams,
    pub accepted_repairs: &'a [Repair],
}

pub fn parameter_fingerprint(params: &RuleParams) -> String {
    stable_id(&[
        "rule-params-v1",
        &params.integer_search_radius.to_string(),
        &params.td_sigma_cycles.to_string(),
        &params.mw_sigma_cycles.to_string(),
        &params.iono_tolerance_m.to_string(),
        &params.lli_low_snr_dbhz.to_string(),
    ])
}

pub fn run(input: EngineInput) -> Value {
    let freq: BTreeMap<String, f64> = input
        .frequencies
        .iter()
        .map(|f| (f.carrier_code.clone(), f.mhz))
        .collect();
    let clock_epochs: BTreeSet<String> = input.clocks.iter().map(|c| c.epoch.clone()).collect();
    let mut epochs: BTreeSet<(f64, String)> = BTreeSet::new();
    let mut signals: BTreeSet<(String, String)> = BTreeSet::new();
    let mut by_key: BTreeMap<(String, String, String), ObsField> = BTreeMap::new();
    for obs in input.observations {
        if obs.status == "unconvertible" {
            continue;
        }
        epochs.insert((obs.t, obs.epoch.clone()));
        if obs.code.starts_with('L') {
            signals.insert((obs.sat.clone(), band(&obs.code)));
        }
        by_key.insert((obs.sat.clone(), obs.epoch.clone(), obs.code.clone()), obs.clone());
    }
    let epoch_list: Vec<(f64, String)> = epochs.into_iter().collect();
    let mut satellites: BTreeSet<String> = BTreeSet::new();
    let mut sig_list: Vec<(String, String)> = signals.into_iter().collect();
    sig_list.sort();
    for (sat, _) in &sig_list {
        satellites.insert(sat.clone());
    }
    let satellites: Vec<String> = satellites.into_iter().collect();

    let mut arcs = Vec::new();
    let mut boundaries = Vec::new();
    let mut findings = Vec::new();
    let mut candidates = Vec::new();

    for (sat, sig_band) in &sig_list {
        let l_code = format!("L{sig_band}");
        let c_code = format!("C{sig_band}");
        let mut open_start: Option<(f64, String, String)> = None;
        let mut previous: Option<ObsField> = None;
        for (_, epoch) in &epoch_list {
            let carrier = by_key.get(&(sat.clone(), epoch.clone(), l_code)).cloned();
            if carrier.is_none() {
                if let Some(prev) = &previous {
                    let reason = if clock_epochs.contains(epoch) { "receiver-clock-jump" } else { "frequency-missing" };
                    close_arc(&mut arcs, &input, sat, &l_code, &mut open_start, prev, epoch, reason);
                }
                previous = None;
                continue;
            }
            let carrier = carrier.expect("carrier");
            if open_start.is_none() {
                open_start = Some((carrier.t, carrier.epoch.clone(), "arc-start"));
                boundaries.push(boundary_json(&input, sat, &l_code, &carrier.epoch, "arc-start", "hard", &carrier.field_id));
            }
            if let Some(prev) = &previous {
                let dt = carrier.t - prev.t;
                let has_clock = clock_epochs.contains(&carrier.epoch);
                let was_missing = epoch_list
                    .iter()
                    .any(|(t, e)| *t > prev.t && *t < carrier.t && by_key.contains_key(&(sat.clone(), e.clone(), l_code)) == false && observed_other_sat(input, sat, e, &l_code));
                let repair = input
                    .accepted_repairs
                    .iter()
                    .find(|r| r.sat == *sat && r.epoch == carrier.epoch && r.band == *sig_band);
                if has_clock || was_missing || repair.is_some() {
                    let reason = if has_clock {
                        "receiver-clock-jump"
                    } else if repair.is_some() {
                        "accepted-cycle-slip"
                    } else {
                        "frequency-gap"
                    };
                    close_arc(&mut arcs, &input, sat, &l_code, &mut open_start, prev, &carrier.epoch, reason);
                    open_start = Some((carrier.t, carrier.epoch.clone(), reason));
                    boundaries.push(boundary_json(&input, sat, &l_code, &carrier.epoch, reason, "hard", &carrier.field_id));
                }
                if carrier.lli.unwrap_or(0) != 0 {
                    boundaries.push(boundary_json(&input, sat, &l_code, &carrier.epoch, "lli-flag", "proposed", &carrier.field_id));
                }
                analyze_transition(
                    &input,
                    &freq,
                    &by_key,
                    &mut findings,
                    &mut candidates,
                    sat,
                    &l_code,
                    &c_code,
                    prev,
                    &carrier,
                    dt,
                    has_clock,
                    was_missing,
                    &satellites,
                );
            } else if let Some((_, _, reason)) = &open_start {
                if reason != "arc-start" {
                    boundaries.push(boundary_json(&input, sat, &l_code, &carrier.epoch, reason, "hard", &carrier.field_id));
                }
            }
            previous = Some(carrier);
        }
        if let Some((_, start_epoch, _)) = &open_start {
            let last = epoch_list.iter().rev().find_map(|(_, e)| by_key.get(&(sat.clone(), e.clone(), &l_code))).unwrap();
            let id = arc_id(&input, sat, &l_code, start_epoch, &last.epoch);
            arcs.push(json!({
                "id": id,
                "sat": sat,
                "carrier_code": l_code,
                "start_epoch": start_epoch,
                "end_epoch": last.epoch,
                "open": true,
                "start_reason": "open",
                "algorithm_version": ALGORITHM_VERSION,
                "parameter_fingerprint": parameter_fingerprint(input.params),
                "input_version_id": input.input_version_id,
            }));
        }
    }

    json!({
        "algorithm_version": ALGORITHM_VERSION,
        "parameter_fingerprint": parameter_fingerprint(input.params),
        "arcs": arcs,
        "boundaries": boundaries,
        "findings": findings,
        "candidates": candidates,
    })
}

#[allow(clippy::too_many_arguments)]
fn analyze_transition(
    input: &EngineInput,
    frequencies: &BTreeMap<String, f64>,
    by_key: &BTreeMap<(String, String, String), ObsField>,
    findings: &mut Vec<Value>,
    candidates: &mut Vec<Value>,
    sat: &str,
    l1: &str,
    c1: &str,
    previous: &ObsField,
    current: &ObsField,
    dt: f64,
    has_clock: bool,
    had_gap: bool,
    satellites: &[String],
) {
    let phase_delta = match (previous.value, current.value) {
        (Some(before), Some(after)) => Some(after - before),
        _ => None,
    };
    let doppler_now = code_value(by_key, sat, &current.epoch, &format!("D{}", &l1[1..]));
    let expected_delta = doppler_now.map(|d| -d * dt);
    if let (Some(actual), Some(expected)) = (phase_delta, expected_delta) {
        let raw_residual = actual - expected;
        let common_count = satellites
            .iter()
            .filter(|other| {
                if **other == sat {
                    return false;
                }
                if let Some(other_obs) = by_key.get(&((*other).clone(), current.epoch.clone(), l1.to_string())) {
                    if let Some(other_prev) = epoch_before(by_key, other, current.t, l1) {
                        if let (Some(now), Some(before)) = (other_obs.value, other_prev.value) {
                            return (now - before - raw_residual).abs() < 0.35;
                        }
                    }
                }
                false
            })
            .count();
        let clock_residual = if common_count >= 2 { raw_residual - receiver_common_delta(by_key, satellites, current.epoch, current.t, l1) } else { raw_residual };
        if has_clock {
            findings.push(finding_json(input, sat, &current.epoch, l1, "receiver-clock-jump", "excluded", raw_residual, "cycle", "common receiver clock event separated from satellite slip"));
        } else {
            let detector_id = candidate_id(input, sat, &current.epoch, l1, "td", "single");
            emit_integer_candidates(
                candidates,
                input,
                &detector_id,
                sat,
                &current.epoch,
                l1,
                "time-difference",
                clock_residual,
                input.params.td_sigma_cycles,
                input.params.integer_search_radius,
                false,
                json!({"dt_seconds": dt, "raw_residual_cycles": raw_residual, "clock_separated_residual_cycles": clock_residual, "doppler_hz": doppler_now}),
                had_gap,
            );
        }
    }

    let bands = ["1", "2"];
    if l1 == "L1" {
        let l2 = "L2";
        let c2 = "C2";
        if let (Some(p1), Some(p2), Some(q1), Some(q2)) = (
            code_value(by_key, sat, &previous.epoch, c1),
            code_value(by_key, sat, &previous.epoch, c2),
            code_value(by_key, sat, &current.epoch, c1),
            code_value(by_key, sat, &current.epoch, c2),
        ) {
            if let (Some(lp1), Some(lp2), Some(lq1), Some(lq2)) = (
                phase_value(by_key, sat, &previous.epoch, l1),
                phase_value(by_key, sat, &previous.epoch, l2),
                phase_value(by_key, sat, &current.epoch, l1),
                phase_value(by_key, sat, &current.epoch, l2),
            ) {
                let f1 = *frequencies.get(l1).unwrap_or(&1575.42);
                let f2 = *frequencies.get(l2).unwrap_or(&1227.60);
                let wl = wavelength(f1);
                let w2 = wavelength(f2);
                let m_before = (p1 - p2) / (wl - w2) - (wl * lp1 - w2 * lp2) / (wl - w2);
                let m_after = (q1 - q2) / (wl - w2) - (wl * lq1 - w2 * lq2) / (wl - w2);
                let mw_residual = if has_clock { 0.0 } else { m_after - m_before };
                if has_clock {
                    findings.push(finding_json(input, sat, &current.epoch, "L1/L2", "receiver-clock-jump", "excluded", mw_residual, "cycle", "Melbourne-Wubbena clock epoch separated"));
                } else {
                    let detector_id = candidate_id(input, sat, &current.epoch, "L1/L2", "mw", "wide-lane");
                    emit_pair_candidates(
                        candidates,
                        input,
                        &detector_id,
                        sat,
                        &current.epoch,
                        "melbourne-wubbena",
                        mw_residual,
                        f1,
                        f2,
                        input.params.mw_sigma_cycles,
                        input.params.integer_search_radius,
                        "wide-lane",
                        json!({"residual_wide_lane_cycles": mw_residual, "previous_ambiguity_cycles": m_before, "current_ambiguity_cycles": m_after}),
                        had_gap,
                    );
                }

                let gf_before = wl * lp1 - w2 * lp2 - (p1 - p2);
                let gf_after = wl * lq1 - w2 * lq2 - (q1 - q2);
                let gf_residual = if has_clock { 0.0 } else { gf_after - gf_before };
                if !has_clock {
                    let detector_id = candidate_id(input, sat, &current.epoch, "L1/L2", "iono", "geometry-free");
                    emit_pair_candidates(
                        candidates,
                        input,
                        &detector_id,
                        sat,
                        &current.epoch,
                        "ionosphere-combination",
                        gf_residual,
                        f1,
                        f2,
                        input.params.iono_tolerance_m / wl,
                        input.params.integer_search_radius,
                        "geometry-free",
                        json!({"residual_m": gf_residual, "tolerance_m": input.params.iono_tolerance_m}),
                        had_gap,
                    );
                }
            }
        }
    }

    if current.lli.unwrap_or(0) != 0 {
        let snr = code_value(by_key, sat, &current.epoch, &format!("S{}", &l1[1..])).unwrap_or(50.0);
        let id = candidate_id(input, sat, &current.epoch, l1, "lli", "flag");
        let status = if snr < input.params.lli_low_snr_dbhz { "rejected" } else { "unknown" };
        let rejection = if status == "rejected" { Some("lli-false-positive: high SNR and carrier/Doppler continuity") } else { None };
        candidates.push(json!({
            "stable_identity": id,
            "sat": sat,
            "epoch": current.epoch,
            "band": band(l1),
            "detector": "LLI",
            "hypothesis": "flag-only",
            "proposed_cycles_l1": Value::Null,
            "proposed_cycles_l2": Value::Null,
            "residual": {"value": current.lli.unwrap_or(0), "unit": "lli-bit"},
            "score": 0.1,
            "status": status,
            "tie_group": null,
            "rejection_reason": rejection,
            "evidence": {"lli": current.lli, "snr_dbhz": snr, "gap": had_gap},
            "algorithm_version": ALGORITHM_VERSION,
            "parameter_fingerprint": parameter_fingerprint(input.params),
            "direct_source_field_id": current.field_id,
        }));
    }
}

fn emit_integer_candidates(
    out: &mut Vec<Value>,
    input: &EngineInput,
    group_id: &str,
    sat: &str,
    epoch: &str,
    carrier_code: &str,
    detector: &str,
    residual: f64,
    sigma: f64,
    radius: i64,
    _pair: bool,
    evidence: Value,
    had_gap: bool,
) {
    let mut local = Vec::new();
    for cycles in -radius..=radius {
        let error = residual - cycles as f64;
        if error.abs() <= sigma {
            let identity = stable_id(&[group_id, "cycles", &cycles.to_string()]);
            let score = (1.0 - error.abs() / sigma).max(0.0) - if had_gap { 0.10 } else { 0.0 };
            local.push((cycles as f64, error, score, identity));
        }
    }
    if local.is_empty() {
        return;
    }
    let ambiguous = local.len() > 1;
    for (cycles, error, score, identity) in local {
        out.push(json!({
            "stable_identity": identity,
            "group_identity": group_id,
            "sat": sat,
            "epoch": epoch,
            "band": band(carrier_code),
            "detector": detector,
            "hypothesis": format!("{cycles:+} cycle"),
            "proposed_cycles_l1": cycles,
            "proposed_cycles_l2": Value::Null,
            "residual": {"value": error, "unit": "cycle", "residual_before_offset_cycles": residual},
            "score": score.max(0.0),
            "status": if ambiguous { "ambiguous" } else { "proposed" },
            "tie_group": if ambiguous { Some(group_id) } else { Value::Null },
            "rejection_reason": Value::Null,
            "evidence": evidence,
            "algorithm_version": ALGORITHM_VERSION,
            "parameter_fingerprint": parameter_fingerprint(input.params),
        }));
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_pair_candidates(
    out: &mut Vec<Value>,
    input: &EngineInput,
    group_id: &str,
    sat: &str,
    epoch: &str,
    detector: &str,
    residual: f64,
    f1: f64,
    f2: f64,
    sigma_cycles: f64,
    radius: i64,
    unit_kind: &str,
    evidence: Value,
    had_gap: bool,
) {
    let wl = wavelength(f1);
    let w2 = wavelength(f2);
    let mut local = Vec::new();
    for n1 in -radius..=radius {
        for n2 in -radius..=radius {
            let predicted_cycles = if unit_kind == "wide-lane" {
                n1 as f64 - n2 as f64
            } else {
                (wl * n1 as f64 - w2 * n2 as f64) / wl
            };
            let error_cycles = residual / wl - predicted_cycles;
            if error_cycles.abs() <= sigma_cycles {
                let identity = stable_id(&[group_id, "pair", &n1.to_string(), &n2.to_string()]);
                let magnitude_penalty = (n1.unsigned_abs() + n2.unsigned_abs()) as f64 * 0.001;
                let score = (1.0 - error_cycles.abs() / sigma_cycles).max(0.0) - magnitude_penalty - if had_gap { 0.10 } else { 0.0 };
                local.push((n1 as f64, n2 as f64, error_cycles * wl, score.max(0.0), identity));
            }
        }
    }
    if local.is_empty() {
        return;
    }
    let ambiguous = local.len() > 1;
    for (n1, n2, error_m, score, identity) in local {
        out.push(json!({
            "stable_identity": identity,
            "group_identity": group_id,
            "sat": sat,
            "epoch": epoch,
            "band": "L1/L2",
            "detector": detector,
            "hypothesis": format!("L1 {n1:+}, L2 {n2:+} cycles"),
            "proposed_cycles_l1": n1,
            "proposed_cycles_l2": n2,
            "residual": {"value": error_m, "unit": if unit_kind == "wide-lane" { "wide-lane-cycle" } else { "m" }, "residual_before_offset": residual},
            "score": score,
            "status": if ambiguous { "ambiguous" } else { "proposed" },
            "tie_group": if ambiguous { Some(group_id) } else { Value::Null },
            "rejection_reason": Value::Null,
            "evidence": evidence,
            "algorithm_version": ALGORITHM_VERSION,
            "parameter_fingerprint": parameter_fingerprint(input.params),
        }));
    }
}

fn close_arcs(arcs: &mut Vec<Value>, input: &EngineInput, sat: &str, carrier: &str, open: &mut Option<(f64, String, &str)>, last: &ObsField, end_epoch: &str, end_reason: &str) {
    if let Some((_, start_epoch, start_reason)) = open.take() {
        let id = arc_id(input, sat, carrier, &start_epoch, end_epoch);
        arcs.push(json!({
            "id": id,
            "sat": sat,
            "carrier_code": carrier,
            "start_epoch": start_epoch,
            "end_epoch": if end_reason == "accepted-cycle-slip" || end_reason == "frequency-gap" || end_reason == "receiver-clock-jump" { last.epoch } else { end_epoch },
            "open": false,
            "start_reason": start_reason,
            "end_reason": end_reason,
            "algorithm_version": ALGORITHM_VERSION,
            "parameter_fingerprint": parameter_fingerprint(input.params),
            "input_version_id": input.input_version_id,
        }));
    }
}

fn boundary_json(input: &EngineInput, sat: &str, carrier: &str, epoch: &str, reason: &str, strength: &str, field_id: &str) -> Value {
    json!({
        "id": boundary_id(input, sat, carrier, epoch, reason),
        "sat": sat,
        "carrier_code": carrier,
        "epoch": epoch,
        "reason": reason,
        "strength": strength,
        "direct_source_field_id": field_id,
        "algorithm_version": ALGORITHM_VERSION,
        "parameter_fingerprint": parameter_fingerprint(input.params),
    })
}

fn finding_json(input: &EngineInput, sat: &str, epoch: &str, band: &str, kind: &str, state: &str, value: f64, unit: &str, message: &str) -> Value {
    json!({
        "id": finding_id(input, sat, epoch, band, kind),
        "sat": sat,
        "epoch": epoch,
        "band": band,
        "kind": kind,
        "state": state,
        "value": {"value": value, "unit": unit},
        "message": message,
        "algorithm_version": ALGORITHM_VERSION,
        "parameter_fingerprint": parameter_fingerprint(input.params),
    })
}

fn code_value(map: &BTreeMap<(String, String, String), ObsField>, sat: &str, epoch: &str, code: &str) -> Option<f64> {
    map.get(&(sat.to_string(), epoch.to_string(), code.to_string())).and_then(|x| x.value)
}

fn phase_value(map: &BTreeMap<(String, String, String), ObsField>, sat: &str, epoch: &str, code: &str) -> Option<f64> {
    code_value(map, sat, epoch, code)
}

fn epoch_before(map: &BTreeMap<(String, String, String), ObsField>, sat: &str, t: f64, code: &str) -> Option<ObsField> {
    map.values()
        .filter(|x| x.sat == sat && x.code == code && x.t < t)
        .max_by(|a, b| a.t.total_cmp(&b.t))
        .cloned()
}

fn observed_other_sat(input: &EngineInput, sat: &str, epoch: &str, code: &str) -> bool {
    input.observations.iter().any(|o| o.sat != sat && o.epoch == epoch && o.code == code && o.value.is_some())
}

fn receiver_common_delta(map: &BTreeMap<(String, String, String), ObsField>, satellites: &[String], epoch: &str, t: f64, code: &str) -> f64 {
    let mut values = Vec::new();
    for sat in satellites {
        if let (Some(current), Some(previous)) = (code_value(map, sat, epoch, code), epoch_before(map, sat, t, code).and_then(|x| x.value)) {
            values.push(current - previous);
        }
    }
    values.sort_by(|a, b| a.total_cmp(b));
    values[values.len() / 2]
}

fn wavelength(mhz: f64) -> f64 {
    299_792_458.0 / (mhz * 1_000_000.0)
}

fn arc_id(input: &EngineInput, sat: &str, carrier: &str, start: &str, end: &str) -> String {
    stable_id(&[input.project_id, "arc", sat, carrier, start, end])
}

fn boundary_id(input: &EngineInput, sat: &str, carrier: &str, epoch: &str, reason: &str) -> String {
    stable_id(&[input.project_id, "boundary", sat, carrier, epoch, reason])
}

fn finding_id(input: &EngineInput, sat: &str, epoch: &str, band: &str, kind: &str) -> String {
    stable_id(&[input.project_id, "finding", sat, epoch, band, kind])
}

fn candidate_id(input: &EngineInput, sat: &str, epoch: &str, band: &str, detector: &str, hypothesis: &str) -> String {
    stable_id(&[input.project_id, "candidate-group", sat, epoch, band, detector, hypothesis])
}
