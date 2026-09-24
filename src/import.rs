//! Import layer.
//!
//! Accepts (a) a small flattened dialect **CBS-OBS/1** (a documented subset of
//! RINEX observation encoding) and (b) a RINEX 3.xx observation header/epoch
//! subset. Pseudorange, carrier, Doppler, LLI, S/N, clock-jump events,
//! frequency number and observation code are preserved verbatim per epoch.
//!
//! File-level format errors reject the whole batch; value-level conversion
//! failures stay in place with a diagnostic (`value = NULL`) rather than being
//! dropped to make plots work.

use crate::model::{Band, RawObs, RawValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SatState {
    pub station: String,
    pub epoch_iso: String,
    pub sat: String,
    pub elevation_deg: Option<f64>,
    pub azimuth_deg: Option<f64>,
    pub health: Option<String>,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClockEvent {
    pub station: String,
    pub epoch_iso: String,
    pub kind: String,
    pub note: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParseReport {
    pub format: String,
    pub station: String,
    pub obs_count: usize,
    pub epoch_count: usize,
    pub state_count: usize,
    pub clock_event_count: usize,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParsedBundle {
    pub obs: Vec<RawObs>,
    pub states: Vec<SatState>,
    pub clock_events: Vec<ClockEvent>,
    pub report: ParseReport,
}

#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ParseError {}

fn parse_num(raw: &str, unit: &str, diags: &mut Vec<String>, ctx: &str) -> RawValue {
    let t = raw.trim();
    if t.is_empty() {
        return RawValue { raw: raw.to_string(), value: None, diag: None };
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => RawValue::ok(raw, v),
        _ => {
            let d = format!("{}: cannot convert {:?} to {}", ctx, t, unit);
            diags.push(d.clone());
            RawValue::bad(raw, &d)
        }
    }
}

fn band_and_field(code: &str) -> Option<(Band, &'static str)> {
    let mut chars = code.chars();
    let typ = chars.next()?;
    let num = chars.next()?;
    let fnum = num.to_digit(10)? as u8;
    let field = match typ {
        'C' => "C",
        'L' => "L",
        'D' => "D",
        'S' => "S",
        _ => return None,
    };
    Some((Band::from_num(fnum), field))
}

/// Parse a full submission: header lines (`# ...`, `STATION`, etc.), obs rows,
/// state rows (`STATE`), explicit clock events (`CLKEV`).
pub fn parse_submission(text: &str) -> Result<ParsedBundle, ParseError> {
    let mut station = String::new();
    let mut format = String::from("CBS-OBS/1");
    let mut obs: BTreeMap<(String, String, String, u8), RawObs> = BTreeMap::new();
    let mut states: Vec<SatState> = Vec::new();
    let mut clock_events: Vec<ClockEvent> = Vec::new();
    let mut diags: Vec<String> = Vec::new();
    let mut epochs = std::collections::BTreeSet::new();

    for (lineno, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let upper = line.trim_start().to_ascii_uppercase();
        if upper.starts_with("# CBS-OBS/1") || upper.starts_with("# CBS-OBS") {
            format = "CBS-OBS/1".into();
            continue;
        }
        if upper.starts_with("# RINEX VERSION") || upper.contains("RINEX VERSION / TYPE") {
            format = "RINEX-3-subset".into();
            continue;
        }
        if upper.starts_with('#') {
            if let Some(rest) = line.trim_start().trim_start_matches('#').trim().strip_prefix("STATION") {
                station = rest.split_whitespace().next().unwrap_or("").to_string();
            }
            continue;
        }
        let mut tokens = line.split_whitespace();
        let tag = tokens.next().unwrap_or("").to_ascii_uppercase();
        match tag.as_str() {
            "STATION" => {
                let s = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: STATION requires an id", lineno + 1))
                })?;
                station = s.to_string();
            }
            "CLKEV" | "CLOCKJUMP" => {
                let epoch = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: CLKEV requires an epoch", lineno + 1))
                })?;
                let kind = tokens.next().unwrap_or("clock_jump");
                let note = tokens.collect::<Vec<_>>().join(" ");
                clock_events.push(ClockEvent {
                    station: station.clone(),
                    epoch_iso: epoch.to_string(),
                    kind: kind.to_string(),
                    note,
                    raw_line: line.to_string(),
                });
                epochs.insert(epoch.to_string());
            }
            "STATE" => {
                let epoch = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: STATE requires epoch sat elev az health", lineno + 1))
                })?;
                let sat = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: STATE requires sat", lineno + 1))
                })?;
                let elev_raw = tokens.next().unwrap_or("");
                let az_raw = tokens.next().unwrap_or("");
                let health = tokens.next().unwrap_or("");
                let elev = parse_num(
                    elev_raw,
                    "deg",
                    &mut diags,
                    &format!("line {} STATE elev", lineno + 1),
                );
                let az = parse_num(
                    az_raw,
                    "deg",
                    &mut diags,
                    &format!("line {} STATE azimuth", lineno + 1),
                );
                states.push(SatState {
                    station: station.clone(),
                    epoch_iso: epoch.to_string(),
                    sat: sat.to_string(),
                    elevation_deg: elev.value,
                    azimuth_deg: az.value,
                    health: if health.is_empty() { None } else { Some(health.to_string()) },
                    raw_line: line.to_string(),
                });
                epochs.insert(epoch.to_string());
            }
            "OBS" => {
                let epoch = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: OBS requires epoch sat code field=value ...", lineno + 1))
                })?;
                let sat = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: OBS requires sat", lineno + 1))
                })?;
                let code_tok = tokens.next().ok_or_else(|| {
                    ParseError(format!("line {}: OBS requires observation code", lineno + 1))
                })?;
                let (band, field) = band_and_field(code_tok).ok_or_else(|| {
                    ParseError(format!(
                        "line {}: observation code {:?} must start with C/L/D/S followed by a frequency number",
                        lineno + 1,
                        code_tok
                    ))
                })?;
                if station.is_empty() {
                    return Err(ParseError(format!(
                        "line {}: OBS before any STATION declaration",
                        lineno + 1
                    )));
                }
                epochs.insert(epoch.to_string());
                let key = (station.clone(), sat.to_string(), epoch.to_string(), band.num());
                let entry = obs.entry(key).or_insert_with(|| RawObs {
                    station: station.clone(),
                    sat: sat.to_string(),
                    epoch_iso: epoch.to_string(),
                    band,
                    code: code_tok.to_string(),
                    fields: BTreeMap::new(),
                });
                // Keep the first code seen but record any additional code into the diag channel.
                if entry.code != code_tok && entry.code.chars().nth(1) == code_tok.chars().nth(1) {
                    entry.code = code_tok.to_string();
                }
                let mut saw_field = false;
                for fv in tokens {
                    let mut it = fv.splitn(2, '=');
                    let name = it.next().unwrap_or("");
                    let raw = it.next().unwrap_or("");
                    if name == "LLI" {
                        // Loss-of-lock indicator: a physical flag carried beside
                        // the phase field, preserved verbatim.
                        let digit = raw.trim().chars().next().unwrap_or('0');
                        let bit = if digit == '1' || digit == '3' || digit == '5' || digit == '7' { 1.0 } else { 0.0 };
                        entry.fields.insert(
                            "LLI".into(),
                            RawValue { raw: raw.to_string(), value: Some(bit), diag: None },
                        );
                        saw_field = true;
                        continue;
                    }
                    if name != field {
                        diags.push(format!(
                            "line {}: field {} inconsistent with code {} (code preserved; token kept under {})",
                            lineno + 1, name, code_tok, field
                        ));
                    }
                    let unit = match field {
                        "C" => "m",
                        "L" => "cycles",
                        "D" => "Hz",
                        "S" => "dBHz",
                        _ => "",
                    };
                    let v = parse_num(raw, unit, &mut diags, &format!("line {} {}/{}", lineno + 1, code_tok, field));
                    entry.fields.insert(field.to_string(), v);
                    saw_field = true;
                }
                if !saw_field {
                    return Err(ParseError(format!(
                        "line {}: OBS {} {} {} requires at least one {}=<value>",
                        lineno + 1,
                        epoch,
                        sat,
                        code_tok,
                        field
                    )));
                }
            }
            _ => {
                return Err(ParseError(format!(
                    "line {}: unknown record tag {:?} (expected STATION/OBS/STATE/CLKEV/#)",
                    lineno + 1,
                    tag
                )));
            }
        }
    }

    if station.is_empty() {
        return Err(ParseError("submission has no STATION declaration".into()));
    }
    if obs.is_empty() {
        return Err(ParseError("submission contains no OBS records".into()));
    }

    // LLI is a separate physical flag. Accept rows tagged LLI (code L<f>Q form)
    // via LLI= token; also a standalone LLI record.
    let mut obs_vec: Vec<RawObs> = obs.into_values().collect();
    obs_vec.sort_by(|a, b| {
        a.epoch_iso
            .cmp(&b.epoch_iso)
            .then(a.sat.cmp(&b.sat))
            .then(a.band.cmp(&b.band))
    });

    let mut report = ParseReport {
        format,
        station: obs_vec.first().map(|o| o.station.clone()).unwrap_or_default(),
        obs_count: obs_vec.len(),
        epoch_count: epochs.len(),
        state_count: states.len(),
        clock_event_count: clock_events.len(),
        diagnostics: diags,
    };
    let _ = &mut report;
    Ok(ParsedBundle { obs: obs_vec, states, clock_events, report })
}

/// Apply standalone `LLI=` supplied in a separate pass. Called for records of
/// shape `LLI <epoch> <sat> L<f> <flags>` — kept separate because LLI is a flag,
/// not a measurement.
pub fn apply_lli_lines(bundle: &mut ParsedBundle, text: &str) -> Result<(), ParseError> {
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut t = line.split_whitespace();
        let tag = t.next().unwrap_or("");
        if tag != "LLI" {
            continue;
        }
        let epoch = t.next().ok_or_else(|| ParseError(format!("LLI line {}: epoch missing", lineno + 1)))?;
        let sat = t.next().ok_or_else(|| ParseError(format!("LLI line {}: sat missing", lineno + 1)))?;
        let code = t.next().ok_or_else(|| ParseError(format!("LLI line {}: code missing", lineno + 1)))?;
        let flag = t.next().ok_or_else(|| ParseError(format!("LLI line {}: flag missing", lineno + 1)))?;
        let band = code
            .chars()
            .nth(1)
            .and_then(|c| c.to_digit(10))
            .map(|n| Band::from_num(n as u8))
            .ok_or_else(|| ParseError(format!("LLI line {}: bad code {}", lineno + 1, code)))?;
        let obs = bundle
            .obs
            .iter_mut()
            .find(|o| o.epoch_iso == epoch && o.sat == sat && o.band == band);
        if let Some(o) = obs {
            o.fields.insert(
                "LLI".into(),
                RawValue { raw: flag.to_string(), value: Some(lli_flag_bit(flag) as f64), diag: None },
            );
        } else {
            bundle.report.diagnostics.push(format!(
                "LLI line {}: no observation {}/{}/{:?} (flag retained nowhere; reported)",
                lineno + 1,
                epoch,
                sat,
                band
            ));
        }
    }
    Ok(())
}

pub fn lli_flag_bit(flag: &str) -> u8 {
    // RINEX loss-of-lock indicator: bit 0 set => possible cycle slip.
    match flag.trim().chars().next() {
        Some(c) if c == '1' || c == '3' || c == '5' || c == '7' => 1,
        _ => 0,
    }
}

pub fn lli_set(v: Option<f64>) -> bool {
    v.map(|x| (x as i64 & 1) == 1).unwrap_or(false)
}

/// Parse satellite-state-only file (header `# CBS-STATE/1`).
pub fn parse_states(text: &str, station_default: &str) -> Result<Vec<SatState>, ParseError> {
    let mut out = Vec::new();
    let mut diags = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut t = line.split_whitespace();
        let tag = t.next().unwrap_or("");
        if tag == "STATION" {
            continue;
        }
        if tag != "STATE" {
            return Err(ParseError(format!("state file line {}: expected STATE", lineno + 1)));
        }
        let epoch = t.next().ok_or_else(|| ParseError(format!("state line {} no epoch", lineno + 1)))?;
        let sat = t.next().ok_or_else(|| ParseError(format!("state line {} no sat", lineno + 1)))?;
        let e = t.next().unwrap_or("");
        let a = t.next().unwrap_or("");
        let h = t.next().unwrap_or("");
        let ev = parse_num(e, "deg", &mut diags, "state elev");
        let av = parse_num(a, "deg", &mut diags, "state az");
        out.push(SatState {
            station: station_default.into(),
            epoch_iso: epoch.into(),
            sat: sat.into(),
            elevation_deg: ev.value,
            azimuth_deg: av.value,
            health: if h.is_empty() { None } else { Some(h.into()) },
            raw_line: line.into(),
        });
    }
    Ok(out)
}
