//! RINEX 3 observation / navigation importer plus a clock-event ledger.
//! Hard format errors are collected as structured diagnostics; individual fields
//! that cannot be converted are preserved raw with a diagnostic attached.

use crate::qty::RawField;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct RawEpoch {
    pub time: String,           // normalized "YYYY-MM-DDTHH:MM:SS" (UTC, RINEX civil)
    pub time_raw: String,
    pub flag: String,
    pub clock_offset_raw: Option<String>,
    pub clock_offset: Option<f64>,
    pub lines: Vec<RawSatLine>,
}

#[derive(Debug, Clone)]
pub struct RawSatLine {
    pub sat: String,
    pub code: String,           // "C1C", "L2W", ...
    pub field: RawField,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavRecord {
    pub sat: String,
    pub time: String,
    pub time_raw: String,
    pub kind: String,           // "gps_nav" | "glo_nav"
    pub frequency_number: Option<i32>,
    pub values: Vec<RawField>,  // raw field + numeric value with unit metadata added upstream
}

#[derive(Debug, Clone, Serialize)]
pub struct ClockEventRaw {
    pub time: String,
    pub time_raw: String,
    pub source: String,
    pub magnitude_raw: String,
    pub magnitude: Option<f64>,
    pub unit: String,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ObsFile {
    pub station: String,
    pub marker_name: String,
    pub receiver: String,
    pub antenna: String,
    pub approx_pos: Option<(f64, f64, f64)>,
    pub approx_pos_unit: String,
    pub sys_order: Vec<String>,
    pub obs_types: Vec<(String, String)>, // (system char, code)
    pub epochs: Vec<RawEpoch>,
    pub diagnostics: Vec<ParseDiag>,
    pub raw_header: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParseDiag {
    pub line_no: usize,
    pub severity: String, // "error" | "warning"
    pub code: String,
    pub message: String,
    pub raw: String,
}

fn d(line_no: usize, severity: &str, code: &str, message: String, raw: &str) -> ParseDiag {
    ParseDiag { line_no, severity: severity.into(), code: code.into(), message, raw: raw.to_string() }
}

fn float_field(raw: &str, line_no: usize, code: &str, diags: &mut Vec<ParseDiag>) -> RawField {
    let tok = raw.trim_end();
    let (num, lli, ssi) = split_field(tok);
    match num.parse::<f64>() {
        Ok(v) if v.is_finite() => RawField {
            raw: tok.to_string(),
            v: Some(v),
            u: None,
            lli,
            ssi,
            diagnostic: None,
        },
        _ => {
            diags.push(d(
                line_no,
                "warning",
                code,
                format!("field not convertible to number: {:?}; kept at original position", tok),
                tok,
            ));
            RawField { raw: tok.to_string(), v: None, u: None, lli, ssi,
                       diagnostic: Some("unconvertible".into()) }
        }
    }
}

fn split_field(tok: &str) -> (String, Option<String>, Option<String>) {
    // RINEX 3 field: 14 chars numeric, position 15 = LLI, position 16 = SSI.
    if tok.len() > 14 {
        let (num, rest) = tok.split_at(14);
        let mut ch = rest.chars();
        let a = ch.next().map(|c| c.to_string());
        let b = ch.next().map(|c| c.to_string());
        let lli = a.filter(|s| s != " ");
        let ssi = b.filter(|s| s != " ");
        (num.to_string(), lli, ssi)
    } else {
        (tok.to_string(), None, None)
    }
}

pub fn parse_obs(text: &str) -> Result<ObsFile, Vec<ParseDiag>> {
    let mut f = ObsFile { approx_pos_unit: "m(ITRF-like RINEX marker frame)".into(), ..Default::default() };
    let mut hard: Vec<ParseDiag> = Vec::new();
    let mut cur_epoch: Option<RawEpoch> = None;
    let mut cur_sat: Option<String> = None;
    let mut idx = 0usize; // index into obs_types for current satellite

    for (i0, raw_line) in text.lines().enumerate() {
        let line_no = i0 + 1;
        if raw_line.trim().is_empty() { continue; }
        let label = if raw_line.len() >= 60 { raw_line[60..].trim() } else { "" };
        if cur_epoch.is_none() {
            // header region
            match label {
                "MARKER NAME" => f.marker_name = raw_line[..60.min(raw_line.len())].trim().into(),
                "MARKER NUMBER" => f.station = raw_line[..20.min(raw_line.len())].trim().into(),
                "REC # / TYPE / VERS" => f.receiver = raw_line[..60.min(raw_line.len())].trim().into(),
                "ANT # / TYPE" => f.antenna = raw_line[..60.min(raw_line.len())].trim().into(),
                "APPROX POSITION XYZ" => {
                    let p: Vec<Option<f64>> = (0..3)
                        .map(|k| raw_line.get(k * 14..k * 14 + 14).and_then(|s| s.trim().parse().ok()))
                        .collect();
                    if let (Some(Some(x)), Some(Some(y)), Some(Some(z))) = (p[0], p[1], p[2]) {
                        f.approx_pos = Some((x, y, z));
                    } else {
                        f.diagnostics.push(d(line_no, "warning", "approx_pos",
                            "approx position not convertible; raw line retained".into(), raw_line));
                    }
                }
                "SYS / # / OBS TYPES" => {
                    let sys = raw_line[..1].trim().to_string();
                    let n: usize = raw_line[4..6].trim().parse().unwrap_or(0);
                    if sys.is_empty() || n == 0 {
                        hard.push(d(line_no, "error", "obs_types",
                            "invalid SYS / # / OBS TYPES record".into(), raw_line));
                    } else {
                        for k in 0..n {
                            let start = 7 + k * 4;
                            if let Some(t) = raw_line.get(start..start + 4) {
                                let t = t.trim();
                                if t.len() == 3 {
                                    f.obs_types.push((sys.clone(), t.to_string()));
                                }
                            }
                        }
                        if !f.sys_order.contains(&sys) { f.sys_order.push(sys); }
                    }
                }
                "END OF HEADER" => {
                    f.raw_header.push(raw_line.to_string());
                    if f.obs_types.is_empty() {
                        hard.push(d(line_no, "error", "obs_types",
                            "no observation types declared".into(), raw_line));
                    }
                    if !hard.is_empty() { return Err(hard); }
                    cur_epoch = None;
                }
                _ => f.raw_header.push(raw_line.to_string()),
            }
            continue;
        }
        // data region
        if raw_line.starts_with('>') {
            if let Some(e) = cur_epoch.take() { f.epochs.push(e); }
            cur_sat = None;
            idx = 0;
            match parse_epoch_line(raw_line, line_no, &mut f.diagnostics) {
                Ok(e) => cur_epoch = Some(e),
                Err(diag) => hard.push(diag),
            }
            continue;
        }
        let e = match cur_epoch.as_mut() { Some(e) => e, None => {
            hard.push(d(line_no, "error", "orphan_data",
                "observation data outside an epoch record".into(), raw_line));
            continue;
        }};
        let sat = raw_line[..3].trim().to_string();
        if sat.is_empty() || sat.len() < 3 {
            hard.push(d(line_no, "error", "sat_id", format!("invalid satellite id: {sat:?}"), raw_line));
            continue;
        }
        if cur_sat.as_deref() != Some(sat.as_str()) {
            cur_sat = Some(sat.clone());
            idx = 0;
        }
        let ntypes = f.obs_types.iter().filter(|(s, _)| s.as_str() == &sat[..1]).count();
        let body = &raw_line[3..];
        for k in 0..ntypes {
            let start = k * 16;
            if start + 14 > body.len() { break; }
            let chunk = &body[start..(start + 16).min(body.len())];
            if chunk.trim().is_empty() { idx += 1; continue; }
            let sys = &sat[..1];
            let code = f.obs_types.iter().filter(|(s, _)| s == sys).nth(k).map(|(_, c)| c.clone());
            let Some(code) = code else { continue; };
            let mut field = float_field(chunk, line_no, "obs_value", &mut f.diagnostics);
            field.u = Some(unit_for_code(&code));
            e.lines.push(RawSatLine { sat: sat.clone(), code, field });
            idx += 1;
        }
        let _ = idx;
    }
    if let Some(e) = cur_epoch.take() { f.epochs.push(e); }
    if !hard.is_empty() { Err(hard) } else { Ok(f) }
}

fn unit_for_code(code: &str) -> &'static str {
    match code.as_bytes()[0] {
        b'C' | b'L' => if code.as_bytes()[0] == b'C' { "m" } else { "cycle" },
        b'D' => "Hz",
        b'S' => "dB-Hz",
        _ => "unknown",
    }
}

fn parse_epoch_line(line: &str, line_no: usize, diags: &mut Vec<ParseDiag>) -> Result<RawEpoch, ParseDiag> {
    let nums = |off: usize, len: usize| line.get(off..off + len).and_then(|s| s.trim().parse::<i64>().ok());
    let yr = nums(2, 4);
    let mo = nums(7, 2);
    let dy = nums(10, 2);
    let hh = nums(13, 2);
    let mm = nums(16, 2);
    let ss = nums(19, 2);
    let flag = line.get(31..32).unwrap_or(" ").trim().to_string();
    if let (Some(y), Some(m), Some(dd), Some(h), Some(mi), Some(s)) = (yr, mo, dy, hh, mm, ss) {
        let y4 = if y < 80 { y + 2000 } else if y < 100 { y + 1900 } else { y };
        let time = format!("{y4:04}-{m:02}-{dd:02}T{h:02}:{mi:02}:{s:02}");
        let (clock_raw, clock_val) = if line.len() >= 42 && line[41..].trim() != "" {
            let tok = line[41..].trim();
            (Some(tok.to_string()), tok.parse::<f64>().ok())
        } else { (None, None) };
        if clock_raw.is_some() && clock_val.is_none() {
            diags.push(d(line_no, "warning", "clock_offset",
                "receiver clock offset not convertible; raw token retained".into(), line));
        }
        Ok(RawEpoch { time, time_raw: line[1..30.min(line.len())].trim().to_string(),
            flag, clock_offset_raw: clock_raw, clock_offset: clock_val, lines: vec![] })
    } else {
        Err(d(line_no, "error", "epoch_time", "invalid epoch date/time fields".into(), line))
    }
}

const GPS_NAV_UNITS: [&str; 17] = [
    "s(clock bias)", "1/s(clock drift)", "1/s^2(clock drift rate)",
    "s", "rad/orbit", "rad/orbit^2",
    "m", "rad", "rad/s",
    "rad/s", "rad/s^2", "rad/s^3",
    "m", "m", "m",
    "1", "1",
];

pub fn parse_nav(text: &str, kind_hint: &str) -> Result<Vec<NavRecord>, Vec<ParseDiag>> {
    let mut out: Vec<NavRecord> = Vec::new();
    let mut hard: Vec<ParseDiag> = Vec::new();
    let mut in_header = true;
    let mut pending: Option<(String, String, String, usize)> = None;
    let mut vals: Vec<RawField> = Vec::new();
    for (i0, line) in text.lines().enumerate() {
        let line_no = i0 + 1;
        if in_header {
            if line.len() >= 60 && line[60..].trim() == "END OF HEADER" { in_header = false; }
            continue;
        }
        if line.trim().is_empty() { continue; }
        if pending.is_none() {
            // first broadcast-orbit line: PRN / EPOCH / SV CLK
            let sat = line[..3].trim().to_string();
            let nums = |off: usize, len: usize| line.get(off..off + len).and_then(|s| s.trim().parse::<i64>().ok());
            let (yr, mo, dy, hh, mm, ss) = (nums(4, 4), nums(9, 2), nums(12, 2),
                                            nums(15, 2), nums(18, 2), nums(21, 2));
            if sat.len() != 3 {
                hard.push(d(line_no, "error", "nav_sat", "invalid navigation satellite id".into(), line));
                continue;
            }
            let Some((y, m, dd, h, mi, s)) = yr.zip(mo).zip(dy).zip(hh).zip(mm).zip(ss)
                .map(|(((((y, m), dd), h), mi), s)| (y, m, dd, h, mi, s)) else {
                return Err(vec![d(line_no, "error", "nav_time",
                    "invalid navigation epoch fields".into(), line)]);
            };
            let y4 = if y < 80 { y + 2000 } else if y < 100 { y + 1900 } else { y };
            let time = format!("{y4:04}-{m:02}-{dd:02}T{h:02}:{mi:02}:{s:02}");
            let mut di = Vec::new();
            let mut v3 = Vec::new();
            for k in 0..3 {
                let start = 22 + k * 19;
                if let Some(chunk) = line.get(start..start + 19) {
                    if chunk.trim() != "" {
                        let mut rf = float_field(chunk, line_no, "nav_value", &mut di);
                        rf.u = Some(GPS_NAV_UNITS[k.min(2)]);
                        v3.push(rf);
                    }
                }
            }
            vals = v3;
            let kind = if kind_hint == "glo" || sat.starts_with('R') { "glo_nav" } else { "gps_nav" };
            pending = Some((sat, time, line[4..22].trim().to_string(), line_no));
            let _ = kind;
            continue;
        }
        let mut di = Vec::new();
        for k in 0..4 {
            let start = k * 19;
            if let Some(chunk) = line.get(start..start + 19) {
                if chunk.trim() != "" {
                    let mut rf = float_field(chunk, line_no, "nav_value", &mut di);
                    let gi = vals.len();
                    rf.u = Some(if pending.as_ref().map(|p| p.0.starts_with('R')).unwrap_or(false) {
                        "GLO-broadcast-field(see RINEX NAV table)"
                    } else {
                        GPS_NAV_UNITS[gi.min(GPS_NAV_UNITS.len() - 1)]
                    });
                    vals.push(rf);
                }
            }
        }
        if vals.len() >= (if pending.as_ref().unwrap().0.starts_with('R') { 15 } else { 17 }) {
            let (sat, time, time_raw, _ln) = pending.take().unwrap();
            let is_glo = sat.starts_with('R');
            let mut freq = None;
            if is_glo {
                // GLO line 4 field 2 (overall zero-based index 11) carries the
                // frequency number (frequency slot, signed integer).
                if let Some(rf) = vals.get(11) {
                    if let Some(v) = rf.v { freq = Some(v.round() as i32); }
                }
            }
            let limit = if is_glo { 15 } else { 17 };
            vals.truncate(limit);
            out.push(NavRecord {
                sat, time, time_raw,
                kind: if is_glo { "glo_nav".into() } else { "gps_nav".into() },
                frequency_number: freq, values: vals,
            });
            vals = Vec::new();
        }
    }
    if pending.is_some() {
        hard.push(d(0, "error", "nav_truncated",
            "navigation record missing continuation lines".into(), ""));
    }
    if hard.is_empty() { Ok(out) } else { Err(hard) }
}

/// Clock-event ledger (RINEX-style comment ledger kept in project text files):
/// `CLOCK <iso-time> <source> <magnitude><unit> <note...>`
pub fn parse_clock_events(text: &str) -> (Vec<ClockEventRaw>, Vec<ParseDiag>) {
    let mut out = Vec::new();
    let mut diags = Vec::new();
    for (i0, line) in text.lines().enumerate() {
        let line_no = i0 + 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let t = t.strip_prefix("CLOCK").unwrap_or(t).trim_start();
        let mut parts = t.splitn(4, char::is_whitespace);
        let (Some(time), Some(source), Some(mag), note) =
            (parts.next(), parts.next(), parts.next(), parts.next()) else {
            diags.push(d(line_no, "error", "clock_event",
                "expected: CLOCK <time> <source> <magnitude><unit> <note>".into(), line));
            continue;
        };
        let mut unit = "s";
        let mut num = String::new();
        for c in mag.chars() {
            if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E' {
                num.push(c);
            } else {
                unit = "";
                unit = { let rest = &mag[num.len()..]; let _ = rest; mag.get(num.len()..).unwrap_or("s") };
                break;
            }
        }
        let magnitude = num.parse::<f64>().ok();
        if magnitude.is_none() {
            diags.push(d(line_no, "warning", "clock_magnitude",
                format!("clock-event magnitude not convertible: {mag:?}; raw token retained"), line));
        }
        out.push(ClockEventRaw {
            time: time.into(), time_raw: time.into(), source: source.into(),
            magnitude_raw: mag.into(), magnitude,
            unit: unit.to_string(), note: note.unwrap_or("").into(),
        });
    }
    (out, diags)
}
