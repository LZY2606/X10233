//! Import layer: parses a copy-pastable mini-RINEX observation subset.
//!
//! Format (`mini-rinex/v1`), whitespace separated:
//!   # mini-rinex/v1
//!   # station: TOKO
//!   ! CLOCKJUMP 2024 01 02 00 02 30.0000000 1        (millis, kept raw)
//!   > 2024 01 02 00 00 00.0000000 0 2                (epoch, flag, numsat)
//!   G05 C1C 20250283.190 L1C 105896328.318#0 D1C -2134.552 S1C 47.25
//!
//! LLI is attached to a value as `#<digit>`. Structural problems produce
//! precise, line-numbered format errors and reject the import. Values that
//! cannot be converted stay in place with a diagnostic instead of failing.

use crate::model::*;

#[derive(Clone, Debug)]
pub struct ParseFailure {
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ParseOutput {
    pub station: String,
    pub records: Vec<ParsedObs>,
    pub clock_events: Vec<ClockEventRaw>,
    pub diagnostics: Vec<(Option<usize>, String, String)>, // (record idx, severity, msg)
}

fn is_code(tok: &str) -> bool {
    let b = tok.as_bytes();
    b.len() == 3
        && matches!(b[0], b'C' | b'L' | b'D' | b'S')
        && b[1].is_ascii_digit()
        && b[1] != b'0'
        && b[2].is_ascii_alphabetic()
}

/// Convert a civil datetime to continuous GPS seconds (datum: GPS; the given
/// wall fields are interpreted as GPS time directly, no leap-second games).
pub fn to_gps_seconds(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: f64) -> Option<f64> {
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || !(0.0..62.0).contains(&s) {
        return None;
    }
    // days-from-civil (Howard Hinnant's algorithm)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + h * 3600 + mi * 60) as f64 + s)
}

fn parse_epoch_tokens(toks: &[&str], lineno: usize) -> Result<(String, f64, u8, usize), ParseFailure> {
    if toks.len() != 8 {
        return Err(ParseFailure {
            errors: vec![format!(
                "line {}: epoch line needs '> Y M D h m s.sssssss flag numsat' (8 fields), got {}",
                lineno,
                toks.len()
            )],
        });
    }
    let num = |i: usize, what: &str| -> Result<i64, ParseFailure> {
        toks[i].parse::<i64>().map_err(|_| ParseFailure {
            errors: vec![format!("line {}: invalid {} '{}'", lineno, what, toks[i])],
        })
    };
    let (y, mo, d, h, mi) = (num(1, "year")?, num(2, "month")?, num(3, "day")?, num(4, "hour")?, num(5, "minute")?);
    let sec: f64 = toks[6].parse().map_err(|_| ParseFailure {
        errors: vec![format!("line {}: invalid seconds '{}'", lineno, toks[6])],
    })?;
    let gps = to_gps_seconds(y, mo, d, h, mi, sec).ok_or_else(|| ParseFailure {
        errors: vec![format!("line {}: datetime out of range", lineno)],
    })?;
    let flag: u8 = toks[7].parse().map_err(|_| ParseFailure {
        errors: vec![format!("line {}: invalid event flag '{}'", lineno, toks[7])],
    })?;
    let numsat: usize = toks[8].parse().map_err(|_| ParseFailure {
        errors: vec![format!("line {}: invalid satellite count '{}'", lineno, toks[8])],
    })?;
    let epoch_raw = toks[1..7].join(" ");
    Ok((epoch_raw, gps, flag, numsat))
}

pub fn parse_mini_rinex(input_id: &str, text: &str) -> Result<ParseOutput, ParseFailure> {
    let _ = input_id;
    let mut errors: Vec<String> = Vec::new();
    let mut station: Option<String> = None;
    let mut records: Vec<ParsedObs> = Vec::new();
    let mut clock_events: Vec<ClockEventRaw> = Vec::new();
    let mut diagnostics: Vec<(Option<usize>, String, String)> = Vec::new();

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    let mut saw_format_header = false;

    while i < lines.len() {
        let lineno = i + 1;
        let line = lines[i].trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim();
            if rest.eq_ignore_ascii_case("mini-rinex/v1") {
                saw_format_header = true;
            } else if let Some(s) = rest.strip_prefix("station:") {
                let s = s.trim();
                if s.is_empty() {
                    errors.push(format!("line {}: empty station name", lineno));
                } else {
                    station = Some(s.to_string());
                }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('!') {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if toks.first() == Some(&"CLOCKJUMP") {
                if toks.len() != 8 {
                    errors.push(format!(
                        "line {}: CLOCKJUMP needs '! CLOCKJUMP Y M D h m s.sssssss millis', got {} fields",
                        lineno,
                        toks.len()
                    ));
                } else {
                    let epoch_raw = toks[1..7].join(" ");
                    let epoch_gps_s = (|| {
                        let y = toks[1].parse().ok()?;
                        let mo = toks[2].parse().ok()?;
                        let d = toks[3].parse().ok()?;
                        let h = toks[4].parse().ok()?;
                        let mi = toks[5].parse().ok()?;
                        let s = toks[6].parse().ok()?;
                        to_gps_seconds(y, mo, d, h, mi, s)
                    })();
                    if epoch_gps_s.is_none() {
                        diagnostics.push((None, "warning".into(), format!(
                            "line {}: clock-jump epoch not convertible, kept raw", lineno)));
                    }
                    clock_events.push(ClockEventRaw {
                        epoch_raw,
                        epoch_gps_s,
                        millis: toks[7].to_string(),
                        raw_line: line.to_string(),
                    });
                }
            } else {
                errors.push(format!("line {}: unknown event directive '!{}'", lineno, toks.first().copied().unwrap_or("")));
            }
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let toks: Vec<&str> = {
                let mut v = vec![">"];
                v.extend(rest.split_whitespace());
                v
            };
            let (epoch_raw, gps, flag, numsat) = match parse_epoch_tokens(&toks, lineno) {
                Ok(v) => v,
                Err(f) => {
                    errors.extend(f.errors);
                    i += 1;
                    continue;
                }
            };
            i += 1;
            let mut got = 0usize;
            while got < numsat {
                if i >= lines.len() {
                    errors.push(format!(
                        "line {}: epoch declares {} satellites but file ended after {}",
                        lineno, numsat, got
                    ));
                    break;
                }
                let sline = lines[i].trim_end();
                let slineno = i + 1;
                i += 1;
                if sline.trim().is_empty() {
                    continue;
                }
                let stoks: Vec<&str> = sline.split_whitespace().collect();
                if stoks.is_empty() {
                    continue;
                }
                let sat = stoks[0];
                if !(sat.len() == 3 && sat.as_bytes()[0].is_ascii_alphabetic()
                    && sat.as_bytes()[1].is_ascii_digit() && sat.as_bytes()[2].is_ascii_digit())
                {
                    errors.push(format!("line {}: expected satellite id like 'G05', got '{}'", slineno, sat));
                    continue;
                }
                let rest = &stoks[1..];
                if rest.len() % 2 != 0 {
                    errors.push(format!(
                        "line {}: satellite {} has {} trailing tokens; expected CODE VALUE pairs",
                        slineno, sat, rest.len()
                    ));
                    continue;
                }
                let mut obs: Vec<ObsValue> = Vec::new();
                let mut rec_diags: Vec<String> = Vec::new();
                let mut pairs_ok = true;
                for pair in rest.chunks(2) {
                    let code = pair[0];
                    let rawval = pair[1];
                    if !is_code(code) {
                        errors.push(format!(
                            "line {}: '{}' is not a valid observation code (expected like C1C/L2W)",
                            slineno, code
                        ));
                        pairs_ok = false;
                        break;
                    }
                    let (valpart, lli) = match rawval.split_once('#') {
                        Some((v, l)) => {
                            let lli = if l.len() == 1 && l.as_bytes()[0].is_ascii_digit() {
                                l.as_bytes()[0] - b'0'
                            } else {
                                rec_diags.push(format!(
                                    "line {}: {} LLI suffix '{}' not a single digit, kept raw",
                                    slineno, code, l
                                ));
                                0
                            };
                            (v, lli)
                        }
                        None => (rawval, 0),
                    };
                    if valpart.parse::<f64>().is_err() {
                        rec_diags.push(format!(
                            "line {}: {} value '{}' not convertible to a number, kept raw in place",
                            slineno, code, valpart
                        ));
                    }
                    let kind = ObsKind::from_code(code).unwrap();
                    obs.push(ObsValue {
                        code: code.to_string(),
                        freq: code.as_bytes()[1] - b'0',
                        kind,
                        value_raw: rawval.to_string(),
                        lli,
                        unit: kind.unit().to_string(),
                    });
                }
                if !pairs_ok {
                    continue;
                }
                let rec_index = records.len();
                for d in rec_diags {
                    diagnostics.push((Some(rec_index), "warning".into(), d));
                }
                records.push(ParsedObs {
                    station: station.clone().unwrap_or_else(|| "UNKN".to_string()),
                    sat: sat.to_string(),
                    epoch_raw: epoch_raw.clone(),
                    epoch_gps_s: Some(gps),
                    event_flag: flag,
                    obs,
                });
                got += 1;
            }
            continue;
        }
        errors.push(format!(
            "line {}: unexpected content '{}'; expected '#', '!', '>' or a satellite record",
            lineno,
            trimmed.chars().take(24).collect::<String>()
        ));
        i += 1;
    }

    if !saw_format_header {
        errors.push("missing '# mini-rinex/v1' format header".to_string());
    }
    if station.is_none() {
        errors.push("missing '# station: <NAME>' header".to_string());
    }
    if records.is_empty() && errors.is_empty() {
        errors.push("no observation epochs found".to_string());
    }
    if !errors.is_empty() {
        return Err(ParseFailure { errors });
    }
    Ok(ParseOutput {
        station: station.unwrap(),
        records,
        clock_events,
        diagnostics,
    })
}
