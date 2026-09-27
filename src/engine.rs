use std::collections::{BTreeMap, BTreeSet};

use crate::identity::stable_hash;
use crate::model::{
    ArcSegment, CandidateGroup, CandidateOption, ClockEvent, DetectorParameters, EpochDef,
    FrequencyDef, NormalizedValue, RawObservation, RepairOffset, Residual, SatelliteDef, Station,
};

#[derive(Debug, Clone)]
pub struct ComputeInput<'a> {
    pub project_id: &'a str,
    pub event_id: &'a str,
    pub station: &'a Station,
    pub epochs: &'a [EpochDef],
    pub satellites: &'a [SatelliteDef],
    pub frequencies: &'a [FrequencyDef],
    pub raw_observations: &'a [RawObservation],
    pub normalized: &'a [NormalizedValue],
    pub clock_events: &'a [ClockEvent],
    pub parameters: &'a DetectorParameters,
}

#[derive(Debug, Clone, Default)]
pub struct ComputedState {
    pub residuals: Vec<Residual>,
    pub candidate_groups: Vec<CandidateGroup>,
    pub repair_offsets: Vec<RepairOffset>,
    pub arcs: Vec<ArcSegment>,
}

#[derive(Debug, Clone)]
pub struct NormalizedPoint {
    pub epoch_id: String,
    pub satellite: String,
    pub code: String,
    pub frequency: i64,
    pub value: Option<f64>,
    pub raw_id: String,
    pub raw_event_id: String,
    pub diagnostics: Vec<String>,
    pub lli: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
struct Freq {
    number: i64,
    wavelength: f64,
}

fn key(epoch: &str, satellite: &str, code: &str, frequency: i64) -> String {
    format!("{epoch}|{satellite}|{code}|f{frequency}")
}

fn point(input: &ComputeInput, epoch: &str, satellite: &str, code: &str, frequency: i64) -> Option<NormalizedPoint> {
    let wanted = format!("raw:{epoch}:{satellite}:{code}:f{frequency}");
    input.raw_observations.iter().find_map(|raw| {
        if raw.stable_id != wanted {
            return None;
        }
        let norm = input.normalized.iter().find(|item| item.raw_id == raw.stable_id)?;
        Some(NormalizedPoint {
            epoch_id: epoch.to_string(),
            satellite: satellite.to_string(),
            code: code.to_string(),
            frequency,
            value: norm.value,
            raw_id: raw.stable_id.clone(),
            raw_event_id: raw.event_id.clone(),
            diagnostics: norm.diagnostics.clone(),
            lli: raw.lli,
        })
    })
}

fn frequencies(input: &ComputeInput) -> Vec<Freq> {
    input
        .frequencies
        .iter()
        .map(|item| Freq {
            number: item.frequency_number,
            wavelength: item.wavelength_m,
        })
        .collect()
}

fn epoch_time(input: &ComputeInput, epoch_id: &str) -> f64 {
    input
        .epochs
        .iter()
        .find(|item| item.id == epoch_id)
        .map(|item| item.virtual_seconds)
        .unwrap_or(0.0)
}

fn clock_event(input: &ComputeInput, epoch_id: &str) -> Option<&ClockEvent> {
    input.clock_events.iter().find(|item| item.epoch_id == epoch_id && item.scope == "receiver")
}

fn clock_seconds_at(input: &ComputeInput, epoch_id: &str) -> f64 {
    let mut total = 0.0;
    for item in input.clock_events {
        if item.scope == "receiver" && epoch_time(input, &item.epoch_id) <= epoch_time(input, epoch_id) {
            total += item.delta_seconds;
        }
    }
    total
}

fn make_residual(
    input: &ComputeInput,
    detector: &str,
    from_epoch: &str,
    epoch: &str,
    satellite: &str,
    frequency: Option<i64>,
    value: Option<f64>,
    unit: &str,
    status: &str,
    diagnostics: Vec<String>,
    sources: Vec<String>,
    parameter_fingerprint: &str,
) -> Residual {
    let value_part = value.map(|item| format!("{item:.9}")).unwrap_or_else(|| "none".into());
    let diag = diagnostics.join("|");
    let id_hash = stable_hash(&[
        input.project_id.into(),
        detector.into(),
        from_epoch.into(),
        epoch.into(),
        satellite.into(),
        frequency.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
        value_part,
        status.into(),
        diag.clone(),
        sources.join(","),
        input.event_id.into(),
        parameter_fingerprint.into(),
    ]);
    Residual {
        stable_id: format!("res:{detector}:{from_epoch}-{epoch}:{satellite}:{}:{id_hash}", frequency.map(|v| format!("f{v}")).unwrap_or_else(|| "sta".into())),
        detector: detector.into(),
        from_epoch_id: from_epoch.into(),
        epoch_id: epoch.into(),
        satellite: satellite.into(),
        frequency_number: frequency,
        value,
        unit: unit.into(),
        status: status.into(),
        diagnostics,
        direct_sources: sources,
        algorithm_version: input.parameters.algorithm_version.clone(),
        parameter_fingerprint: parameter_fingerprint.into(),
    }
}

fn nearest_integer(value: f64) -> f64 {
    value.round()
}

fn parameter_fingerprint(parameters: &DetectorParameters) -> String {
    format!("pf:{}", crate::identity::fingerprint_json(&serde_json::json!(parameters)))
}

fn code_value(input: &ComputeInput, epoch: &str, satellite: &str, frequency: i64) -> Option<f64> {
    let raw = point(input, epoch, satellite, &format!("C{frequency}W"), frequency)?;
    if raw.value.is_none() {
        return None;
    }
    let mut value = raw.value?;
    if let Some(event) = clock_event(input, epoch) {
        value -= event.delta_m;
    }
    Some(value)
}

fn phase_value(input: &ComputeInput, epoch: &str, satellite: &str, frequency: i64) -> Option<f64> {
    let raw = point(input, epoch, satellite, &format!("L{frequency}W"), frequency)?;
    if raw.value.is_none() {
        return None;
    }
    let mut value = raw.value?;
    let wavelength = frequencies(input)
        .into_iter()
        .find(|item| item.number == frequency)
        .map(|item| item.wavelength)
        .unwrap_or(1.0);
    if let Some(event) = clock_event(input, epoch) {
        value += event.delta_m / wavelength;
    }
    Some(value)
}

fn doppler_value(input: &ComputeInput, epoch: &str, satellite: &str, frequency: i64) -> Option<f64> {
    point(input, epoch, satellite, &format!("D{frequency}W"), frequency)?.value
}

pub fn compute(input: &ComputeInput) -> ComputedState {
    let mut state = ComputedState::default();
    let fingerprint = parameter_fingerprint(input.parameters);
    let freqs = frequencies(input);
    let satellites: Vec<String> = input.satellites.iter().map(|item| item.id.clone()).collect();

    for satellite in &satellites {
        for freq in &freqs {
            compute_time_differences(&input, &mut state, satellite, freq.number, &fingerprint);
            build_arcs(&input, &mut state, satellite, freq.number);
        }
        if freqs.iter().any(|item| item.number == 1) && freqs.iter().any(|item| item.number == 2) {
            compute_iono_pairs(&input, &mut state, satellite, &fingerprint);
            compute_mw_pairs(&input, &mut state, satellite, &fingerprint);
        }
        add_lli_groups(&input, &mut state, satellite);
    }
    compute_receiver_clock(&input, &mut state, &fingerprint);
    merge_groups(&mut state);
    state
}

fn compute_time_differences(
    input: &ComputeInput,
    state: &mut ComputedState,
    satellite: &str,
    frequency: i64,
    fingerprint: &str,
) {
    for pair in input.epochs.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let dt = current.virtual_seconds - previous.virtual_seconds;
        let old_phase = phase_value(input, &previous.id, satellite, frequency);
        let new_phase = phase_value(input, &current.id, satellite, frequency);
        let doppler = doppler_value(input, &previous.id, satellite, frequency);
        let sources = collect_sources(input, &previous.id, &current.id, satellite, frequency, &["L", "D"]);
        match (old_phase, new_phase, doppler) {
            (Some(old), Some(new), Some(dop)) => {
                let residual_cycles = (new - old) - dop * dt;
                let detector = "time-difference";
                let threshold = input.parameters.time_difference_cycles_threshold;
                let status = if residual_cycles.abs() > threshold { "candidate" } else { "ok" };
                let residual = make_residual(
                    input,
                    detector,
                    &previous.id,
                    &current.id,
                    satellite,
                    Some(frequency),
                    Some(residual_cycles),
                    "cycles",
                    status,
                    vec![],
                    sources,
                    fingerprint,
                );
                state.residuals.push(residual.clone());
                if status == "candidate" {
                    let nearest = nearest_integer(residual_cycles);
                    add_candidate(state, &residual, vec![(nearest, 0.9)], "time-difference", fingerprint);
                }
            }
            _ => {
                let diagnostics = vec!["missing carrier or Doppler; boundary retained as unknown".to_string()];
                state.residuals.push(make_residual(
                    input,
                    "time-difference",
                    &previous.id,
                    &current.id,
                    satellite,
                    Some(frequency),
                    None,
                    "cycles",
                    "unknown",
                    diagnostics,
                    sources,
                    fingerprint,
                ));
            }
        }
    }
}

fn collect_sources(
    input: &ComputeInput,
    previous: &str,
    current: &str,
    satellite: &str,
    frequency: i64,
    prefixes: &[&str],
) -> Vec<String> {
    let mut result = BTreeSet::new();
    for epoch in [previous, current] {
        for prefix in prefixes {
            let code = format!("{prefix}{frequency}W");
            if let Some(item) = point(input, epoch, satellite, &code, frequency) {
                result.insert(item.raw_id);
            }
        }
    }
    result.into_iter().collect()
}

fn compute_iono_pairs(
    input: &ComputeInput,
    state: &mut ComputedState,
    satellite: &str,
    fingerprint: &str,
) {
    for pair in input.epochs.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let old_l1 = phase_value(input, &previous.id, satellite, 1);
        let old_l2 = phase_value(input, &previous.id, satellite, 2);
        let new_l1 = phase_value(input, &current.id, satellite, 1);
        let new_l2 = phase_value(input, &current.id, satellite, 2);
        let mut sources = collect_sources(input, &previous.id, &current.id, satellite, 1, &["L"]);
        sources.extend(collect_sources(input, &previous.id, &current.id, satellite, 2, &["L"]));
        sources.sort();
        match (old_l1, old_l2, new_l1, new_l2) {
            (Some(old1), Some(old2), Some(new1), Some(new2)) => {
                let residual_cycles = (new1 - old1) - (new2 - old2);
                let threshold = input.parameters.iono_cycles_threshold;
                let status = if residual_cycles.abs() > threshold { "candidate" } else { "ok" };
                let residual = make_residual(
                    input,
                    "ionosphere",
                    &previous.id,
                    &current.id,
                    satellite,
                    Some(1),
                    Some(residual_cycles),
                    "L1 cycles",
                    status,
                    vec!["geometry-free carrier difference".into()],
                    sources,
                    fingerprint,
                );
                state.residuals.push(residual.clone());
                if status == "candidate" {
                    let options = if (residual_cycles.abs() - input.parameters.tie_half_cycle).abs() < 0.05 {
                        vec![(0.0, 0.5), (1.0, 0.5)]
                    } else {
                        vec![(nearest_integer(residual_cycles), 0.91)]
                    };
                    add_candidate(state, &residual, options, "ionosphere", fingerprint);
                }
            }
            _ => {
                state.residuals.push(make_residual(
                    input,
                    "ionosphere",
                    &previous.id,
                    &current.id,
                    satellite,
                    Some(1),
                    None,
                    "L1 cycles",
                    "unknown",
                    vec!["frequency missing; ionosphere combination unavailable".into()],
                    sources,
                    fingerprint,
                ));
            }
        }
    }
}

fn compute_mw_pairs(
    input: &ComputeInput,
    state: &mut ComputedState,
    satellite: &str,
    fingerprint: &str,
) {
    let l1 = frequencies(input).into_iter().find(|item| item.number == 1).unwrap();
    let l2 = frequencies(input).into_iter().find(|item| item.number == 2).unwrap();
    for pair in input.epochs.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let values = (
            code_value(input, &previous.id, satellite, 1),
            phase_value(input, &previous.id, satellite, 1),
            code_value(input, &previous.id, satellite, 2),
            phase_value(input, &previous.id, satellite, 2),
            code_value(input, &current.id, satellite, 1),
            phase_value(input, &current.id, satellite, 1),
            code_value(input, &current.id, satellite, 2),
            phase_value(input, &current.id, satellite, 2),
        );
        let mut sources = collect_sources(input, &previous.id, &current.id, satellite, 1, &["C", "L"]);
        sources.extend(collect_sources(input, &previous.id, &current.id, satellite, 2, &["C", "L"]));
        sources.sort();
        if let (Some(c1a), Some(l1a), Some(c2a), Some(l2a), Some(c1b), Some(l1b), Some(c2b), Some(l2b)) = values {
            let wl = l1.wavelength - l2.wavelength;
            let old = (l1a - l2a) - (l1.wavelength * c1a - l2.wavelength * c2a) / (wl * wl);
            let new = (l1b - l2b) - (l1.wavelength * c1b - l2.wavelength * c2b) / (wl * wl);
            let residual_cycles = new - old;
            let threshold = input.parameters.mw_cycles_threshold;
            let status = if residual_cycles.abs() > threshold { "candidate" } else { "ok" };
            let residual = make_residual(
                input,
                "melbourne-wubbena",
                &previous.id,
                &current.id,
                satellite,
                Some(1),
                Some(residual_cycles),
                "wide-lane cycles",
                status,
                vec!["code/carrier wide-lane minus narrow-lane pseudorange".into()],
                sources,
                fingerprint,
            );
            state.residuals.push(residual.clone());
            if status == "candidate" {
                let options = if (residual_cycles.abs() - input.parameters.tie_half_cycle).abs() < 0.05 {
                    vec![(0.0, 0.5), (1.0, 0.5)]
                } else {
                    vec![(nearest_integer(residual_cycles), 0.92)]
                };
                add_candidate(state, &residual, options, "melbourne-wubbena", fingerprint);
            }
        } else {
            state.residuals.push(make_residual(
                input,
                "melbourne-wubbena",
                &previous.id,
                &current.id,
                satellite,
                Some(1),
                None,
                "wide-lane cycles",
                "unknown",
                vec!["frequency/code missing; Melbourne-Wubbena combination unavailable".into()],
                sources,
                fingerprint,
            ));
        }
    }
}

fn add_lli_groups(input: &ComputeInput, state: &mut ComputedState, satellite: &str) {
    for epoch_index in 1..input.epochs.len() {
        let current = &input.epochs[epoch_index];
        let previous = &input.epochs[epoch_index - 1];
        for frequency in [1, 2] {
            let Some(raw) = point(input, &current.id, satellite, &format!("L{frequency}W"), frequency) else {
                continue;
            };
            if raw.lli != Some(1) {
                continue;
            }
            let continuous = state
                .residuals
                .iter()
                .filter(|item| {
                    item.detector == "time-difference"
                        && item.from_epoch_id == previous.id
                        && item.epoch_id == current.id
                        && item.satellite == satellite
                        && item.frequency_number == Some(frequency)
                        && item.status == "ok"
                })
                .count()
                > 0;
            let status = if continuous { "false_positive" } else { "candidate" };
            let reject_reason = if continuous { Some("LLI flag but carrier time-difference residual is within tolerance".to_string()) } else { None };
            let residual = make_residual(
                input,
                "lli",
                &previous.id,
                &current.id,
                satellite,
                Some(frequency),
                Some(if continuous { 0.0 } else { 1.0 }),
                "flag",
                status,
                vec![reject_reason.clone().unwrap_or_else(|| "LLI loss-of-lock asserted".into())],
                vec![raw.raw_id.clone()],
                &parameter_fingerprint(input.parameters),
            );
            state.residuals.push(residual.clone());
            if continuous {
                add_rejected_lli(state, &residual, reject_reason.unwrap_or_default(), input);
            } else {
                add_candidate(
                    state,
                    &residual,
                    vec![(1.0, input.parameters.lli_score_weight)],
                    "lli",
                    &parameter_fingerprint(input.parameters),
                );
            }
        }
    }
}

fn compute_receiver_clock(input: &ComputeInput, state: &mut ComputedState, fingerprint: &str) {
    for event in input.clock_events {
        if event.scope != "receiver" {
            continue;
        }
        let epoch_index = input.epochs.iter().position(|item| item.id == event.epoch_id);
        let Some(index) = epoch_index else { continue };
        if index == 0 {
            continue;
        }
        let previous = &input.epochs[index - 1];
        let residual = make_residual(
            input,
            "receiver-clock",
            &previous.id,
            &event.epoch_id,
            "STATION",
            None,
            Some(event.delta_seconds),
            "s",
            "clock_event",
            vec!["whole-receiver clock jump; separated from per-satellite carrier repair".into()],
            vec![format!("clock:{}", event.id)],
            fingerprint,
        );
        state.residuals.push(residual.clone());
        add_clock_group(state, &residual, event.delta_seconds, fingerprint);
    }
}

fn add_candidate(
    state: &mut ComputedState,
    residual: &Residual,
    options: Vec<(f64, f64)>,
    detector: &str,
    fingerprint: &str,
) {
    let group_id = format!(
        "cand:{}:{}:{}:{}",
        residual.from_epoch_id,
        residual.epoch_id,
        residual.satellite,
        residual.frequency_number.map(|v| format!("f{v}")).unwrap_or_else(|| "sta".into())
    );
    let tied: Vec<f64> = options.iter().map(|(offset, _)| *offset).collect();
    let group_status = if tied.len() > 1 && tied.windows(2).all(|pair| (pair[0] - pair[1]).abs() == 1.0) {
        "ambiguous"
    } else {
        "pending"
    };
    state.candidate_groups.push(CandidateGroup {
        stable_id: group_id.clone(),
        from_epoch_id: residual.from_epoch_id.clone(),
        epoch_id: residual.epoch_id.clone(),
        satellite: residual.satellite.clone(),
        frequency_number: residual.frequency_number,
        kind: "satellite_slip".into(),
        status: group_status,
        reject_reason: None,
        options: options
            .into_iter()
            .map(|(offset, score)| {
                let option_id = stable_hash(&[
                    group_id.clone(),
                    detector.into(),
                    format!("{offset:.9}"),
                    residual.stable_id.clone(),
                    fingerprint.into(),
                ]);
                CandidateOption {
                    stable_id: format!("candopt:{option_id}"),
                    group_id: group_id.clone(),
                    detector: detector.into(),
                    observed_slip_cycles: residual.value,
                    offset_cycles: offset,
                    score,
                    status: "legal".into(),
                    reject_reason: None,
                    direct_residual_ids: vec![residual.stable_id.clone()],
                    parameter_fingerprint: fingerprint.into(),
                }
            })
            .collect(),
        decision: None,
    });
}

fn add_rejected_lli(state: &mut ComputedState, residual: &Residual, reason: String, input: &ComputeInput) {
    let group_id = format!(
        "cand:{}:{}:{}:f{}",
        residual.from_epoch_id, residual.epoch_id, residual.satellite, residual.frequency_number.unwrap_or_default()
    );
    let id = stable_hash(&[group_id.clone(), "lli".into(), "rejected".into(), residual.stable_id.clone()]);
    state.candidate_groups.push(CandidateGroup {
        stable_id: group_id,
        from_epoch_id: residual.from_epoch_id.clone(),
        epoch_id: residual.epoch_id.clone(),
        satellite: residual.satellite.clone(),
        frequency_number: residual.frequency_number,
        kind: "lli_false_positive".into(),
        status: "rejected".into(),
        reject_reason: Some(reason.clone()),
        options: vec![CandidateOption {
            stable_id: format!("candopt:{id}"),
            group_id: format!(
                "cand:{}:{}:{}:f{}",
                residual.from_epoch_id, residual.epoch_id, residual.satellite, residual.frequency_number.unwrap_or_default()
            ),
            detector: "lli".into(),
            observed_slip_cycles: residual.value,
            offset_cycles: 0.0,
            score: input.parameters.lli_score_weight,
            status: "rejected".into(),
            reject_reason: Some(reason),
            direct_residual_ids: vec![residual.stable_id.clone()],
            parameter_fingerprint: residual.parameter_fingerprint.clone(),
        }],
        decision: None,
    });
}

fn add_clock_group(state: &mut ComputedState, residual: &Residual, seconds: f64, fingerprint: &str) {
    let group_id = format!("cand:{}:{}:STATION:sta", residual.from_epoch_id, residual.epoch_id);
    let option_id = stable_hash(&[group_id.clone(), seconds.to_string(), residual.stable_id.clone(), fingerprint.into()]);
    state.candidate_groups.push(CandidateGroup {
        stable_id: group_id,
        from_epoch_id: residual.from_epoch_id.clone(),
        epoch_id: residual.epoch_id.clone(),
        satellite: "STATION".into(),
        frequency_number: None,
        kind: "receiver_clock".into(),
        status: "clock_separated".into(),
        reject_reason: Some("receiver clock jump must not be stored as a satellite repair offset".into()),
        options: vec![CandidateOption {
            stable_id: format!("candopt:{option_id}"),
            group_id: format!("cand:{}:{}:STATION:sta", residual.from_epoch_id, residual.epoch_id),
            detector: "receiver-clock".into(),
            observed_slip_cycles: None,
            offset_cycles: 0.0,
            score: 1.0,
            status: "clock_event".into(),
            reject_reason: Some("whole-receiver event".into()),
            direct_residual_ids: vec![residual.stable_id.clone()],
            parameter_fingerprint: fingerprint.into(),
        }],
        decision: None,
    });
}

fn merge_groups(state: &mut ComputedState) {
    let mut merged: BTreeMap<String, CandidateGroup> = BTreeMap::new();
    for group in std::mem::take(&mut state.candidate_groups) {
        let entry = merged.entry(group.stable_id.clone()).or_insert_with(|| CandidateGroup {
            stable_id: group.stable_id.clone(),
            from_epoch_id: group.from_epoch_id.clone(),
            epoch_id: group.epoch_id.clone(),
            satellite: group.satellite.clone(),
            frequency_number: group.frequency_number,
            kind: if group.kind == "receiver_clock" || group.kind == "lli_false_positive" {
                group.kind.clone()
            } else {
                "satellite_slip".into()
            },
            status: group.status.clone(),
            reject_reason: group.reject_reason.clone(),
            options: Vec::new(),
            decision: group.decision.clone(),
        });
        for option in group.options {
            if !entry.options.iter().any(|existing| existing.stable_id == option.stable_id) {
                entry.options.push(option);
            }
        }
        let detectors: BTreeSet<String> = entry.options.iter().map(|option| option.detector.clone()).collect();
        if entry.kind == "satellite_slip" && detectors.len() > 1 {
            entry.status = "detector_disagreement".into();
        }
        if group.kind == "lli_false_positive" {
            entry.kind = "lli_false_positive".into();
            entry.status = "rejected".into();
        }
        if group.kind == "receiver_clock" {
            entry.kind = "receiver_clock".into();
            entry.status = "clock_separated".into();
        }
    }
    state.candidate_groups = merged.into_values().collect();
    state.candidate_groups.sort_by(|a, b| {
        a.epoch_id
            .cmp(&b.epoch_id)
            .then(a.satellite.cmp(&b.satellite))
            .then(a.frequency_number.cmp(&b.frequency_number))
            .then(a.stable_id.cmp(&b.stable_id))
    });
}

fn build_arcs(input: &ComputeInput, state: &mut ComputedState, satellite: &str, frequency: i64) {
    let mut start = 0;
    let mut next_reason: Option<&str> = None;
    for index in 1..input.epochs.len() {
        let previous = &input.epochs[index - 1];
        let current = &input.epochs[index];
        let missing = phase_value(input, &previous.id, satellite, frequency).is_none()
            || phase_value(input, &current.id, satellite, frequency).is_none();
        let flagged = state.candidate_groups.iter().any(|group| {
            group.satellite == satellite
                && group.frequency_number == Some(frequency)
                && group.from_epoch_id == previous.id
                && group.epoch_id == current.id
                && group.kind == "satellite_slip"
        });
        let boundary = if missing {
            Some(("frequency_missing", "unknown"))
        } else if flagged {
            Some(("cycle_slip_candidate", "provisional_split"))
        } else {
            None
        };
        if let Some((reason, _)) = boundary {
            push_arc(
                state,
                input,
                satellite,
                frequency,
                &input.epochs[start].id,
                &input.epochs[index - 1].id,
                if next_reason.is_some() { "provisional_split" } else { "active" },
                next_reason,
                None,
            );
            start = index;
            next_reason = Some(reason);
        }
    }
    if start < input.epochs.len() {
        push_arc(
            state,
            input,
            satellite,
            frequency,
            &input.epochs[start].id,
            &input.epochs[input.epochs.len() - 1].id,
            if next_reason.is_some() { "provisional_split" } else { "active" },
            next_reason,
            next_reason.map(|_| {
                format!(
                    "cand:{}:{}:{}:f{}",
                    input.epochs[start - 1].id,
                    input.epochs[start].id,
                    satellite,
                    frequency
                )
            }),
        );
    }
}

fn push_arc(
    state: &mut ComputedState,
    input: &ComputeInput,
    satellite: &str,
    frequency: i64,
    start_epoch: &str,
    end_epoch: &str,
    status: &str,
    reason: Option<&str>,
    candidate: Option<String>,
) {
    let id_hash = stable_hash(&[
        input.project_id.into(),
        satellite.into(),
        frequency.to_string(),
        start_epoch.into(),
        reason.unwrap_or("continuous").into(),
        input.event_id.into(),
    ]);
    state.arcs.push(ArcSegment {
        stable_id: format!("arc:{satellite}:f{frequency}:{start_epoch}:{id_hash}"),
        satellite: satellite.into(),
        frequency_number: frequency,
        start_epoch_id: start_epoch.into(),
        end_epoch_id: end_epoch.into(),
        state: status.into(),
        boundary_reason: reason.map(str::to_string),
        candidate_group_id: candidate,
        generated_by_event_id: input.event_id.into(),
    });
}
