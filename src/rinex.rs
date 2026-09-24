//! Mini RINEX 3 observation parser (GPS subset) plus time helpers.
//! Accepts small copy-pasteable samples and reports precise format errors.
//!
//! Supported header labels: RINEX VERSION / TYPE, MARKER NAME,
//! SYS / # / OBS TYPES (single system 'G', up to 13 types on one line),
//! TIME OF FIRST OBS, INTERVAL, APPROX POSITION XYZ, DATUM (comment-style),
//! END OF HEADER. Epoch lines start with '>'; satellite lines are the
//! 3-char sat id followed by 16-char fields (value[14] LLI[1] SSI[1]).

#[derive(Debug, Clone)]
pub struct ParsedValue {
    pub code: String,
    pub value_raw: Option<String>,
    pub lli: Option<u8>,
    pub ssi: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct ParsedSat {
    pub sat: String,
    pub values: Vec<ParsedValue>,
}

#[derive(Debug, Clone)]
pub struct ParsedEpoch {
    pub epoch_ms: i64,
    pub flag: u8,
    pub sats: Vec<ParsedSat>,
}

#[derive(Debug, Clone)]
pub struct ParsedObs {
    pub station: String,
    pub obs_types: Vec<String>,
    pub epochs: Vec<ParsedEpoch>,
    pub approx_position: Option<(f64, f64, f64)>,
    pub datum: Option<String>,
}

/// Days from civil date (Howard Hinnant's algorithm), no external deps.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe
}

/// Milliseconds since Unix epoch for a UTC/GPS calendar reading. We keep the
/// time scale of the source (GPS) as-is: the raw label is preserved and the
/// integer is only the derived-layer ordering key.
pub fn calendar_to_ms(y: i64, mo: i64, d: i64, h: i64, mi: i64, sec: f64) -> i64 {
    let days = days_from_civil(y, mo, d) - days_from_civil(1970, 1, 1);
    ((days * 24 + h) * 3600 + mi * 60) * 1000 + (sec * 1000.0).round() as i64
}

/// Parse an ISO-8601-ish timestamp `YYYY-MM-DDTHH:MM:SS.sss(Z)` to ms.
pub fn iso_to_ms(s: &str) -> Result<i64, String> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s
        .split_once('T')
        .ok_or_else(|| format!("expected 'T' in timestamp '{}'", s))?;
    let dp: Vec<&str> = date.split('-').collect();
    if dp.len() != 3 {
        return Err(format!("bad date '{}'", date));
    }
    let tp: Vec<&str> = time.split(':').collect();
    if tp.len() != 3 {
        return Err(format!("bad time '{}'", time));
    }
    let y: i64 = dp[0].parse().map_err(|_| format!("bad year '{}'", dp[0]))?;
    let mo: i64 = dp[1].parse().map_err(|_| format!("bad month '{}'", dp[1]))?;
    let d: i64 = dp[2].parse().map_err(|_| format!("bad day '{}'", dp[2]))?;
    let h: i64 = tp[0].parse().map_err(|_| format!("bad hour '{}'", tp[0]))?;
    let mi: i64 = tp[1].parse().map_err(|_| format!("bad minute '{}'", tp[1]))?;
    let sec: f64 = tp[2].parse().map_err(|_| format!("bad second '{}'", tp[2]))?;
    Ok(calendar_to_ms(y, mo, d, h, mi, sec))
}

pub fn ms_to_iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (h, rem) = (rem / 3_600_000, rem % 3_600_000);
    let (mi, rem) = (rem / 60_000, rem % 60_000);
    let (s, msec) = (rem / 1000, rem % 1000);
    // civil from days (Hinnant)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, h, mi, s, msec
    )
}

fn label_of(line: &str) -> &str {
    if line.len() >= 60 {
        line[60..].trim()
    } else {
        ""
    }
}

pub fn parse_rinex_obs(text: &str) -> Result<ParsedObs, Vec<String>> {
    let mut errors = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut station = String::new();
    let mut obs_types: Vec<String> = Vec::new();
    let mut approx_position = None;
    let mut datum = None;
    let mut header_end = None;

    for (i, line) in lines.iter().enumerate() {
        let label = label_of(line);
        match label {
            "RINEX VERSION / TYPE" => {
                let ver: f64 = line
                    .get(0..9)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0.0);
                if ver < 3.0 {
                    errors.push(format!(
                        "line {}: only RINEX 3.x observation files are supported (got version {})",
                        i + 1,
                        line.get(0..9).unwrap_or("").trim()
                    ));
                }
            }
            "MARKER NAME" => {
                station = line.get(0..60).unwrap_or(line).trim().to_string();
            }
            "SYS / # / OBS TYPES" => {
                let sys = line.chars().next().unwrap_or(' ');
                if sys != 'G' {
                    errors.push(format!(
                        "line {}: only GPS ('G') obs types are supported, got '{}'",
                        i + 1,
                        sys
                    ));
                }
                let count: usize = line
                    .get(3..6)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                let body = line.get(7..60).unwrap_or("");
                for chunk in body.split_whitespace() {
                    obs_types.push(chunk.to_string());
                }
                if obs_types.len() != count {
                    errors.push(format!(
                        "line {}: SYS / # / OBS TYPES declares {} types but {} were found",
                        i + 1,
                        count,
                        obs_types.len()
                    ));
                }
            }
            "APPROX POSITION XYZ" => {
                let mut nums = line.get(0..42).unwrap_or("").split_whitespace();
                let p: Vec<f64> = nums.by_ref().filter_map(|s| s.parse().ok()).collect();
                if p.len() == 3 {
                    approx_position = Some((p[0], p[1], p[2]));
                }
            }
            "DATUM" => {
                datum = Some(line.get(0..60).unwrap_or(line).trim().to_string());
            }
            "END OF HEADER" => {
                header_end = Some(i);
                break;
            }
            _ => {}
        }
    }

    let Some(hdr) = header_end else {
        errors.push("missing END OF HEADER line".to_string());
        return Err(errors);
    };
    if station.is_empty() {
        errors.push("missing MARKER NAME in header".to_string());
    }
    if obs_types.is_empty() {
        errors.push("missing SYS / # / OBS TYPES in header".to_string());
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut epochs = Vec::new();
    let mut i = hdr + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !line.starts_with('>') {
            errors.push(format!(
                "line {}: expected epoch line starting with '>', got '{}'",
                i + 1,
                line.chars().take(20).collect::<String>()
            ));
            i += 1;
            continue;
        }
        let f: Vec<&str> = line[1..].split_whitespace().collect();
        if f.len() < 8 {
            errors.push(format!(
                "line {}: epoch line needs 'y m d h m s flag nsat', got {} fields",
                i + 1,
                f.len()
            ));
            i += 1;
            continue;
        }
        let nums: Result<Vec<i64>, _> = f[0..5].iter().map(|s| s.parse::<i64>()).collect();
        let Ok(nums) = nums else {
            errors.push(format!("line {}: bad epoch calendar fields", i + 1));
            i += 1;
            continue;
        };
        let sec: f64 = match f[5].parse() {
            Ok(v) => v,
            Err(_) => {
                errors.push(format!("line {}: bad epoch seconds '{}'", i + 1, f[5]));
                i += 1;
                continue;
            }
        };
        let flag: u8 = f[6].parse().unwrap_or(0);
        let nsat: usize = match f[7].parse() {
            Ok(v) => v,
            Err(_) => {
                errors.push(format!("line {}: bad satellite count '{}'", i + 1, f[7]));
                i += 1;
                continue;
            }
        };
        let epoch_ms = calendar_to_ms(nums[0], nums[1], nums[2], nums[3], nums[4], sec);
        let mut sats = Vec::new();
        for k in 0..nsat {
            let li = i + 1 + k;
            let Some(sline) = lines.get(li) else {
                errors.push(format!(
                    "line {}: epoch declares {} satellites but file ends early",
                    i + 1,
                    nsat
                ));
                break;
            };
            if sline.len() < 3 {
                errors.push(format!("line {}: satellite line too short", li + 1));
                continue;
            }
            let sat = sline[0..3].trim().to_string();
            if sat.is_empty() || !sat.starts_with('G') {
                errors.push(format!(
                    "line {}: expected satellite id like 'G05', got '{}'",
                    li + 1,
                    &sline[0..3]
                ));
                continue;
            }
            let body = &sline[3..];
            let mut values = Vec::new();
            for (t, code) in obs_types.iter().enumerate() {
                let start = t * 16;
                let field = body.get(start..start + 16).unwrap_or("");
                let value_txt = field.get(0..14).unwrap_or("").trim();
                let lli = field
                    .get(14..15)
                    .and_then(|c| c.trim().parse::<u8>().ok());
                let ssi = field
                    .get(15..16)
                    .and_then(|c| c.trim().parse::<u8>().ok());
                let value_raw = if value_txt.is_empty() {
                    None
                } else {
                    // Validate numeric now so format errors are precise.
                    if value_txt.parse::<f64>().is_err() {
                        errors.push(format!(
                            "line {}: bad numeric field '{}' for {}",
                            li + 1,
                            value_txt,
                            code
                        ));
                    }
                    Some(value_txt.to_string())
                };
                values.push(ParsedValue {
                    code: code.clone(),
                    value_raw,
                    lli,
                    ssi,
                });
            }
            sats.push(ParsedSat { sat, values });
        }
        epochs.push(ParsedEpoch { epoch_ms, flag, sats });
        i += 1 + nsat;
    }

    if epochs.is_empty() {
        errors.push("no observation epochs found after header".to_string());
    }
    if errors.is_empty() {
        Ok(ParsedObs { station, obs_types, epochs, approx_position, datum })
    } else {
        Err(errors)
    }
}
