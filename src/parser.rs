//! Parser for the project's small, copy-pasteable plain-text observation format.
//!
//! The format is intentionally tiny and human-writable so that counter-example
//! cases are readable domain material rather than generic spreadsheets:
//!
//! ```text
//! META project demo
//! SAT_STATE <sat> <x_m> <y_m> <z_m> <clock_bias_s>
//! EPOCH <epoch_id> <gpst_seconds> <time_raw...>
//! CLK <epoch_id> <jump_seconds_or_token> <marker...>
//! SIG <epoch_id> <sat> <code> <raw_value> [LLI=<bits>] [SSI=<raw>]
//! ```
//!
//! Numeric values carry units by code family: C/P -> m, L -> cycle, D -> Hz,
//! S -> dB-Hz. A value that cannot be parsed is retained in place with a
//! diagnostic instead of being dropped.

use crate::raw_model::{
    ClockEvent, EpochRecord, ImportDiagnostic, ImportReport, ObsCode, SatelliteState, SignalObs,
};
use crate::units::{datum, unit, Qty};

fn kind_unit(code: &str) -> Option<(&'static str, &'static str)> {
    match code.chars().next()? {
        'C' | 'P' => Some((unit::METER, datum::RAW)),
        'L' => Some((unit::CYCLE, datum::RAW)),
        'D' => Some((unit::HZ, datum::RAW)),
        'S' => Some((unit::DBHZ, datum::RAW)),
        _ => None,
    }
}

fn parse_qty(raw: &str, code: &str) -> (Qty, Option<ImportDiagnostic>) {
    let (u, d) = match kind_unit(code) {
        Some(v) => v,
        None => {
            let q = Qty::unconvertible(
                raw,
                "?",
                format!("unknown observation code `{code}`: expected leading C/P/L/D/S"),
            );
            return (q, None);
        }
    };
    match raw.trim().parse::<f64>() {
        Ok(v) => (Qty::new(v, u, d, raw), None),
        Err(_) => (
            Qty::unconvertible(raw, u, format!("cannot parse `{raw}` as {u} for code {code}")),
            Some(ImportDiagnostic {
                line_no: 0,
                severity: crate::raw_model::DiagSeverity::Warning,
                code: "UNCONVERTIBLE_VALUE".to_string(),
                message: format!("value `{raw}` kept raw; not plotted as a number"),
                raw_line: raw.to_string(),
            }),
        ),
    }
}

/// Parse a complete source document into epoch records plus a report.
pub fn parse_document(text: &str) -> (Vec<EpochRecord>, ImportReport) {
    let mut report = ImportReport::default();
    // epoch_id -> index
    let mut index: Vec<(String, usize)> = Vec::new();
    let mut epochs: Vec<EpochRecord> = Vec::new();

    let mut sat_state_lines: Vec<(usize, Vec<String>)> = Vec::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let push_err = |report: &mut ImportReport, code: &str, message: String| {
            report.diagnostics.push(ImportDiagnostic {
                line_no,
                severity: crate::raw_model::DiagSeverity::Error,
                code: code.to_string(),
                message,
                raw_line: raw_line.to_string(),
            });
        };

        match parts.first().copied() {
            Some("META") => { /* free-form, retained via project metadata elsewhere */ }
            Some("EPOCH") => {
                if parts.len() < 4 {
                    push_err(
                        &mut report,
                        "BAD_EPOCH",
                        "usage: EPOCH <id> <gpst_seconds> <time_raw...>".to_string(),
                    );
                    continue;
                }
                let id = parts[1].to_string();
                let t: f64 = match parts[2].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        push_err(
                            &mut report,
                            "BAD_EPOCH_TIME",
                            format!("epoch `{id}` gpst_seconds `{}` not a number", parts[2]),
                        );
                        continue;
                    }
                };
                let time_raw = parts[3..].join(" ");
                if index.iter().any(|(e, _)| e == &id) {
                    push_err(&mut report, "DUP_EPOCH", format!("duplicate epoch id `{id}`"));
                    continue;
                }
                index.push((id.clone(), epochs.len()));
                epochs.push(EpochRecord {
                    epoch_id: id,
                    time_raw,
                    t_seconds: t,
                    station: String::new(),
                    signals: Vec::new(),
                    clock_events: Vec::new(),
                    sat_states: Vec::new(),
                });
            }
            Some("STATION") => {
                if parts.len() < 3 {
                    push_err(&mut report, "BAD_STATION", "usage: STATION <epoch_id> <station>".into());
                    continue;
                }
                let (id, name) = (parts[1], parts[2]);
                match find(&index, id) {
                    Some(i) => epochs[i].station = name.to_string(),
                    None => push_err(&mut report, "ORPHAN_LINE", format!("STATION references unknown epoch `{id}`")),
                }
            }
            Some("CLK") => {
                if parts.len() < 3 {
                    push_err(&mut report, "BAD_CLK", "usage: CLK <epoch_id> <jump_seconds|RAW> <marker...>".into());
                    continue;
                }
                let (id, tok) = (parts[1], parts[2]);
                let marker = parts[2..].join(" ");
                let jump = match tok.parse::<f64>() {
                    Ok(v) => Qty::new(v, unit::SECOND, datum::GPST, tok),
                    Err(_) => Qty::unconvertible(tok, unit::SECOND, "clock jump token not numeric; retained raw"),
                };
                match find(&index, id) {
                    Some(i) => epochs[i].clock_events.push(ClockEvent {
                        at_epoch: id.to_string(),
                        marker,
                        jump,
                    }),
                    None => push_err(&mut report, "ORPHAN_LINE", format!("CLK references unknown epoch `{id}`")),
                }
            }
            Some("SAT_STATE") => {
                if parts.len() < 6 {
                    push_err(
                        &mut report,
                        "BAD_SAT_STATE",
                        "usage: SAT_STATE <sat> <x_m> <y_m> <z_m> <clock_bias_s>".into(),
                    );
                    continue;
                }
                sat_state_lines.push((line_no, parts.iter().map(|s| s.to_string()).collect()));
            }
            Some("SIG") => {
                if parts.len() < 5 {
                    push_err(
                        &mut report,
                        "BAD_SIG",
                        "usage: SIG <epoch_id> <sat> <code> <raw_value> [LLI=<n>] [SSI=<raw>]".into(),
                    );
                    continue;
                }
                let (id, sat, code, rawv) = (parts[1], parts[2], parts[3], parts[4]);
                let slot = match find(&index, id) {
                    Some(i) => i,
                    None => {
                        push_err(&mut report, "ORPHAN_LINE", format!("SIG references unknown epoch `{id}`"));
                        continue;
                    }
                };
                if ObsCode(code.to_string()).kind().is_none() {
                    push_err(
                        &mut report,
                        "UNKNOWN_CODE",
                        format!("observation code `{code}` has unrecognised leading letter"),
                    );
                }
                if ObsCode(code.to_string()).freq_num().is_none() {
                    push_err(
                        &mut report,
                        "BAD_FREQ_NUM",
                        format!("observation code `{code}` has no frequency band digit"),
                    );
                }
                let (qty, diag) = parse_qty(rawv, code);
                let mut lli = 0u8;
                let mut ssi = None;
                for opt in &parts[5..] {
                    if let Some(v) = opt.strip_prefix("LLI=") {
                        lli = v.parse().unwrap_or(0);
                    } else if let Some(v) = opt.strip_prefix("SSI=") {
                        ssi = Some(v.to_string());
                    }
                }
                if let Some(mut d) = diag {
                    d.line_no = line_no;
                    d.raw_line = raw_line.to_string();
                    report.diagnostics.push(d);
                }
                let sat_sigs =
                    get_or_insert_sat(&mut epochs[slot].signals, sat.to_string());
                sat_sigs.push(SignalObs {
                    code: ObsCode(code.to_string()),
                    value: qty,
                    lli,
                    strength_raw: ssi,
                });
            }
            _ => push_err(
                &mut report,
                "UNKNOWN_DIRECTIVE",
                format!("line starts with `{}` which is not EPOCH/STATION/CLK/SAT_STATE/SIG/META", parts[0]),
            ),
        }
    }

    // Attach satellite states to every epoch that declares that satellite.
    for (line_no, p) in sat_state_lines {
        let sat = p[1].clone();
        let mut nums = [0.0f64; 4];
        let mut ok = true;
        for (k, tok) in p[2..6].iter().enumerate() {
            match tok.parse::<f64>() {
                Ok(v) => nums[k] = v,
                Err(_) => {
                    ok = false;
                    report.diagnostics.push(ImportDiagnostic {
                        line_no,
                        severity: crate::raw_model::DiagSeverity::Warning,
                        code: "UNCONVERTIBLE_VALUE".to_string(),
                        message: format!("sat_state `{sat}` field `{tok}` not numeric"),
                        raw_line: String::new(),
                    });
                }
            }
        }
        let state = SatelliteState {
            sat: sat.clone(),
            x: if ok { Qty::new(nums[0], unit::METER, datum::WGS84, &p[2]) } else { Qty::unconvertible(&p[2], unit::METER, "bad x") },
            y: if ok { Qty::new(nums[1], unit::METER, datum::WGS84, &p[3]) } else { Qty::unconvertible(&p[3], unit::METER, "bad y") },
            z: if ok { Qty::new(nums[2], unit::METER, datum::WGS84, &p[4]) } else { Qty::unconvertible(&p[4], unit::METER, "bad z") },
            clock_bias: if ok { Qty::new(nums[3], unit::SECOND, datum::GPST, &p[5]) } else { Qty::unconvertible(&p[5], unit::SECOND, "bad clock") },
        };
        for e in &mut epochs {
            if e.signals.iter().any(|(s, _)| s == &sat) {
                e.sat_states.push(state.clone());
            }
        }
    }

    // Fill default station from the first non-empty station for determinism.
    let default_station = epochs.iter().find_map(|e| if e.station.is_empty() { None } else { Some(e.station.clone()) });
    for e in &mut epochs {
        if e.station.is_empty() {
            if let Some(s) = &default_station {
                e.station = s.clone();
            } else {
                e.station = "STA1".to_string();
            }
        }
    }
    epochs.sort_by(|a, b| a.t_seconds.partial_cmp(&b.t_seconds).unwrap_or(std::cmp::Ordering::Equal));
    report.accepted_epochs = epochs.len();
    (epochs, report)
}

fn find(index: &[(String, usize)], id: &str) -> Option<usize> {
    index.iter().find(|(e, _)| e == id).map(|(_, i)| *i)
}

fn get_or_insert_sat<'a>(
    signals: &'a mut Vec<(String, Vec<SignalObs>)>,
    sat: String,
) -> &'a mut Vec<SignalObs> {
    let pos = signals.iter().position(|(s, _)| *s == sat);
    match pos {
        Some(p) => &mut signals[p].1,
        None => {
            signals.push((sat, Vec::new()));
            signals.last_mut().unwrap().1
        }
    }
}
