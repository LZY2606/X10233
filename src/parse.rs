//! Text import format (RINEX-inspired, line oriented).
//!
//! ```text
//! # comment
//! STATION ABCD
//! CLOCK 2026-09-19T00:05:00Z receiver_clock_jump
//! E 2026-09-19T00:00:00Z
//! O G05 1 C1C 20223456.789
//! O G05 1 L1C 106345678.234 LLI=1 SNR=48.5
//! O G05 1 D1C -812.34
//! ```
//!
//! Pseudorange, carrier, Doppler, LLI, SNR, clock events, frequency numbers
//! and observation codes are all stored per epoch exactly as submitted.

use crate::model::*;

pub struct ParsedInput {
    pub station: Option<String>,
    pub epochs: Vec<(String, f64, String)>,       // (iso, t_secs, raw_line)
    pub observations: Vec<RawObs>,
    pub clock_events: Vec<(String, f64, String, String)>, // (iso, t, kind, raw_line)
    pub issues: Vec<ParseIssue>,
}

pub struct RawObs {
    pub t: f64,
    pub t_iso: String,
    pub sat: String,
    pub freq_number: i64,
    pub obs_code: String,
    pub raw_value: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub lli: Option<i64>,
    pub snr: Option<f64>,
    pub diag: Option<String>,
    pub raw_line: String,
}

/// Convert `YYYY-MM-DDTHH:MM:SS(.fff)Z` to seconds since 1980-01-06 (GPS-like
/// continuous scale; leap seconds are out of scope for this desk).
pub fn iso_to_secs(iso: &str) -> Option<f64> {
    let b = iso.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year: i64 = iso.get(0..4)?.parse().ok()?;
    let month: i64 = iso.get(5..7)?.parse().ok()?;
    let day: i64 = iso.get(8..10)?.parse().ok()?;
    let hour: i64 = iso.get(11..13)?.parse().ok()?;
    let min: i64 = iso.get(14..16)?.parse().ok()?;
    let sec_part = iso.get(17..)?.trim_end_matches('Z');
    let secs: f64 = sec_part.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || secs >= 61.0 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let base = days_from_civil(1980, 1, 6);
    Some(((days - base) * 86400 + hour * 3600 + min * 60) as f64 + secs)
}

pub fn secs_to_iso(t: f64) -> String {
    let base = days_from_civil(1980, 1, 6);
    let total = t.floor() as i64;
    let days = total.div_euclid(86400) + base;
    let sod = total.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let frac = t - t.floor();
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:06.3}Z",
        sod / 3600,
        (sod % 3600) / 60,
        (sod % 60) as f64 + frac
    )
}

/// Howard Hinnant's civil-date algorithms (public domain formulas).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn parse_input(text: &str) -> ParsedInput {
    let mut out = ParsedInput {
        station: None,
        epochs: Vec::new(),
        observations: Vec::new(),
        clock_events: Vec::new(),
        issues: Vec::new(),
    };
    let mut current: Option<(String, f64)> = None;
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks[0] {
            "STATION" if toks.len() == 2 => out.station = Some(toks[1].to_string()),
            "E" if toks.len() == 2 => match iso_to_secs(toks[1]) {
                Some(t) => {
                    current = Some((toks[1].to_string(), t));
                    out.epochs.push((toks[1].to_string(), t, raw.to_string()));
                }
                None => out.issues.push(ParseIssue {
                    line: line_no,
                    message: format!("invalid epoch timestamp '{}'", toks[1]),
                    raw: raw.to_string(),
                }),
            },
            "CLOCK" if toks.len() == 3 => match iso_to_secs(toks[1]) {
                Some(t) => out
                    .clock_events
                    .push((toks[1].to_string(), t, toks[2].to_string(), raw.to_string())),
                None => out.issues.push(ParseIssue {
                    line: line_no,
                    message: format!("invalid clock-event timestamp '{}'", toks[1]),
                    raw: raw.to_string(),
                }),
            },
            "O" if toks.len() >= 5 => {
                let Some((iso, t)) = current.clone() else {
                    out.issues.push(ParseIssue {
                        line: line_no,
                        message: "observation before any E epoch line".into(),
                        raw: raw.to_string(),
                    });
                    continue;
                };
                let sat = toks[1].to_string();
                let freq_number: i64 = match toks[2].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        out.issues.push(ParseIssue {
                            line: line_no,
                            message: format!("invalid frequency number '{}'", toks[2]),
                            raw: raw.to_string(),
                        });
                        continue;
                    }
                };
                let obs_code = toks[3].to_string();
                let raw_value = toks[4].to_string();
                let unit = unit_for_code(&obs_code).map(|u| u.to_string());
                let (value, diag) = match raw_value.parse::<f64>() {
                    Ok(v) if v.is_finite() => (Some(v), None),
                    _ => (
                        None,
                        Some(format!(
                            "unconvertible value '{raw_value}'; kept in place, excluded from combinations"
                        )),
                    ),
                };
                if unit.is_none() {
                    out.issues.push(ParseIssue {
                        line: line_no,
                        message: format!("unknown observation code '{obs_code}'"),
                        raw: raw.to_string(),
                    });
                }
                let mut lli = None;
                let mut snr = None;
                for attr in &toks[5..] {
                    if let Some(v) = attr.strip_prefix("LLI=") {
                        lli = v.parse::<i64>().ok();
                    } else if let Some(v) = attr.strip_prefix("SNR=") {
                        snr = v.parse::<f64>().ok();
                    } else {
                        out.issues.push(ParseIssue {
                            line: line_no,
                            message: format!("unknown attribute '{attr}'"),
                            raw: raw.to_string(),
                        });
                    }
                }
                out.observations.push(RawObs {
                    t,
                    t_iso: iso,
                    sat,
                    freq_number,
                    obs_code,
                    raw_value,
                    value,
                    unit,
                    lli,
                    snr,
                    diag,
                    raw_line: raw.to_string(),
                });
            }
            _ => out.issues.push(ParseIssue {
                line: line_no,
                message: format!("unrecognized record '{}'", toks[0]),
                raw: raw.to_string(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_roundtrip() {
        let a = iso_to_secs("2026-09-19T00:00:00.000Z").unwrap();
        let b = iso_to_secs("2026-09-19T00:01:40.000Z").unwrap();
        assert_eq!(b - a, 100.0); // unequal-interval friendly continuous scale
        assert_eq!(secs_to_iso(b), "2026-09-19T00:01:40.000Z");
    }
}
