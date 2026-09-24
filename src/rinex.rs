use crate::model::*;
use serde_json::Value;

pub const RANGE_DATUM: &str = "antenna_phase_center<->satellite_CoM(WGS84-compatible)";
pub const TIME_DATUM: &str = "GPST seconds of week";
pub const SNR_DATUM: &str = "receiver tracking loop (dB-Hz)";

pub fn diag(severity: &str, code: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: severity.to_string(),
        code: code.to_string(),
        message: message.into(),
        context: None,
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn tow_from_epoch(year: i32, month: i32, day: i32, hour: i32, minute: i32, sec: f64) -> (f64, i64) {
    let days = days_from_civil(year as i64, month as i64, day as i64);
    let gps_epoch = days_from_civil(1980, 1, 6);
    let day_delta = days - gps_epoch;
    let week = day_delta.div_euclid(7);
    let dow = day_delta.rem_euclid(7);
    let tow = dow as f64 * 86400.0 + hour as f64 * 3600.0 + minute as f64 * 60.0 + sec;
    (tow, week)
}

pub fn band_of(code: &str) -> Option<u8> {
    code.chars().nth(1).and_then(|c| c.to_digit(10)).map(|d| d as u8)
}

pub fn class_unit(class: char) -> (&'static str, &'static str) {
    match class {
        'C' | 'P' => ("code", "m"),
        'L' => ("phase", "cycle"),
        'D' => ("doppler", "Hz"),
        'S' => ("snr", "dB-Hz"),
        _ => ("other", "unknown"),
    }
}

pub struct ParsedFile {
    pub kind: String,
    pub station: String,
    pub observations: Vec<RawObservation>,
    pub clock_events: Vec<ClockEvent>,
    pub sat_states: Vec<Value>,
    pub diagnostics: Vec<Diagnostic>,
}

fn parse_obs_field(field: &str) -> (String, Option<f64>, Option<u8>, Option<u8>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let raw = field.to_string();
    if field.trim().is_empty() {
        return (raw, None, None, None, diags);
    }
    let bytes = field.as_bytes();
    let (num_part, lli_part, ssi_part) = if bytes.len() >= 13 {
        (field[..11].trim(), Some(bytes[11] as char), Some(bytes[12] as char))
    } else {
        (field.trim(), None, None)
    };
    if num_part.is_empty() {
        return (raw, None, None, None, diags);
    }
    let value: Option<f64> = match num_part.parse::<f64>() {
        Ok(v) => Some(v),
        Err(_) => {
            diags.push(diag(
                "error",
                "unconvertible_observation",
                format!("observation value '{num_part}' could not be converted to f64; kept in place"),
            ));
            None
        }
    };
    let lli = lli_part.and_then(|c| c.to_digit(10)).map(|d| d as u8);
    let ssi = ssi_part.and_then(|c| c.to_digit(10)).map(|d| d as u8);
    (raw, value, lli, ssi, diags)
}

fn is_numeric_epoch_header(fields: &[&str]) -> bool {
    if fields.len() < 8 {
        return false;
    }
    fields[1..=6].iter().all(|t| t.parse::<f64>().is_ok())
}

pub fn parse_rinex(content: &str) -> ParsedFile {
    let mut diags = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut pos = 0usize;
    let mut version = String::new();
    let mut file_type = ' ';
    let mut station = String::new();
    let mut obs_types: std::collections::BTreeMap<char, Vec<String>> = std::collections::BTreeMap::new();

    while pos < lines.len() {
        let line = lines[pos];
        let label = if line.len() >= 60 { line[60..].trim() } else { line.trim() };
        if label == "END OF HEADER" {
            pos += 1;
            break;
        }
        match label {
            "RINEX VERSION / TYPE" => {
                version = line.get(0..9).unwrap_or("").trim().to_string();
                file_type = line.chars().nth(20).unwrap_or(' ');
                if file_type != 'O' {
                    diags.push(diag("error", "not_observation_file",
                        format!("RINEX file type is '{file_type}', expected 'O' (observation data)")));
                }
                if version.starts_with('2') {
                    diags.push(diag("error", "rinex2_unsupported",
                        "only RINEX 3 observation files are supported by this importer"));
                }
            }
            "MARKER NAME" => station = line.get(0..4).unwrap_or("").trim().to_string(),
            "SYS / # / OBS TYPES" => {
                let sys = line.chars().next().unwrap_or(' ');
                let n: usize = line.get(3..6).unwrap_or(" 0").trim().parse().unwrap_or(0);
                let mut codes = Vec::new();
                for k in 0..5 {
                    let start = 7 + k * 4;
                    if let Some(tok) = line.get(start..start + 4) {
                        let tok = tok.trim();
                        if !tok.is_empty() {
                            codes.push(tok.to_string());
                        }
                    }
                }
                while codes.len() < n && pos + 1 < lines.len() {
                    pos += 1;
                    let cont = lines[pos];
                    let cont_label = if cont.len() >= 60 { cont[60..].trim() } else { "" };
                    if cont_label != "SYS / # / OBS TYPES" {
                        pos -= 1;
                        break;
                    }
                    for k in 0..7 {
                        let start = 7 + k * 4;
                        if let Some(tok) = cont.get(start..start + 4) {
                            let tok = tok.trim();
                            if !tok.is_empty() {
                                codes.push(tok.to_string());
                            }
                        }
                    }
                }
                obs_types.insert(sys, codes);
            }
            _ => {}
        }
        pos += 1;
    }

    let mut observations = Vec::new();
    let mut epoch_idx = 0usize;

    while pos < lines.len() {
        let line = lines[pos];
        if !line.starts_with('>') {
            pos += 1;
            continue;
        }
        let epoch_fields: Vec<&str> = line.split_whitespace().collect();
        pos += 1;
        // After the epoch flag, the satellite count and the satellite list may be
        // concatenated without whitespace (fixed-column RINEX), so rebuild the tail
        // and split it into a leading integer plus 3-character satellite ids.
        let mut tail: String = epoch_fields[8..].concat();
        while pos < lines.len() && lines[pos].starts_with('>')
            && !is_numeric_epoch_header(&lines[pos].split_whitespace().collect::<Vec<_>>())
        {
            let cont_fields: Vec<&str> = lines[pos].split_whitespace().collect();
            tail.push_str(&cont_fields[1..].concat());
            pos += 1;
        }
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n_sat: usize = digits.parse().unwrap_or(0);
        let satblob = &tail[digits.len()..];
        let sats: Vec<String> = satblob
            .as_bytes()
            .chunks(3)
            .map(|c| String::from_utf8_lossy(c).trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let nums: Vec<f64> = epoch_fields[1..=6]
            .iter()
            .filter_map(|t| t.parse().ok())
            .collect();
        if nums.len() < 6 {
            diags.push(diag("error", "bad_epoch_header",
                format!("epoch header near line {} has unparsable date/time; satellites skipped", pos + 1)));
            continue;
        }
        let year = nums[0] as i32;
        let (month, day, hour, minute) = (nums[1] as i32, nums[2] as i32, nums[3] as i32, nums[4] as i32);
        let sec = nums[5];
        let (tow_s, week) = tow_from_epoch(year, month, day, hour, minute, sec);
        let epoch_flag = epoch_fields.get(7).copied().unwrap_or("?").to_string();
        if epoch_flag != "0" {
            diags.push(diag("info", "non_nominal_epoch_flag",
                format!("epoch {epoch_idx} ({tow_s} s) has RINEX epoch flag {epoch_flag}")));
        }
        let epoch_text = format!("{:04} {:02} {:02} {:02} {:02} {:011.7} (GPS week {week})",
            year, month, day, hour, minute, sec);

        for sat in sats.iter().take(n_sat) {
            let sys = sat.chars().next().unwrap_or(' ');
            let codes = match obs_types.get(&sys) {
                Some(c) => c.clone(),
                None => {
                    diags.push(diag("warning", "unknown_system",
                        format!("satellite {sat} at epoch {epoch_idx}: no OBS TYPES declared for system '{sys}'")));
                    Vec::new()
                }
            };
            let n_lines = (codes.len() + 4) / 5;
            let mut sat_lines = String::new();
            for ln in 0..n_lines {
                if pos >= lines.len() {
                    diags.push(diag("error", "truncated_observation_record",
                        format!("satellite {sat} at epoch {epoch_idx}: observation record truncated")));
                    break;
                }
                let ol = lines[pos];
                pos += 1;
                if ln == 0 {
                    sat_lines.push_str(ol.get(4..).unwrap_or(ol));
                } else {
                    sat_lines.push_str(ol.get(3..).unwrap_or(ol));
                }
            }
            for (i, code) in codes.iter().enumerate() {
                let chunk = sat_lines.as_bytes().chunks(14).nth(i);
                let field = match chunk {
                    Some(b) => String::from_utf8_lossy(b).to_string(),
                    None => " ".repeat(14),
                };
                let (value_raw, value, lli, ssi, mut fd) = parse_obs_field(&field);
                diags.append(&mut fd);
                let class = code.chars().next().unwrap_or('?');
                let (obs_class, unit) = class_unit(class);
                let datum = match class {
                    'C' | 'P' => RANGE_DATUM.to_string(),
                    'L' => format!("full carrier cycle, {TIME_DATUM}"),
                    'D' => format!("instantaneous Doppler, {TIME_DATUM}"),
                    'S' => SNR_DATUM.to_string(),
                    _ => "unknown datum".to_string(),
                };
                if !value_raw.trim().is_empty() && band_of(code).is_none() {
                    diags.push(diag("warning", "unknown_observation_code",
                        format!("code {code} ({sat} epoch {epoch_idx}) has no band digit; kept raw, excluded from combinations")));
                }
                let id = crate::ids::id12("obs", &[sat, code, &epoch_text]);
                observations.push(RawObservation {
                    id, raw_id: String::new(), epoch_idx, tow_s,
                    epoch_text: epoch_text.clone(), epoch_flag: epoch_flag.clone(),
                    sat: sat.clone(), code: code.clone(),
                    band: band_of(code), obs_class: obs_class.to_string(),
                    value_raw, value, lli, ssi, unit: unit.to_string(), datum,
                    raw_line: field.clone(), diagnostics: Vec::new(),
                });
            }
        }
        epoch_idx += 1;
    }

    ParsedFile {
        kind: "rinex_obs".to_string(),
        station,
        observations,
        clock_events: Vec::new(),
        sat_states: Vec::new(),
        diagnostics: diags,
    }
}

fn json_diag_list(v: &Value, code: &str, msg: &str) -> Diagnostic {
    let mut d = diag("error", code, msg);
    d.context = Some(v.clone());
    d
}

pub fn parse_json_sample(content: &str) -> ParsedFile {
    let mut diags = Vec::new();
    let root: Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            return ParsedFile {
                kind: "unknown".into(),
                station: String::new(),
                observations: vec![],
                clock_events: vec![],
                sat_states: vec![],
                diagnostics: vec![diag("error", "invalid_json", format!("sample is not valid JSON: {e}"))],
            };
        }
    };
    let kind = root.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    match kind.as_str() {
        "obs_sample" => parse_obs_sample(&root, &mut diags),
        "clock_events" => parse_clock_events(&root, &mut diags),
        "sat_states" => parse_sat_states(&root, &mut diags),
        other => ParsedFile {
            kind: "unknown".into(),
            station: String::new(),
            observations: vec![],
            clock_events: vec![],
            sat_states: vec![],
            diagnostics: vec![diag("error", "unknown_sample_kind",
                format!("sample kind '{other}' is not one of obs_sample | clock_events | sat_states"))],
        },
    }
}

fn parse_obs_sample(root: &Value, diags: &mut Vec<Diagnostic>) -> ParsedFile {
    let station = root.get("station").and_then(Value::as_str).unwrap_or("UNKNOWN").to_string();
    let mut observations = Vec::new();
    let epochs = match root.get("epochs").and_then(Value::as_array) {
        Some(e) => e.clone(),
        None => {
            diags.push(diag("error", "missing_epochs", "obs_sample requires an 'epochs' array"));
            Vec::new()
        }
    };
    for (epoch_idx, ep) in epochs.iter().enumerate() {
        let tow_s = ep.get("tow_s").and_then(Value::as_f64).unwrap_or(0.0);
        let epoch_text = ep.get("epoch_text").and_then(Value::as_str).unwrap_or("").to_string();
        let epoch_flag = ep.get("flag").and_then(Value::as_str).unwrap_or("0").to_string();
        let sats = match ep.get("sats").and_then(Value::as_object) {
            Some(s) => s.clone(),
            None => {
                diags.push(json_diag_list(ep.clone(), "missing_sats", "epoch entry requires a 'sats' object"));
                continue;
            }
        };
        for (sat, obs) in &sats {
            let obs_map = match obs.as_object() {
                Some(m) => m,
                None => {
                    diags.push(json_diag_list(obs.clone(), "bad_sat_entry", "satellite entry must be an object of obs codes"));
                    continue;
                }
            };
            for (code, ov) in obs_map {
                let (value_raw, value, lli, ssi) = match ov {
                    Value::Object(m) => {
                        let raw = m.get("value").map(|v| match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        }).unwrap_or_default();
                        let val = m.get("value").and_then(|v| v.as_f64());
                        let mut val = val;
                        if val.is_none() && !raw.trim().is_empty() {
                            diags.push(diag("error", "unconvertible_observation",
                                format!("{sat} {code} epoch {epoch_idx}: value '{raw}' not convertible; kept raw")));
                        }
                        if val.is_none() {
                            if let Some(Value::String(s)) = m.get("value") {
                                val = s.trim().parse::<f64>().ok();
                            }
                        }
                        let lli = m.get("lli").and_then(Value::as_u64).map(|v| v as u8);
                        let ssi = m.get("ssi").and_then(Value::as_u64).map(|v| v as u8);
                        (raw, val, lli, ssi)
                    }
                    Value::Number(n) => (n.to_string(), n.as_f64(), None, None),
                    Value::String(s) => (s.clone(), s.trim().parse::<f64>().ok(), None, None),
                    other => {
                        diags.push(json_diag_list(other.clone(), "bad_obs_value", "observation value must be number, string or object"));
                        continue;
                    }
                };
                let class = code.chars().next().unwrap_or('?');
                let (obs_class, unit) = class_unit(class);
                let datum = match class {
                    'C' | 'P' => RANGE_DATUM.to_string(),
                    'L' => format!("full carrier cycle, {TIME_DATUM}"),
                    'D' => format!("instantaneous Doppler, {TIME_DATUM}"),
                    'S' => SNR_DATUM.to_string(),
                    _ => "unknown datum".to_string(),
                };
                let id = crate::ids::id12("obs", &[sat, code, &epoch_text]);
                observations.push(RawObservation {
                    id, raw_id: String::new(), epoch_idx, tow_s,
                    epoch_text: epoch_text.clone(), epoch_flag: epoch_flag.clone(),
                    sat: sat.clone(), code: code.clone(),
                    band: band_of(code), obs_class: obs_class.to_string(),
                    value_raw, value, lli, ssi, unit: unit.to_string(), datum,
                    raw_line: serde_json::to_string(ov).unwrap_or_default(),
                    diagnostics: Vec::new(),
                });
            }
        }
    }
    ParsedFile {
        kind: "obs_sample".into(),
        station,
        observations,
        clock_events: vec![],
        sat_states: vec![],
        diagnostics: std::mem::take(diags),
    }
}

fn parse_clock_events(root: &Value, diags: &mut Vec<Diagnostic>) -> ParsedFile {
    let mut events = Vec::new();
    if let Some(arr) = root.get("events").and_then(Value::as_array) {
        for e in arr {
            let tow_s = e.get("tow_s").and_then(Value::as_f64).unwrap_or(0.0);
            let kind = e.get("kind").and_then(Value::as_str).unwrap_or("receiver_clock_jump").to_string();
            let offset = e.get("offset").cloned().unwrap_or(Value::Null);
            let value = offset.get("value").and_then(Value::as_f64).unwrap_or(0.0);
            let unit = offset.get("unit").and_then(Value::as_str).unwrap_or("s").to_string();
            if unit != "s" {
                diags.push(diag("warning", "unusual_clock_offset_unit",
                    format!("clock event at tow {tow_s} uses unit '{unit}', expected 's'")));
            }
            let source = e.get("source").and_then(Value::as_str).unwrap_or("receiver_log").to_string();
            events.push(ClockEvent {
                tow_s, kind,
                offset: Quantity { value, unit, datum: Some(TIME_DATUM.to_string()) },
                source,
            });
        }
    } else {
        diags.push(diag("error", "missing_events", "clock_events sample requires an 'events' array"));
    }
    ParsedFile {
        kind: "clock_events".into(),
        station: String::new(),
        observations: vec![],
        clock_events: events,
        sat_states: vec![],
        diagnostics: std::mem::take(diags),
    }
}

fn parse_sat_states(root: &Value, diags: &mut Vec<Diagnostic>) -> ParsedFile {
    let mut states = Vec::new();
    if let Some(arr) = root.get("states").and_then(Value::as_array) {
        for s in arr {
            if s.get("tow_s").is_none() || s.get("sat").is_none() {
                diags.push(json_diag_list(s.clone(), "bad_sat_state", "satellite state requires tow_s and sat"));
                continue;
            }
            states.push(s.clone());
        }
    } else {
        diags.push(diag("error", "missing_states", "sat_states sample requires a 'states' array"));
    }
    ParsedFile {
        kind: "sat_states".into(),
        station: String::new(),
        observations: vec![],
        clock_events: vec![],
        sat_states: states,
        diagnostics: std::mem::take(diags),
    }
}

pub fn parse_any(content: &str) -> ParsedFile {
    let t = content.trim_start();
    if t.starts_with('{') {
        parse_json_sample(t)
    } else {
        parse_rinex(content)
    }
}
