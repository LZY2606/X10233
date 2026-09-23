// Parser for REL 1.0, a small RINEX-style local import format.
// See cases/FORMAT_REL.md. Hard format errors abort the import.

use crate::model::*;

fn err(errors: &mut Vec<String>, line_no: usize, msg: impl Into<String>) {
    errors.push(format!("line {}: {}", line_no, msg.into()));
}

fn parse_date_time(tok: &[&str]) -> Option<(String, f64)> {
    // yyyy mm dd hh mm ss
    if tok.len() != 6 {
        return None;
    }
    let nums: Vec<f64> = tok.iter().filter_map(|t| t.parse::<f64>().ok()).collect();
    if nums.len() != 6 {
        return None;
    }
    let day_secs = nums[3] * 3600.0 + nums[4] * 60.0 + nums[5];
    let iso = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:06.3}",
        nums[0] as u32, nums[1] as u32, nums[2] as u32, nums[3] as u32, nums[4] as u32, nums[5]
    );
    Some((iso, day_secs))
}

fn split_token(raw: &str) -> (String, Option<String>) {
    // LLI/strength suffix may be glued to a phase value, e.g. "123.4561".
    let bytes = raw.as_bytes();
    if let Some(pos) = bytes.iter().rposition(|&b| b.is_ascii_digit()) {
        let suffix = &raw[pos + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return (raw[..=pos].to_string(), Some(suffix.to_string()));
        }
    }
    (raw.to_string(), None)
}

pub fn parse_rel(text: &str) -> ParsedCase {
    let mut errors = Vec::new();
    let mut station: Option<StationDef> = None;
    let mut frequencies: Vec<FrequencyDef> = Vec::new();
    let mut notes = Vec::new();
    let mut epochs: Vec<RawEpoch> = Vec::new();
    let mut clock_events = Vec::new();
    let mut sat_states = Vec::new();
    let mut format = String::new();
    let mut epoch_seq: u32 = 0;
    let mut pending_epoch: Option<RawEpoch> = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let head = line.split_whitespace().next().unwrap_or("");
        match head {
            "REL" => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 || parts[1] != "1.0" {
                    err(&mut errors, line_no, "expected 'REL 1.0' header");
                }
                format = "REL/1.0".to_string();
            }
            "NOTE" => notes.push(line[4..].trim().to_string()),
            "STATION" => {
                let p: Vec<&str> = line.split_whitespace().collect();
                if p.len() < 6 {
                    err(&mut errors, line_no, "STATION needs id lat lon height datum");
                } else {
                    let coords: Vec<f64> = p[2..5].iter().filter_map(|t| t.parse().ok()).collect();
                    if coords.len() != 3 {
                        err(&mut errors, line_no, "STATION coordinates must be numeric");
                    } else {
                        station = Some(StationDef {
                            id: p[1].to_string(),
                            lat_deg: coords[0],
                            lon_deg: coords[1],
                            height_m: coords[2],
                            datum: p[5].to_string(),
                        });
                    }
                }
            }
            "FREQ" => {
                let p: Vec<&str> = line.split_whitespace().collect();
                if p.len() < 7 {
                    err(&mut errors, line_no, "FREQ needs idx band value unit system code...");
                } else {
                    let fidx: Result<u32, _> = p[1].parse();
                    let val: Result<f64, _> = p[3].parse();
                    match (fidx, val, p[4]) {
                        (Ok(i), Ok(v), "MHz") => {
                            frequencies.push(FrequencyDef::new(
                                i,
                                p[2].to_string(),
                                v,
                                p[5].to_string(),
                                p[6..].iter().map(|s| s.to_string()).collect(),
                            ));
                        }
                        _ => err(&mut errors, line_no, "invalid FREQ declaration"),
                    }
                }
            }
            "CLOCK" => {
                let p: Vec<&str> = line.split_whitespace().collect();
                if p.len() < 12 {
                    err(&mut errors, line_no, "CLOCK needs date time scope delta unit kind [NOTE ...]");
                } else {
                    match parse_date_time(&p[1..7]) {
                        None => err(&mut errors, line_no, "CLOCK has invalid timestamp"),
                        Some((iso, epoch_s)) => {
                            let (delta_text, delta_m, unit) = parse_delta(p[8], p[9]);
                            clock_events.push(ClockEventLine {
                                iso,
                                epoch_s,
                                scope: p[7].to_string(),
                                delta_text: p[8].to_string(),
                                delta_m,
                                unit,
                                kind: p[10].to_string(),
                                note: note_after(&p, 11),
                                diagnostic: if delta_m.is_none() {
                                    Some(format!("unconvertible clock delta '{}'", p[8]))
                                } else {
                                    None
                                },
                            });
                        }
                    }
                }
            }
            "SATSTATE" => {
                let p: Vec<&str> = line.split_whitespace().collect();
                if p.len() < 10 {
                    err(&mut errors, line_no, "SATSTATE needs sat date time HEALTH EL x AZ y");
                } else {
                    match parse_date_time(&p[2..8]) {
                        None => err(&mut errors, line_no, "SATSTATE has invalid timestamp"),
                        Some((iso, epoch_s)) => {
                            let mut el = None;
                            let mut az = None;
                            for w in p[8..].windows(2) {
                                if w[0] == "EL" {
                                    el = w[1].parse().ok();
                                }
                                if w[0] == "AZ" {
                                    az = w[1].parse().ok();
                                }
                            }
                            sat_states.push(SatStateLine {
                                sat: p[1].to_string(),
                                iso,
                                epoch_s,
                                health: p[8].to_string(),
                                elevation_deg: el,
                                azimuth_deg: az,
                                datum: "topocentric ENU (el/az degrees)".to_string(),
                            });
                        }
                    }
                }
            }
            "*" => {
                let p: Vec<&str> = line.split_whitespace().collect();
                if let Some(pe) = pending_epoch.take() {
                    epochs.push(pe);
                }
                if p.len() < 8 {
                    err(&mut errors, line_no, "epoch header needs date time system nsat");
                    pending_epoch = None;
                } else {
                    match parse_date_time(&p[1..7]) {
                        None => {
                            err(&mut errors, line_no, "invalid epoch timestamp");
                        }
                        Some((iso, epoch_s)) => {
                            epoch_seq += 1;
                            let _: Result<u32, _> = p[7].parse();
                            pending_epoch = Some(RawEpoch {
                                seq: epoch_seq,
                                iso,
                                epoch_s,
                                system: p[7].to_string(),
                                lines: Vec::new(),
                            });
                        }
                    }
                }
            }
            _ => {
                // Satellite observation line (id starts with letter+digits, e.g. G07).
                let p: Vec<&str> = line.split_whitespace().collect();
                match &mut pending_epoch {
                    None => err(&mut errors, line_no, format!("observation line '{}' outside epoch", head)),
                    Some(epoch) => {
                        if !head.len() >= 3
                            || !head.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                            || !head[1..].chars().all(|c| c.is_ascii_digit())
                        {
                            err(&mut errors, line_no, format!("expected satellite id, got '{}'", head));
                        } else {
                            epoch.lines.push(parse_sat_line(&p, &frequencies, line_no, &mut errors));
                        }
                    }
                }
            }
        }
    }
    if let Some(pe) = pending_epoch.take() {
        epochs.push(pe);
    }
    if format.is_empty() {
        errors.push("missing REL 1.0 header".to_string());
    }
    if station.is_none() {
        errors.push("missing STATION declaration".to_string());
    }
    if frequencies.is_empty() {
        errors.push("no FREQ declarations".to_string());
    }

    ParsedCase {
        format,
        station: station.unwrap_or(StationDef {
            id: "UNKNOWN".to_string(),
            lat_deg: 0.0,
            lon_deg: 0.0,
            height_m: 0.0,
            datum: "NONE".to_string(),
        }),
        frequencies,
        notes,
        epochs,
        clock_events,
        sat_states,
        errors,
    }
}

fn note_after(p: &[&str], from: usize) -> String {
    if from < p.len() {
        p[from..].join(" ").trim().to_string()
    } else {
        String::new()
    }
}

fn parse_delta(text: &str, unit: &str) -> (String, Option<f64>, String) {
    let signed = text.trim_start_matches('+');
    let val: Option<f64> = signed.parse().ok();
    (text.to_string(), val, unit.to_string())
}

fn parse_sat_line(
    p: &[&str],
    freqs: &[FrequencyDef],
    line_no: usize,
    errors: &mut Vec<String>,
) -> RawEpochLine {
    let sat = p[0].to_string();
    let mut tokens = Vec::new();
    // Order tokens by declared FREQ code lists.
    let mut expected: Vec<(&str, u32)> = Vec::new();
    for f in freqs {
        for c in &f.codes {
            expected.push((c.as_str(), f.idx));
        }
    }
    for (i, word) in p.iter().enumerate().skip(1) {
        let code = expected.get(i - 1).map(|(c, _)| c.to_string()).unwrap_or_else(|| format!("?{}", i));
        let fidx = expected.get(i - 1).map(|(_, idx)| *idx);
        if word.is_empty() {
            tokens.push(RawToken {
                code,
                freq_idx: fidx,
                original: String::new(),
                ..Default::default()
            });
            continue;
        }
        let (num_text, suffix) = split_token(word);
        let mut diagnostic = None;
        let value: Option<f64> = match num_text.parse::<f64>() {
            Ok(v) if v.is_finite() => Some(v),
            _ => {
                diagnostic = Some(format!("unconvertible token '{}' (line {}); kept in place", word, line_no));
                None
            }
        };
        let unit = ObsUnit::of_code(&code).map(|u| u.label().to_string());
        tokens.push(RawToken {
            code,
            freq_idx: fidx,
            original: word.to_string(),
            value,
            unit,
            lli: suffix,
            diagnostic,
        });
    }
    if tokens.len() != expected.len() {
        err(errors, line_no, format!("sat {} has {} observables, expected {}", sat, tokens.len(), expected.len()));
    }
    RawEpochLine { sat, tokens }
}
