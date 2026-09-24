//! CBF v1 (Carrier-Breakpoint Format) importer.
//!
//! A deliberately small, copy-pasteable, RINEX-like text format:
//!
//! ```text
//! # cbf v1
//! STATION marker=ST01 receiver=rcvr-1 antenna=ANT1 xyz="..." datum=ITRF2020
//! BAND 1 phase=L1 pseudo=C1 doppler=D1 snr=S1 f=1575.42e6Hz wl=0.1902936728m
//! BAND 2 phase=L2 pseudo=C2 doppler=D2 snr=S2 f=1227.60e6Hz wl=0.2442102134m
//! EPOCH 2024-03-15T00:00:00
//!   G01 L1=123456.789cycle C1=20200000.123m D1=-1234.5Hz S1=45dBHz LLI1=0
//!   G01 L2=... C2=...
//!   CLOCK delta=1000ns
//!   STATE G01 az=123.4deg el=45.6deg health=0
//! END
//! ```
//!
//! Rules:
//! * Every value token is stored exactly as written, including its unit
//!   suffix. Numeric conversion is best-effort: a value that cannot convert
//!   stays at its epoch/sat/code with a diagnostic and is never dropped.
//! * Lines that cannot be interpreted produce diagnostics with line numbers
//!   but never panic.

use crate::ids;
use crate::model::*;
use crate::vtime;

pub const FORMAT_ID: &str = "cbf";
pub const FORMAT_VERSION: &str = "v1";

pub fn parse(input: &str, batch_id: &str) -> ParsedImport {
    let raw_sha = ids::sha256_hex(input.as_bytes());
    let mut p = Parser {
        batch_id: batch_id.to_string(),
        raw_sha: raw_sha.clone(),
        header: StationHeader {
            coord_datum: "ITRF (xyz metres)".to_string(),
            coord_unit: U_M.to_string(),
            ..Default::default()
        },
        bands: Vec::new(),
        observations: Vec::new(),
        clock_events: Vec::new(),
        sat_states: Vec::new(),
        diagnostics: Vec::new(),
        current_epoch: None,
    };
    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx as i64 + 1;
        let line = strip_comment(raw_line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        p.line(line_no, line);
    }
    ParsedImport {
        format: format!("{FORMAT_ID}-{FORMAT_VERSION}"),
        raw_sha,
        header: p.header,
        bands: p.bands,
        observations: p.observations,
        clock_events: p.clock_events,
        sat_states: p.sat_states,
        diagnostics: p.diagnostics,
    }
}

fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            // Keep the leading `# cbf v1` magic out of data, allow inline comments.
            return &line[..i];
        }
    }
    line
}

/// Satellite ids: constellation letter followed by digits (G01, R02, E12,
/// J03, C21). Bare words like "STATE" or "STATION" do not match.
fn is_sat_id(s: &str) -> bool {
    let mut it = s.chars();
    matches!(it.next(), Some('G' | 'R' | 'E' | 'C' | 'J'))
        && it.next().is_some_and(|c| c.is_ascii_digit())
        && s.chars().skip(1).all(|c| c.is_ascii_digit())
}

struct Parser {
    batch_id: String,
    raw_sha: String,
    header: StationHeader,
    bands: Vec<BandDef>,
    observations: Vec<RawObservation>,
    clock_events: Vec<RawClockEvent>,
    sat_states: Vec<RawSatState>,
    diagnostics: Vec<Diagnostic>,
    current_epoch: Option<(String, Option<f64>)>,
}

impl Parser {
    fn err(&mut self, line_no: i64, code: &str, msg: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            line_no,
            code: code.to_string(),
            message: msg.into(),
            severity: "error".to_string(),
        });
    }

    fn line(&mut self, line_no: i64, line: &str) {
        let (head, rest) = match line.split_once(char::is_whitespace) {
            Some((h, r)) => (h, r.trim()),
            None => (line, ""),
        };
        match head {
            "#" | "FORMAT" => {
                // magic/format marker, informational
            }
            "STATION" => self.station_line(line_no, rest),
            "BAND" => self.band_line(line_no, rest),
            "EPOCH" => self.epoch_line(line_no, rest),
            "END" => {
                self.current_epoch = None;
            }
            "CLOCK" => self.clock_line(line_no, rest),
            "STATE" => self.state_line(line_no, rest),
            other if is_sat_id(other) => self.obs_line(line_no, other, rest),
            other => self.err(
                line_no,
                "UNKNOWN_LINE",
                format!("unrecognized line starting with {other:?}"),
            ),
        }
    }
}

/// Split `key=value` pairs. Quotes are only honored when the value *starts*
/// with `"`; the token `L1=123cycle` is a plain value with unit suffix.
fn kv(rest: &str) -> Vec<(String, String)> {
    let b = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        let key_start = i;
        while i < b.len() && b[i] != b'=' && b[i] != b' ' {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            // bare word: skip one whole token
            while i < b.len() && b[i] != b' ' {
                i += 1;
            }
            continue;
        }
        let key = rest[key_start..i].to_string();
        i += 1;
        let val = if i < b.len() && b[i] == b'"' {
            i += 1;
            let qs = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            let v = rest[qs..i].to_string();
            if i < b.len() {
                i += 1; // closing quote
            }
            v
        } else {
            let vs = i;
            while i < b.len() && b[i] != b' ' && b[i] != b'\t' {
                i += 1;
            }
            rest[vs..i].to_string()
        };
        out.push((key, val));
    }
    out
}

fn split_unit(token: &str) -> (String, String) {
    // Unit is the trailing run of ascii letters (dBHz handled explicitly).
    // Split at the first letter that follows the numeric part.
    if let Some(i) = token.find("dBHz") {
        return (token[..i].to_string(), "dBHz".to_string());
    }
    let bytes = token.as_bytes();
    let mut i = 0usize;
    // consume optional sign, digits, dot, exponent notation
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    (token[..i].to_string(), token[i..].to_string())
}

fn parse_f64_token(token: &str) -> Option<f64> {
    let (num, _unit) = split_unit(token);
    num.trim().parse::<f64>().ok()
}

impl Parser {
    fn station_line(&mut self, line_no: i64, rest: &str) {
        for (k, v) in kv(rest) {
            match k.as_str() {
                "marker" => self.header.marker = v,
                "receiver" => self.header.receiver = v,
                "antenna" => self.header.antenna = v,
                "xyz" => {
                    self.header.approx_xyz_raw = v.clone();
                    let parts: Vec<&str> = v.split(',').collect();
                    if parts.len() != 3 {
                        self.err(line_no, "BAD_XYZ", format!("xyz needs 3 components: {v:?}"));
                    } else {
                        let mut arr = [0.0; 3];
                        let mut ok = true;
                        for (i, p) in parts.iter().enumerate() {
                            match p.trim().parse::<f64>() {
                                Ok(x) => arr[i] = x,
                                Err(_) => {
                                    ok = false;
                                    self.err(
                                        line_no,
                                        "BAD_XYZ",
                                        format!("non-numeric xyz component {:?}", p.trim()),
                                    );
                                }
                            }
                        }
                        if ok {
                            self.header.approx_xyz_m = Some(arr);
                        }
                    }
                }
                "datum" => self.header.coord_datum = v,
                "unit" => self.header.coord_unit = v,
                other => self.err(line_no, "UNKNOWN_STATION_KEY", format!("{other}={v}")),
            }
        }
    }

    fn band_for_code(&self, code: &str) -> Option<String> {
        let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Some(b) = self.bands.iter().find(|b| b.phase_code == code
            || b.pseudo_code == code
            || b.doppler_code == code
            || b.snr_code == code
            || format!("LLI{}", b.band) == code)
        {
            return Some(b.band.clone());
        }
        if !digits.is_empty() {
            return Some(digits);
        }
        None
    }

    fn band_line(&mut self, line_no: i64, rest: &str) {
        let mut parts = kv(rest);
        if parts.is_empty() {
            self.err(line_no, "BAD_BAND", "empty BAND line");
            return;
        }
        let band = parts.remove(0).1;
        let mut b = BandDef {
            band: band.clone(),
            phase_code: format!("L{band}"),
            pseudo_code: format!("C{band}"),
            doppler_code: format!("D{band}"),
            snr_code: format!("S{band}"),
            frequency_raw: String::new(),
            frequency_hz: None,
            wavelength_raw: String::new(),
            wavelength_m: None,
            unit: U_HZ_BAND.to_string(),
            datum: "nominal_carrier_frequency".to_string(),
        };
        for (k, v) in parts {
            match k.as_str() {
                "phase" => b.phase_code = v,
                "pseudo" => b.pseudo_code = v,
                "doppler" => b.doppler_code = v,
                "snr" => b.snr_code = v,
                "f" => {
                    b.frequency_raw = v.clone();
                    b.frequency_hz = parse_f64_token(&v);
                    if b.frequency_hz.is_none() {
                        self.err(line_no, "BAD_FREQUENCY", format!("cannot parse f={v:?}"));
                    }
                }
                "wl" => {
                    b.wavelength_raw = v.clone();
                    b.wavelength_m = parse_f64_token(&v);
                    if b.wavelength_m.is_none() {
                        self.err(line_no, "BAD_WAVELENGTH", format!("cannot parse wl={v:?}"));
                    }
                }
                other => self.err(line_no, "UNKNOWN_BAND_KEY", format!("{other}={v}")),
            }
        }
        // Cross-check wl = c/f when both present (warning only; both kept).
        if let (Some(f), Some(w)) = (b.frequency_hz, b.wavelength_m) {
            if f > 0.0 && w > 0.0 {
                let implied = crate::engine::SPEED_OF_LIGHT_M_S / f;
                if (implied - w).abs() > 1e-4 {
                    self.diagnostics.push(Diagnostic {
                        line_no,
                        code: "WL_F_MISMATCH".to_string(),
                        message: format!(
                            "band {band}: declared wavelength {w} m differs from c/f = {implied:.9} m; both tokens retained"
                        ),
                        severity: "warning".to_string(),
                    });
                }
            }
        }
        self.bands.push(b);
    }

    fn epoch_line(&mut self, line_no: i64, rest: &str) {
        let token = rest.split_whitespace().next().unwrap_or("");
        match vtime::parse_iso(token) {
            Ok(t) => self.current_epoch = Some((token.to_string(), Some(t))),
            Err(e) => {
                self.err(line_no, "BAD_EPOCH", e);
                self.current_epoch = Some((token.to_string(), None));
            }
        }
    }
}

impl Parser {
    fn obs_line(&mut self, line_no: i64, sat: &str, rest: &str) {
        let (epoch_raw, epoch_secs) = match &self.current_epoch {
            Some(e) => e.clone(),
            None => {
                self.err(line_no, "ORPHAN_OBS", format!("observation for {sat} outside EPOCH block"));
                return;
            }
        };
        for (code, token) in kv(rest) {
            let kind = ObsKind::from_code(&code);
            let band = self
                .band_for_code(&code)
                .unwrap_or_else(|| "?".to_string());
            if band == "?" {
                self.err(
                    line_no,
                    "UNKNOWN_CODE",
                    format!("observation code {code:?} matches no declared BAND"),
                );
            }
            let (num, unit) = split_unit(&token);
            let expected = kind.expected_unit();
            let mut diagnostic = None;
            if !unit.is_empty() && unit != expected {
                let msg = format!(
                    "unit mismatch: {code}={token:?} has unit {unit:?}, expected {expected:?}; raw token retained"
                );
                self.diagnostics.push(Diagnostic {
                    line_no,
                    code: "UNIT_MISMATCH".to_string(),
                    message: msg.clone(),
                    severity: "warning".to_string(),
                });
                diagnostic = Some(msg);
            }
            let value = match num.trim().parse::<f64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    let msg = format!("non-numeric observation {code}={token:?}; left in place unnormalized");
                    self.diagnostics.push(Diagnostic {
                        line_no,
                        code: "UNCONVERTIBLE_VALUE".to_string(),
                        message: msg.clone(),
                        severity: "error".to_string(),
                    });
                    diagnostic = Some(msg);
                    None
                }
            };
            let natural = ids::obs_natural(&self.batch_id, &epoch_raw, sat, &code);
            self.observations.push(RawObservation {
                natural_id: natural,
                batch_id: self.batch_id.clone(),
                line_no,
                epoch_raw: epoch_raw.clone(),
                epoch_secs,
                epoch_datum: DATUM_TIME.to_string(),
                sat: sat.to_string(),
                code: code.clone(),
                kind,
                band,
                raw_value: token.clone(),
                raw_unit: unit,
                value,
                diagnostic,
            });
        }
    }

    fn clock_line(&mut self, line_no: i64, rest: &str) {
        let (epoch_raw, epoch_secs) = match &self.current_epoch {
            Some(e) => e.clone(),
            None => {
                self.err(line_no, "ORPHAN_CLOCK", "CLOCK outside EPOCH block");
                return;
            }
        };
        let mut raw_delta = String::new();
        let mut raw_unit = String::new();
        for (k, v) in kv(rest) {
            match k.as_str() {
                "delta" => {
                    let (n, u) = split_unit(&v);
                    raw_delta = n;
                    raw_unit = if u.is_empty() { U_NS.to_string() } else { u };
                }
                other => self.err(line_no, "UNKNOWN_CLOCK_KEY", format!("{other}={v}")),
            }
        }
        if raw_delta.is_empty() {
            self.err(line_no, "BAD_CLOCK", "CLOCK line missing delta=");
            return;
        }
        let mut diagnostic = None;
        let delta_ns = match raw_delta.parse::<f64>() {
            Ok(v) => {
                let v = if raw_unit == "us" {
                    v * 1000.0
                } else if raw_unit == "ms" {
                    v * 1_000_000.0
                } else if raw_unit == "s" {
                    v * 1e9
                } else {
                    v
                };
                Some(v)
            }
            Err(_) => {
                let msg = format!("non-numeric clock delta {raw_delta:?}");
                self.diagnostics.push(Diagnostic {
                    line_no,
                    code: "UNCONVERTIBLE_VALUE".to_string(),
                    message: msg.clone(),
                    severity: "error".to_string(),
                });
                diagnostic = Some(msg);
                None
            }
        };
        let natural = ids::clock_natural(&self.batch_id, &epoch_raw);
        self.clock_events.push(RawClockEvent {
            natural_id: natural,
            batch_id: self.batch_id.clone(),
            line_no,
            epoch_raw,
            epoch_secs,
            epoch_datum: DATUM_TIME.to_string(),
            raw_delta,
            raw_unit,
            delta_ns,
            diagnostic,
        });
    }

    fn state_line(&mut self, line_no: i64, rest: &str) {
        let (epoch_raw, epoch_secs) = match &self.current_epoch {
            Some(e) => e.clone(),
            None => {
                self.err(line_no, "ORPHAN_STATE", "STATE outside EPOCH block");
                return;
            }
        };
        let (sat, tail) = match rest.split_once(char::is_whitespace) {
            Some((s, t)) => (s.to_string(), t.trim()),
            None => (rest.to_string(), ""),
        };
        if sat.is_empty() {
            self.err(line_no, "BAD_STATE", "empty STATE line");
            return;
        }
        let parts = kv(tail);
        let mut az_raw = String::new();
        let mut el_raw = String::new();
        let mut health_raw = "0".to_string();
        let mut diagnostic = None;
        for (k, v) in parts {
            match k.as_str() {
                "az" => az_raw = v,
                "el" => el_raw = v,
                "health" => health_raw = v,
                other => self.err(line_no, "UNKNOWN_STATE_KEY", format!("{other}={v}")),
            }
        }
        let az_deg = parse_f64_token(&az_raw);
        let el_deg = parse_f64_token(&el_raw);
        if az_deg.is_none() || el_deg.is_none() {
            let msg = "satellite state angle unconvertible; retained raw";
            self.diagnostics.push(Diagnostic {
                line_no,
                code: "UNCONVERTIBLE_VALUE".to_string(),
                message: msg.to_string(),
                severity: "error".to_string(),
            });
            diagnostic = Some(msg.to_string());
        }
        let natural = ids::state_natural(&self.batch_id, &epoch_raw, &sat);
        self.sat_states.push(RawSatState {
            natural_id: natural,
            batch_id: self.batch_id.clone(),
            line_no,
            epoch_raw,
            epoch_secs,
            epoch_datum: DATUM_TIME.to_string(),
            sat,
            az_raw,
            el_raw,
            health_raw,
            datum: DATUM_TOPO.to_string(),
            az_deg,
            el_deg,
            diagnostic,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../cases/carrier-breakpoint/v1-baseline.cbf");

    #[test]
    fn parses_baseline_raw_tokens() {
        let p = parse(SAMPLE, "in_test");
        assert_eq!(p.format, "cbf-v1");
        assert!(p.bands.len() >= 2);
        assert!(!p.observations.is_empty());
        for o in &p.observations {
            assert!(o.value.is_some(), "{} {}", o.code, o.raw_value);
            assert!(!o.raw_unit.is_empty());
        }
        let l1 = p
            .observations
            .iter()
            .find(|o| o.code == "L1")
            .expect("L1 present");
        assert_eq!(l1.raw_unit, U_CYCLE);
    }

    #[test]
    fn unconvertible_stays_in_place() {
        let bad = "# cbf v1\nBAND 1\nEPOCH 2024-01-01T00:00:00\n  G07 L1=oopsycle\nEND\n";
        let p = parse(bad, "in_bad");
        let o = p.observations.iter().find(|o| o.sat == "G07").unwrap();
        assert!(o.value.is_none());
        assert_eq!(o.raw_value, "oopsycle");
        assert!(p.diagnostics.iter().any(|d| d.code == "UNCONVERTIBLE_VALUE"));
    }

    #[test]
    fn malformed_lines_are_diagnosed_not_fatal() {
        let bad = "# cbf v1\nFROBNICATE yes\nEPOCH not-a-time\nEND\n";
        let p = parse(bad, "in_bad2");
        assert!(p.diagnostics.iter().any(|d| d.code == "UNKNOWN_LINE"));
        assert!(p.diagnostics.iter().any(|d| d.code == "BAD_EPOCH"));
    }
}
