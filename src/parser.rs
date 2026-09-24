use crate::model::*;

pub fn parse_observations(text: &str) -> Result<ParsedObservationFile, String> {
    let mut frequencies = Vec::new();
    let mut obs_types = Vec::new();
    let mut epochs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut current_epoch: Option<ParsedEpoch> = None;

    for (zero_line, original) in text.lines().enumerate() {
        let line_no = zero_line as i64 + 1;
        let line = original.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("FREQUENCY ") {
            let mut parts = rest.split_whitespace();
            let system = parts.next().ok_or_else(|| format!("line {line_no}: missing frequency system"))?;
            while let Some(code) = parts.next() {
                let mhz_raw = parts.next().ok_or_else(|| format!("line {line_no}: missing MHz for {code}"))?;
                let mhz = mhz_raw.parse::<f64>().map_err(|_| format!("line {line_no}: invalid frequency {mhz_raw}"))?;
                let unit = parts.next().unwrap_or("MHz").to_string();
                frequencies.push(RawFrequency {
                    system: system.to_string(),
                    carrier_code: code.to_string(),
                    mhz,
                    unit,
                    raw: format!("{code} {mhz_raw} {unit}"),
                });
            }
        } else if let Some(rest) = line.strip_prefix("OBS_TYPES ") {
            for code in rest.split_whitespace() {
                if obs_types.contains(&code.to_string()) {
                    return Err(format!("line {line_no}: duplicate observation code {code}"));
                }
                obs_types.push(code.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("> EPOCH ") {
            if let Some(epoch) = current_epoch.take() {
                epochs.push(epoch);
            }
            let mut pieces = rest.split_whitespace();
            let epoch = pieces.next().ok_or_else(|| format!("line {line_no}: missing epoch timestamp"))?.to_string();
            let mut clock_offset_ns = None;
            while let Some(key) = pieces.next() {
                if key == "CLOCK_NS" {
                    let raw = pieces.next().ok_or_else(|| format!("line {line_no}: missing clock value"))?;
                    clock_offset_ns = Some(parse_number(raw).map_err(|e| format!("line {line_no}: {e}"))?);
                } else {
                    return Err(format!("line {line_no}: unknown epoch field {key}"));
                }
            }
            let t_seconds = epoch_t_seconds(&epoch).ok_or_else(|| format!("line {line_no}: invalid epoch timestamp {epoch}"))?;
            current_epoch = Some(ParsedEpoch {
                epoch,
                t_seconds,
                clock_offset_ns,
                raw_line: original.to_string(),
                line_no,
                satellites: Vec::new(),
            });
        } else if line.starts_with("EVENT CLOCK_JUMP") {
            // Clock jumps belong with satellite status. They are tolerated in an observation
            // paste but reported as a diagnostic so provenance is not silently relocated.
            diagnostics.push(Diagnostic {
                line_no,
                severity: "warning".into(),
                code: "clock-event-in-observation-file".into(),
                message: "receiver clock jump event ignored in observation import".into(),
                raw_line: original.to_string(),
            });
        } else if is_satellite_start(line) {
            let epoch = current_epoch.as_mut().ok_or_else(|| format!("line {line_no}: satellite record without > EPOCH"))?;
            let mut tokens = line.split_whitespace();
            let sat = tokens.next().expect("satellite id").to_string();
            if sat.len() < 3 {
                return Err(format!("line {line_no}: invalid satellite id {sat}"));
            }
            let mut fields = Vec::new();
            for token in tokens {
                let field = parse_field_token(token, line_no, original)?;
                if !obs_types.is_empty() && !obs_types.contains(&field.code) {
                    return Err(format!("line {line_no}: observation code {} is not declared in OBS_TYPES", field.code));
                }
                if field.status == "unconvertible" {
                    diagnostics.push(Diagnostic {
                        line_no,
                        severity: "warning".into(),
                        code: "unconvertible-observation".into(),
                        message: format!("{}={} remains in its original location", field.code, field.raw.clone().unwrap_or_default()),
                        raw_line: original.to_string(),
                    });
                }
                fields.push(field);
            }
            epoch.satellites.push(ParsedSatellite {
                sat,
                raw_line: original.to_string(),
                line_no,
                fields,
            });
        } else {
            return Err(format!("line {line_no}: unrecognized RINEX observation record: {line}"));
        }
    }
    if let Some(epoch) = current_epoch {
        epochs.push(epoch);
    }
    if frequencies.is_empty() {
        return Err("missing FREQUENCY header".into());
    }
    if obs_types.is_empty() {
        return Err("missing OBS_TYPES header".into());
    }
    if epochs.is_empty() {
        return Err("observation file contains no epochs".into());
    }
    let first = epochs[0].epoch.clone();
    for epoch in &epochs {
        if epoch.t_seconds <= epochs.iter().find(|e| e.epoch == first).map(|e| e.t_seconds).unwrap_or(0.0) && epoch.epoch != first {
            return Err(format!("epoch {} is not strictly increasing", epoch.epoch));
        }
    }
    Ok(ParsedObservationFile { frequencies, obs_types, epochs, diagnostics })
}

fn parse_field_token(token: &str, line_no: i64, original: &str) -> Result<RawField, String> {
    let (code, rest) = token.split_once('=').ok_or_else(|| format!("line {line_no}: expected CODE=value, got {token}"))?;
    if code.len() < 2 {
        return Err(format!("line {line_no}: invalid observation code {code}"));
    }
    if rest == "-" {
        return Ok(RawField {
            code: code.into(),
            raw: Some("-".into()),
            value: None,
            unit: unit_for_code(code),
            lli: None,
            status: "missing".into(),
        });
    }
    let (value_part, lli_part) = match rest.strip_suffix(')') {
        Some(_) => match rest.find('(') {
            Some(index) => (&rest[..index], Some(&rest[index + 1..rest.len() - 1])),
            None => return Err(format!("line {line_no}: unmatched LLI parenthesis in {token}")),
        },
        None => (rest, None),
    };
    let raw = Some(rest.to_string());
    let parsed = parse_number(value_part);
    let lli = match lli_part {
        None => None,
        Some(value) => Some(value.parse::<i64>().map_err(|_| format!("line {line_no}: invalid LLI {value}"))?),
    };
    let (value, status) = match parsed {
        Ok(value) => (Some(value), if lli.is_some() { "ok-with-lli".into() } else { "ok".into() }),
        Err(_) => (None, "unconvertible".into()),
    };
    Ok(RawField { code: code.into(), raw, value, unit: unit_for_code(code), lli, status })
}

fn is_satellite_start(line: &str) -> bool {
    let mut chars = line.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && line.get(1..3).is_some_and(|digits| digits.chars().all(|c| c.is_ascii_digit()))
}

pub fn parse_status(text: &str) -> Result<ParsedStatusFile, String> {
    let mut datum = None;
    let mut epochs = Vec::new();
    let mut clock_events = Vec::new();
    let mut diagnostics = Vec::new();
    for (zero_line, original) in text.lines().enumerate() {
        let line_no = zero_line as i64 + 1;
        let line = original.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("DATUM ") {
            datum = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("EVENT CLOCK_JUMP ") {
            let mut epoch = None;
            let mut delta = None;
            let mut kind = "receiver-clock-jump".to_string();
            for token in rest.split_whitespace() {
                let (key, value) = token.split_once('=').ok_or_else(|| format!("line {line_no}: expected key=value"))?;
                match key {
                    "epoch" => epoch = Some(value.to_string()),
                    "kind" => kind = value.to_string(),
                    "delta_s" => delta = Some(parse_number(value).map_err(|e| format!("line {line_no}: {e}"))?),
                    _ => return Err(format!("line {line_no}: unknown clock event key {key}")),
                }
            }
            let epoch = epoch.ok_or_else(|| format!("line {line_no}: clock event missing epoch"))?;
            let t_seconds = epoch_t_seconds(&epoch).ok_or_else(|| format!("line {line_no}: invalid clock epoch"))?;
            clock_events.push(ClockEvent {
                epoch,
                t_seconds,
                kind,
                delta_seconds: delta.ok_or_else(|| format!("line {line_no}: clock event missing delta_s"))?,
                raw: original.to_string(),
                line_no,
            });
        } else if line.contains('|') {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() < 3 {
                return Err(format!("line {line_no}: status line needs epoch, satellite, and fields"));
            }
            let epoch = cells[0].to_string();
            let sat = cells[1].to_string();
            let t_seconds = epoch_t_seconds(&epoch).ok_or_else(|| format!("line {line_no}: invalid status epoch"))?;
            let mut fields = Vec::new();
            for cell in &cells[2..] {
                let (left, unit) = match cell.rfind('[') {
                    Some(start) if cell.ends_with(']') => (&cell[..start], cell[start + 1..cell.len() - 1].to_string()),
                    _ => return Err(format!("line {line_no}: status value {cell} must include [unit]")),
                };
                let (key, raw_value) = left.split_once('=').ok_or_else(|| format!("line {line_no}: expected key=value in {cell}"))?;
                let value = parse_number(raw_value).ok();
                if value.is_none() {
                    diagnostics.push(Diagnostic {
                        line_no,
                        severity: "warning".into(),
                        code: "unconvertible-satellite-state".into(),
                        message: format!("{key}={raw_value} remains in its original location"),
                        raw_line: original.to_string(),
                    });
                }
                fields.push(StatusField { key: key.into(), raw: raw_value.into(), value, unit });
            }
            epochs.push(ParsedStatusEpoch {
                epoch,
                t_seconds,
                sat,
                datum: datum.clone().unwrap_or_else(|| "UNDECLARED".into()),
                raw_line: original.to_string(),
                line_no,
                fields,
            });
        } else {
            return Err(format!("line {line_no}: unrecognized satellite status record: {line}"));
        }
    }
    if epochs.is_empty() && clock_events.is_empty() {
        return Err("status file contains no epochs or clock events".into());
    }
    Ok(ParsedStatusFile { datum: datum.unwrap_or_else(|| "UNDECLARED".into()), epochs, clock_events, diagnostics })
}

fn parse_number(input: &str) -> Result<f64, String> {
    input.parse::<f64>().map_err(|_| format!("invalid number {input}"))
}

fn unit_for_code(code: &str) -> String {
    match code.chars().next() {
        Some('C') => "m".into(),
        Some('L') => "cycle".into(),
        Some('D') => "Hz".into(),
        Some('S') => "dBHz".into(),
        _ => "unknown".into(),
    }
}

pub fn epoch_t_seconds(epoch: &str) -> Option<f64> {
    let (date, time) = epoch.split_once('T')?;
    let mut d = date.split('-');
    let year: i32 = d.next()?.parse().ok()?;
    let month: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hour: u32 = t.next()?.parse().ok()?;
    let minute: u32 = t.next()?.parse().ok()?;
    let second: f64 = t.next()?.parse().ok()?;
    let mut y = year;
    let mut days: i64 = 0;
    if month <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 };
    let era_years = era / 400;
    let yoe = era - era_years * 400;
    let doy = (153 * (u32::try_from((153 * ((month as i64 + 9) % 12) + 2) / 5 + day - 1).ok()?) as i64 + 2) / 5 + day as i64 - 1;
    days += era_years as i64 * 146097 + yoe as i64 * 365 + yoe as i64 / 4 - yoe as i64 / 100 + doy;
    days -= 719468;
    let seconds = days as f64 * 86400.0 + hour as f64 * 3600.0 + minute as f64 * 60.0 + second;
    Some(seconds)
}
