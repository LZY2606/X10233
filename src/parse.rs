//! Import parsers. Raw representation is preserved verbatim; normalization
//! happens only in the derived layer. Unconvertible values stay in place with
//! a diagnostic instead of being dropped.

use crate::model::*;
use crate::timeutil;

#[derive(Debug)]
pub struct ParseFailure {
    pub errors: Vec<String>,
}

pub struct ParsedObs {
    pub values: Vec<ObsValue>,
    pub diagnostics: Vec<(String, String, String)>, // severity, message, raw_fragment
}

fn band_of(code: &str) -> Option<String> {
    let b = code.as_bytes();
    if b.len() >= 2 && b[0] == b'L' {
        match b[1] {
            b'1' => return Some("L1".into()),
            b'2' => return Some("L2".into()),
            b'5' => return Some("L5".into()),
            _ => {}
        }
    }
    None
}

/// Parse a RINEX 3.x observation subset: header obs-type table, optional
/// GLONASS frequency-channel table, `>` epoch records, fixed-width sat lines.
pub fn parse_rinex_obs(source_id: &str, content: &str) -> Result<ParsedObs, ParseFailure> {
    let lines: Vec<&str> = content.lines().collect();
    let mut errors = Vec::new();
    let mut obs_types: Vec<(char, Vec<String>)> = Vec::new(); // sys -> codes
    let mut glo_channels: Vec<(String, i32)> = Vec::new();
    let mut header_end = None;

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let label = if line.len() > 60 { line[60..].trim().to_string() } else { String::new() };
        if label == "END OF HEADER" {
            header_end = Some(i);
            break;
        }
        match label.as_str() {
            "RINEX VERSION / TYPE" => {
                let ver: f64 = line.get(0..9).and_then(|s| s.trim().parse().ok()).unwrap_or(0.0);
                if ver < 3.0 {
                    errors.push(format!("line {}: only RINEX 3.x observation files are supported (got version {})", i + 1, ver));
                }
                let typ = line.get(20..21).unwrap_or(" ");
                if typ != "O" {
                    errors.push(format!("line {}: expected observation file type 'O', got '{}'", i + 1, typ));
                }
            }
            "SYS / # / OBS TYPES" => {
                let sys = line.chars().next().unwrap_or(' ');
                if !matches!(sys, 'G' | 'R' | 'E' | 'C' | 'J' | 'S' | 'I') {
                    errors.push(format!("line {}: unknown satellite system '{}'", i + 1, sys));
                }
                let n: usize = line.get(3..6).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                let mut codes = Vec::new();
                let mut j = i;
                loop {
                    let l = lines[j];
                    for k in 0..13 {
                        let a = 7 + k * 4;
                        if let Some(tok) = l.get(a..a + 3) {
                            let tok = tok.trim();
                            if !tok.is_empty() {
                                codes.push(tok.to_string());
                            }
                        }
                    }
                    if codes.len() >= n {
                        break;
                    }
                    j += 1;
                    if j >= lines.len() || !lines[j].ends_with("SYS / # / OBS TYPES") {
                        errors.push(format!("line {}: obs-type list truncated (expected {} types)", i + 1, n));
                        break;
                    }
                }
                codes.truncate(n);
                for c in &codes {
                    if ObsKind::from_code(c).is_none() {
                        errors.push(format!("line {}: unsupported observation code '{}'", i + 1, c));
                    }
                }
                i = j;
                obs_types.push((sys, codes));
            }
            "GLONASS SLOT / FRQ #" => {
                let mut j = i;
                loop {
                    let l = lines[j];
                    for k in 0..8 {
                        let a = 4 + k * 7;
                        if let Some(f) = l.get(a..a + 6) {
                            let f = f.trim();
                            if f.is_empty() {
                                continue;
                            }
                            let sat = &f[0..3.min(f.len())];
                            let ch = f[3..].trim().parse::<i32>();
                            match ch {
                                Ok(c) => glo_channels.push((sat.to_string(), c)),
                                Err(_) => errors.push(format!("line {}: bad GLONASS channel entry '{}'", j + 1, f)),
                            }
                        }
                    }
                    j += 1;
                    if j >= lines.len() || !lines[j].ends_with("GLONASS SLOT / FRQ #") {
                        break;
                    }
                }
                i = j.saturating_sub(1);
            }
            _ => {}
        }
        i += 1;
    }

    let header_end = match header_end {
        Some(h) => h,
        None => {
            errors.push("missing END OF HEADER line".to_string());
            return Err(ParseFailure { errors });
        }
    };
    if obs_types.is_empty() {
        errors.push("header has no SYS / # / OBS TYPES record".to_string());
    }
    if !errors.is_empty() {
        return Err(ParseFailure { errors });
    }

    let mut values = Vec::new();
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut i = header_end + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !line.starts_with('>') {
            return Err(ParseFailure {
                errors: vec![format!("line {}: expected epoch record starting with '>', got: {}", i + 1, line.trim())],
            });
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 9 {
            return Err(ParseFailure {
                errors: vec![format!("line {}: malformed epoch record (need > y m d h m s flag nsat): {}", i + 1, line)],
            });
        }
        let (y, mo, d, h, mi) = match (toks[1].parse(), toks[2].parse(), toks[3].parse(), toks[4].parse(), toks[5].parse()) {
            (Ok(a), Ok(b), Ok(c), Ok(dd), Ok(e)) => (a, b, c, dd, e),
            _ => return Err(ParseFailure { errors: vec![format!("line {}: bad epoch calendar fields: {}", i + 1, line)] }),
        };
        let sec: f64 = match toks[6].parse() {
            Ok(s) => s,
            Err(_) => return Err(ParseFailure { errors: vec![format!("line {}: bad epoch seconds: {}", i + 1, line)] }),
        };
        let flag: u32 = toks[7].parse().unwrap_or(9);
        let nsat: usize = match toks[8].parse() {
            Ok(n) => n,
            Err(_) => return Err(ParseFailure { errors: vec![format!("line {}: bad satellite count: {}", i + 1, line)] }),
        };
        let epoch_ms = timeutil::civil_to_ms(y, mo, d, h, mi, sec);
        let epoch_raw = format!("{} {} {} {} {} {}", toks[1], toks[2], toks[3], toks[4], toks[5], toks[6]);
        if flag > 1 {
            diags.push(("warning".into(), format!("epoch flag {} (special event) kept as-is", flag), line.to_string()));
        }
        i += 1;
        for _ in 0..nsat {
            if i >= lines.len() {
                return Err(ParseFailure { errors: vec![format!("unexpected end of file inside epoch {}", epoch_raw)] });
            }
            let l = lines[i];
            if l.len() < 3 {
                return Err(ParseFailure { errors: vec![format!("line {}: truncated satellite record: {}", i + 1, l)] });
            }
            let sat = l[0..3].trim().to_string();
            let sys = sat.chars().next().unwrap_or(' ');
            let codes = obs_types.iter().find(|(s, _)| *s == sys).map(|(_, c)| c.clone()).unwrap_or_default();
            if codes.is_empty() {
                return Err(ParseFailure { errors: vec![format!("line {}: no obs-type table for system of satellite {}", i + 1, sat)] });
            }
            let channel = if sys == 'R' {
                glo_channels.iter().find(|(s, _)| *s == sat).map(|(_, c)| *c)
            } else {
                None
            };
            let fields = &l[3..];
            for (k, code) in codes.iter().enumerate() {
                let a = k * 16;
                let field = fields.get(a..(a + 16).min(fields.len())).unwrap_or("");
                let vraw = field.get(0..14).unwrap_or("").trim().to_string();
                let lli = field.get(14..15).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                let ssi = field.get(15..16).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                let kind = ObsKind::from_code(code).unwrap();
                let (value, diag) = if vraw.is_empty() {
                    (None, Some(("info".to_string(), format!("missing {} value for {} at {}", code, sat, epoch_raw), l.to_string())))
                } else {
                    match vraw.parse::<f64>() {
                        Ok(v) => (Some(v), None),
                        Err(_) => (None, Some(("warning".to_string(), format!("unconvertible {} value '{}' kept in place", code, vraw), l.to_string()))),
                    }
                };
                if let Some((sev, msg, frag)) = diag {
                    diags.push((sev, msg, frag));
                }
                let id = stable_id("obs", &[source_id, &sat, &epoch_ms.to_string(), code]);
                values.push(ObsValue {
                    id,
                    source_id: source_id.to_string(),
                    station: String::new(), // filled by engine from header/marker
                    sat: sat.clone(),
                    epoch_ms,
                    epoch_raw: epoch_raw.clone(),
                    code: code.clone(),
                    band: band_of(code),
                    channel,
                    kind,
                    value,
                    value_raw: vraw,
                    unit: kind.unit().to_string(),
                    lli,
                    ssi,
                    raw_line: l.to_string(),
                });
            }
            i += 1;
        }
    }
    Ok(ParsedObs { values, diagnostics: diags })
}

/// Marker name from `MARKER NAME` header line, else fallback.
pub fn rinex_marker_name(content: &str) -> Option<String> {
    for line in content.lines() {
        if line.get(60..).map(|s| s.trim()) == Some("MARKER NAME") {
            let name = line.get(0..60).unwrap_or("").trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Clock-jump event file, one event per line:
/// `CLOCKJUMP <STATION> <YYYY-MM-DDTHH:MM:SS.sssZ> <jump_ms>`
pub fn parse_clock_events(source_id: &str, content: &str) -> Result<Vec<ClockEvent>, ParseFailure> {
    let mut out = Vec::new();
    for (n, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() != 4 || toks[0] != "CLOCKJUMP" {
            return Err(ParseFailure {
                errors: vec![format!("line {}: expected 'CLOCKJUMP <station> <iso8601> <jump_ms>', got: {}", n + 1, line)],
            });
        }
        let epoch_ms = timeutil::iso_to_ms(toks[2]).ok_or_else(|| ParseFailure {
            errors: vec![format!("line {}: bad ISO-8601 epoch '{}'", n + 1, toks[2])],
        })?;
        let jump_ms: f64 = toks[3].parse().map_err(|_| ParseFailure {
            errors: vec![format!("line {}: bad jump value '{}'", n + 1, toks[3])],
        })?;
        out.push(ClockEvent {
            id: stable_id("clk", &[source_id, toks[1], &epoch_ms.to_string()]),
            source_id: source_id.to_string(),
            station: toks[1].to_string(),
            epoch_ms,
            epoch_raw: toks[2].to_string(),
            jump_ms,
            raw_line: line.to_string(),
        });
    }
    Ok(out)
}

/// Minimal SP3-c subset: `#`-header (datum token), `*` epoch lines, `P` sat lines.
pub fn parse_sat_states(source_id: &str, content: &str) -> Result<(Vec<SatState>, Vec<(String, String, String)>), ParseFailure> {
    let mut datum = String::new();
    let mut epoch_ms: Option<i64> = None;
    let mut epoch_raw = String::new();
    let mut out = Vec::new();
    let mut diags = Vec::new();
    for (n, line) in content.lines().enumerate() {
        if line.starts_with('#') {
            for tok in line.split_whitespace() {
                if tok.starts_with("IGS") || tok.starts_with("ITRF") || tok.starts_with("GTRF") {
                    datum = tok.to_string();
                }
            }
            continue;
        }
        if line.starts_with('*') {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 7 {
                return Err(ParseFailure { errors: vec![format!("line {}: malformed SP3 epoch: {}", n + 1, line)] });
            }
            let (y, mo, d, h, mi): (i64, u32, u32, u32, u32) = match (
                toks[1].parse(), toks[2].parse(), toks[3].parse(), toks[4].parse(), toks[5].parse(),
            ) {
                (Ok(a), Ok(b), Ok(c), Ok(dd), Ok(e)) => (a, b, c, dd, e),
                _ => return Err(ParseFailure { errors: vec![format!("line {}: bad SP3 epoch fields", n + 1)] }),
            };
            let sec: f64 = toks[6].parse().map_err(|_| ParseFailure { errors: vec![format!("line {}: bad SP3 epoch seconds", n + 1)] })?;
            epoch_ms = Some(timeutil::civil_to_ms(y, mo, d, h, mi, sec));
            epoch_raw = toks[1..].join(" ");
            continue;
        }
        if line.starts_with('P') {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 5 {
                return Err(ParseFailure { errors: vec![format!("line {}: malformed SP3 position record: {}", n + 1, line)] });
            }
            let ep = epoch_ms.ok_or_else(|| ParseFailure { errors: vec![format!("line {}: position record before any epoch", n + 1)] })?;
            let sat = toks[0].trim_start_matches('P').to_string();
            let nums: Result<Vec<f64>, _> = toks[1..5].iter().map(|t| t.parse::<f64>()).collect();
            let nums = match nums {
                Ok(v) => v,
                Err(_) => {
                    diags.push(("warning".into(), format!("unconvertible SP3 value for {} kept as diagnostic", sat), line.to_string()));
                    continue;
                }
            };
            out.push(SatState {
                id: stable_id("sp3", &[source_id, &sat, &ep.to_string()]),
                source_id: source_id.to_string(),
                sat,
                epoch_ms: ep,
                epoch_raw: epoch_raw.clone(),
                x_km: nums[0],
                y_km: nums[1],
                z_km: nums[2],
                clock_us: nums[3],
                datum: if datum.is_empty() { "unknown".into() } else { datum.clone() },
                raw_line: line.to_string(),
            });
        }
    }
    if datum.is_empty() {
        diags.push(("warning".into(), "no coordinate datum token found in SP3 header; datum recorded as 'unknown'".into(), String::new()));
    }
    Ok((out, diags))
}
