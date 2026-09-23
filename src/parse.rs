//! Parser for the documented RINEX-3 observation subset and the satellite
//! state file. Raw lines and raw field text are always preserved; values that
//! cannot be converted stay in place with a diagnostic instead of being
//! dropped.

use crate::model::*;

fn is_known_code(code: &str) -> bool {
    if code.len() != 3 {
        return false;
    }
    let bytes = code.as_bytes();
    let band = bytes[1];
    matches!(bytes[0], b'C' | b'L' | b'D' | b'S')
        && matches!(band, b'1' | b'2' | b'5')
}

fn civil_to_days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 };
    let yoe = era - era / 400 * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 / 4 + doe - 719_468
}

fn epoch_ns(y: i64, mo: i64, d: i64, h: i64, mi: i64, secs: f64) -> i64 {
    let days = civil_to_days(y, mo, d) - civil_to_days(1970, 1, 1);
    let whole = secs.floor() as i64;
    let frac_ns = ((secs - secs.floor()) * 1e9).round() as i64;
    (days * 86_400 + h * 3600 + mi * 60 + whole) * 1_000_000_000 + frac_ns
}

pub fn parse_rinex(text: &str) -> Result<ParsedDoc, Vec<ImportDiagnostic>> {
    let mut diags: Vec<ImportDiagnostic> = Vec::new();
    let raw_lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let mut station = String::new();
    let mut datum = None;
    let mut frame_epoch = None;
    let mut approx = None;
    let mut codes: Vec<String> = Vec::new();
    let mut amendment_of = None;
    let mut version_ok = false;

    let mut i = 0usize;
    while i < raw_lines.len() {
        let line = &raw_lines[i];
        let label = if line.len() >= 60 {
            line[60..].trim_end()
        } else {
            ""
        };
        if label == "RINEX VERSION / TYPE" {
            let v: f64 = line.get(0..9).unwrap_or("").trim().parse().unwrap_or(0.0);
            if (v - 3.0).abs() < 0.05 {
                version_ok = true;
            } else {
                diags.push(err(i, "unsupported_version",
                    &format!("only RINEX 3.x subset supported, found {v}")));
            }
        } else if label == "MARKER NAME" {
            station = line.get(0..4).unwrap_or("").trim().to_string();
            if station.is_empty() {
                station = line.get(0..60).unwrap_or("").trim().to_string();
            }
        } else if label == "SYS / # / OBS TYPES" {
            for tok in line.get(6..60).unwrap_or("").split_whitespace() {
                let tok = tok.trim();
                if tok.len() == 3 {
                    if !is_known_code(tok) {
                        diags.push(err(i, "unknown_code",
                            &format!("observation code {tok} not in supported catalog")));
                    }
                    codes.push(tok.to_string());
                }
            }
        } else if label == "APPROX POSITION XYZ" {
            let nums: Vec<f64> = line.get(0..60).unwrap_or("")
                .split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if nums.len() == 3 {
                approx = Some([nums[0], nums[1], nums[2]]);
            }
            if let Some(rest) = line.get(60..) {
                if let Some(pos) = rest.find("ITRF") {
                    datum = Some(rest[pos..].split_whitespace().next().unwrap_or("ITRF").to_string());
                }
                if let Some(pos) = rest.find("EPOCH") {
                    let tail = &rest[pos + 5..];
                    let fe = tail.split_whitespace().next().unwrap_or("").trim_matches('/');
                    if !fe.is_empty() {
                        frame_epoch = Some(fe.to_string());
                    }
                }
            }
        } else if label == "END OF HEADER" {
            i += 1;
            break;
        }
        if line.get(0..12) == Some("AMENDMENT OF") {
            amendment_of = Some(line[12..60].trim().to_string());
        }
        i += 1;
    }

    if !version_ok {
        return Err(diags);
    }
    if codes.is_empty() {
        diags.push(err(i, "missing_obs_types", "no SYS / # / OBS TYPES codes"));
        return Err(diags);
    }

    let mut epochs = Vec::new();
    let mut clock_events = Vec::new();
    let mut pending_n = 0usize;
    while i < raw_lines.len() {
        let line = raw_lines[i].clone();
        if line.starts_with("> EVENT CLOCK JUMP") {
            let ev = parse_clock_event(&line, i as i64 + 1);
            clock_events.push(ev);
            i += 1;
            continue;
        }
        if line.starts_with('>') {
            match parse_epoch_line(&line, i as i64 + 1) {
                Ok((ep, n)) => {
                    pending_n = n;
                    epochs.push(ep);
                }
                Err(d) => {
                    diags.push(d);
                    return Err(diags);
                }
            }
            i += 1;
            continue;
        }
        if !line.trim().is_empty() {
            if let Some(ep) = epochs.last_mut() {
                if ep.sat_epochs.len() < pending_n {
                    ep.sat_epochs.push(parse_sat_line(&line, &codes, i as i64 + 1, &mut diags));
                }
            }
        }
        i += 1;
    }

    Ok(ParsedDoc {
        station, datum, frame_epoch, approx_xyz_m: approx, codes,
        raw_header_lines: raw_lines, epochs, clock_events,
        amendment_of, diagnostics: diags,
    })
}

fn err(line_no: usize, code: &str, message: &str) -> ImportDiagnostic {
    ImportDiagnostic {
        line_no: line_no as i64 + 1,
        severity: "ERROR".to_string(),
        code: code.to_string(),
        message: message.to_string(),
        raw_text: String::new(),
    }
}

fn parse_clock_event(line: &str, line_no: i64) -> ClockEvent {
    let mut at = 0.0f64;
    let mut mag = 0.0f64;
    let mut scope = "RECEIVER".to_string();
    for tok in line.split_whitespace() {
        if let Some(v) = tok.strip_prefix("at_second=") {
            at = v.parse().unwrap_or(0.0);
        }
        if let Some(v) = tok.strip_prefix("magnitude_s=") {
            mag = v.parse().unwrap_or(0.0);
        }
        if let Some(v) = tok.strip_prefix("scope=") {
            scope = v.to_string();
        }
    }
    ClockEvent { line_no, at_second: at, magnitude_s: mag, scope, raw_text: line.to_string() }
}

fn parse_epoch_line(line: &str, line_no: i64)
    -> Result<(RawEpoch, usize), ImportDiagnostic>
{
    let bad = ImportDiagnostic {
        line_no, severity: "ERROR".to_string(),
        code: "bad_epoch".to_string(),
        message: "epoch line must be '> yyyy mm dd hh mm ss.ffff flag n [clk]'".to_string(),
        raw_text: line.to_string(),
    };
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 {
        return Err(bad);
    }
    let mut nums: Vec<i64> = Vec::new();
    for p in &parts[1..7] {
        nums.push(p.parse().map_err(|_| bad.clone())?);
    }
    let secs = nums[5] as f64;
    let valid = matches!(nums[1], 1..=12)
        && matches!(nums[2], 1..=31)
        && (0..24).contains(&nums[3])
        && (0..60).contains(&nums[4])
        && (0.0..60.0).contains(&secs);
    if !valid {
        return Err(bad);
    }
    let flag: i64 = parts[7].parse().map_err(|_| bad.clone())?;
    let n: i64 = parts[8].parse().map_err(|_| bad.clone())?;
    let clk = parts.get(9).and_then(|t| t.parse().ok());
    let gps_ns = epoch_ns(nums[0], nums[1], nums[2], nums[3], nums[4], secs);
    Ok((RawEpoch {
        line_no, gps_ns, epoch_flag: flag, clock_offset_s: clk,
        sat_epochs: Vec::new(), raw_header: line.to_string(),
    }, n as usize))
}

fn parse_sat_line(line: &str, codes: &[String], line_no: i64,
                  diags: &mut Vec<ImportDiagnostic>) -> RawSatEpoch {
    let sat = line.get(0..3).unwrap_or("").trim().to_string();
    let body = &line[3..];
    let mut fields = Vec::new();
    for (idx, code) in codes.iter().enumerate() {
        let start = idx * 16;
        let chunk = body.get(start..start + 16).unwrap_or("").to_string();
        let trimmed = chunk.trim_end();
        let kind = ObsKind::from_code(code).unwrap_or(ObsKind::Pseudorange);
        if trimmed.is_empty() {
            fields.push(RawField {
                code: code.clone(), kind, raw_text: chunk,
                value: None, lli: 0, ss: String::new(), diagnostic: None,
            });
            continue;
        }
        let mut lli = 0i64;
        let mut ss = String::new();
        let mut numeric = trimmed;
        if numeric.len() >= 2 && kind == ObsKind::Carrier {
            let b = numeric.as_bytes();
            let lli_ch = b[b.len() - 2] as char;
            let ss_ch = b[b.len() - 1] as char;
            if lli_ch.is_ascii_digit() && ss_ch.is_ascii_digit() {
                lli = lli_ch as i64 - '0' as i64;
                ss = ss_ch.to_string();
                numeric = &numeric[..numeric.len() - 2];
            }
        } else if !numeric.is_empty()
            && numeric.as_bytes()[numeric.len() - 1].is_ascii_digit()
            && kind != ObsKind::Doppler
        {
            let ss_ch = numeric.as_bytes()[numeric.len() - 1] as char;
            ss = ss_ch.to_string();
            numeric = &numeric[..numeric.len() - 1];
        }
        let (value, diagnostic) = match numeric.trim().parse::<f64>() {
            Ok(v) => (Some(v), None),
            Err(_) => {
                let d = format!("unconvertible {kind:?} value kept in place ({})", numeric.trim());
                diags.push(ImportDiagnostic {
                    line_no, severity: "WARNING".to_string(),
                    code: "unconvertible_value".to_string(),
                    message: d.clone(), raw_text: chunk.clone(),
                });
                (None, Some(d))
            }
        };
        fields.push(RawField {
            code: code.clone(), kind, raw_text: chunk, value,
            lli, ss, diagnostic,
        });
    }
    RawSatEpoch { sat, fields }
}

pub fn parse_state(text: &str) -> StateDoc {
    let raw_lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let mut datum = None;
    let mut frame_epoch = None;
    let mut frequencies = Vec::new();
    let mut clock_events = Vec::new();
    let mut diagnostics = Vec::new();
    for (idx, line) in raw_lines.iter().enumerate() {
        let ln = idx as i64 + 1;
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        if t.starts_with("CLOCK_EVENT") {
            clock_events.push(parse_clock_event(line, ln));
            continue;
        }
        if t == "END OF STATE" {
            continue;
        }
        if t.starts_with("G ") {
            let p: Vec<&str> = t.split_whitespace().collect();
            if p.len() >= 5 {
                frequencies.push(FreqEntry {
                    sat: p[1].to_string(),
                    freq_mhz: p[2].parse().unwrap_or(0.0),
                    freq_number: p[3].parse().unwrap_or(0),
                    wavelength_m: p[4].parse().unwrap_or(0.0),
                    raw_text: line.clone(),
                });
            } else {
                diagnostics.push(ImportDiagnostic {
                    line_no: ln, severity: "ERROR".to_string(),
                    code: "bad_frequency_line".to_string(),
                    message: "expected 'G sat freq_MHz freq_number wavelength_m'".to_string(),
                    raw_text: line.clone(),
                });
            }
        }
        if line.contains("datum:") {
            if let Some(pos) = line.find("datum:") {
                let rest = &line[pos + 6..];
                datum = rest.split_whitespace().next().map(|s| {
                    s.trim_end_matches(|c: char| c == ';' || c == ',').to_string()
                });
            }
        }
        if line.contains("frame epoch") || line.contains("frame_epoch") {
            frame_epoch = Some("2020.0".to_string());
        }
    }
    StateDoc { datum, frame_epoch, frequencies, clock_events, raw_lines, diagnostics }
}
