//! Import layer: parses a small, explicit RINEX 3 observation subset and a
//! satellite-state JSON sidecar. Raw textual representations are preserved;
//! every convertibility problem is attached in place as a diagnostic.

use crate::domain::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl ImportReport {
    fn err(&mut self, line_no: usize, msg: impl Into<String>) {
        self.errors.push(format!("line {line_no}: {}", msg.into()));
    }
    fn warn(&mut self, line_no: usize, msg: impl Into<String>) {
        self.warnings.push(format!("line {line_no}: {}", msg.into()));
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Parse RINEX 3.x OBSERVATION DATA (GPS records used by the bundled case).
pub fn parse_rinex3(text: &str) -> Result<(RawBundle, ImportReport), String> {
    let mut report = ImportReport::default();
    let mut bundle = RawBundle {
        station: String::new(),
        marker: String::new(),
        receiver: String::new(),
        rinex_version: String::new(),
        constellation: "GPS".into(),
        receiver_pos: Vec::new(),
        receiver_pos_unit: Unit::Meter,
        system_types: Vec::new(),
        epochs: Vec::new(),
        clock_events: Vec::new(),
        sat_states: Vec::new(),
    };
    let mut types_order: Vec<String> = Vec::new();

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut header_done = false;
    while i < lines.len() {
        let line = lines[i];
        if !header_done {
            let label = if line.len() >= 60 { line[59..].trim() } else { line.trim() };
            match label {
                "RINEX VERSION / TYPE" => {
                    let ver = line.get(0..9).unwrap_or("").trim().to_string();
                    let ftype = line.get(20..21).unwrap_or("").trim();
                    let sys = line.get(40..41).unwrap_or("").trim();
                    if ftype != "O" {
                        report.err(i + 1, format!("expected OBSERVATION DATA file, found type '{ftype}'"));
                    }
                    if !ver.starts_with('3') {
                        report.err(i + 1, format!("only RINEX 3.x supported, found version '{ver}'"));
                    }
                    bundle.rinex_version = ver;
                    bundle.constellation = sys.to_string();
                }
                "MARKER NAME" => bundle.marker = line.get(0..4).unwrap_or("").trim().into(),
                "REC # / TYPE / VERS" => {
                    bundle.receiver = line.get(0..20).unwrap_or("").trim().replace(' ', "_")
                }
                "# / TYPES OF OBSERV" => {
                    let cont = line.get(0..6).unwrap_or("").trim().parse::<i64>().unwrap_or(0);
                    let n = cont as usize;
                    for k in 0..n {
                        let start = 6 + k * 4;
                        if start + 4 <= 60 {
                            let code = line[start..start + 4].trim();
                            if !code.is_empty() {
                                types_order.push(code.into());
                            }
                        }
                    }
                }
                "SYS / # / OBS TYPES" => {
                    let sys = line.get(0..1).unwrap_or("").trim().to_string();
                    if !bundle.system_types.contains(&sys) && !sys.is_empty() {
                        bundle.system_types.push(sys.clone());
                    }
                    let n = line.get(3..6).unwrap_or("").trim().parse::<usize>().unwrap_or(0);
                    for k in 0..n {
                        let start = 6 + k * 4;
                        if start + 4 <= 60 {
                            let code = line[start..start + 4].trim();
                            if !code.is_empty() && !types_order.contains(&code.to_string()) {
                                types_order.push(code.into());
                            }
                        }
                    }
                }
                "APPROX POSITION XYZ" => {
                    for col in [0usize, 14, 28] {
                        let raw = line.get(col..col + 14).unwrap_or("").trim().to_string();
                        match raw.parse::<f64>() {
                            Ok(v) => bundle.receiver_pos.push(v),
                            Err(_) => report.err(i + 1, format!("bad receiver position field '{raw}'")),
                        }
                    }
                }
                "END OF HEADER" => {
                    if types_order.is_empty() {
                        report.err(i + 1, "no observation types declared in header");
                    }
                    header_done = true;
                }
                _ => {}
            }
            i += 1;
            continue;
        }

        // Data section: epoch header starts at column 1 with '>'.
        if !line.starts_with('>') {
            report.err(i + 1, "expected epoch header '>' in data section");
            i += 1;
            continue;
        }
        let parsed_epoch = parse_epoch_header(line, &mut report, i + 1, &mut bundle);
        i += 1;
        let Some(epoch) = parsed_epoch else {
            // consume the rest of this epoch's (possibly broken) sat lines defensively
            while i < lines.len() && !lines[i].starts_with('>') {
                i += 1;
            }
            continue;
        };

        let mut epoch = epoch;
        // satellite records until next '>'
        let sats_this_epoch = count_sat_lines(&lines[i..]);
        for _ in 0..sats_this_epoch {
            let sat_line = lines[i];
            parse_sat_line(sat_line, &types_order, &mut report, i + 1, &mut epoch);
            i += 1;
        }
        bundle.epochs.push(epoch);
    }

    if !header_done {
        return Err("missing END OF HEADER".into());
    }
    if bundle.epochs.is_empty() && !report.has_errors() {
        report.err(0, "no epochs found");
    }
    if bundle.station.is_empty() {
        bundle.station = bundle.marker.clone();
    }
    Ok((bundle, report))
}

fn parse_epoch_header(
    line: &str,
    report: &mut ImportReport,
    line_no: usize,
    bundle: &mut RawBundle,
) -> Option<RawEpoch> {
    let gpst_iso = parse_epoch_time(line, report, line_no)?;
    let flag = field(line, 31, 3).trim().parse::<u8>().unwrap_or(0);
    let nsat = field(line, 32, 3).trim().parse::<usize>().unwrap_or(0);
    let clk_raw = field(line, 41, 15).trim().to_string();
    let recv_clk = if clk_raw.is_empty() {
        RawValue::blank(Unit::Second)
    } else {
        match clk_raw.parse::<f64>() {
            Ok(v) => RawValue::parsed(clk_raw.clone(), v, Unit::Second),
            Err(_) => RawValue::invalid(clk_raw, Unit::Second, "receiver clock offset not a float".into()),
        }
    };

    // Raw clock-event evidence: a step in the header clock offset vs the
    // previous epoch is recorded verbatim (machine-readable event material).
    if let Some(prev) = bundle.epochs.last() {
        if let (Some(a), Some(b)) = (recv_clk.value, prev.recv_clk_offset_raw.value) {
            let jump = a - b;
            if jump.abs() >= 1e-4 {
                let ev = RawClockEvent {
                    epoch_no: prev.epoch_no + 1,
                    gpst_iso: gpst_iso.clone(),
                    kind: "receiver_clock_step".into(),
                    jump_seconds_raw: RawValue::parsed(format!("{jump:11.7}"), jump, Unit::Second),
                };
                bundle.clock_events.push(ev);
            }
        }
    }

    if nsat == 0 {
        report.warn(line_no, "epoch declares 0 satellites");
    }
    Some(RawEpoch {
        epoch_no: bundle.epochs.len() as u32 + 1,
        gpst_iso,
        epoch_flag: flag,
        recv_clk_offset_raw: recv_clk,
        measurements: Vec::new(),
    })
}

fn parse_epoch_time(line: &str, report: &mut ImportReport, line_no: usize) -> Option<String> {
    let yr = field(line, 2, 4).trim().parse::<i32>().ok();
    let mon = field(line, 6, 3).trim().parse::<u32>().ok();
    let day = field(line, 9, 3).trim().parse::<u32>().ok();
    let hour = field(line, 12, 3).trim().parse::<u32>().ok();
    let minute = field(line, 15, 3).trim().parse::<u32>().ok();
    let second = field(line, 18, 11).trim().parse::<f64>().ok();
    match (yr, mon, day, hour, minute, second) {
        (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) => {
            if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || !(0.0..60.0).contains(&s) {
                report.err(line_no, format!("out-of-range epoch date {y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:06.3}"));
                None
            } else {
                Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:06.3f} [GPS]"))
            }
        }
        _ => {
            report.err(line_no, "malformed epoch header timestamp");
            None
        }
    }
}

fn count_sat_lines(rest: &[&str]) -> usize {
    rest.iter().take_while(|l| !l.starts_with('>')).count()
}

fn field(line: &str, start: usize, len: usize) -> &str {
    let end = (start + len).min(line.len());
    if start >= line.len() {
        ""
    } else {
        &line[start..end]
    }
}

fn parse_scalar(raw: &str, unit: Unit, what: &str, sat: &str, line_no: usize, report: &mut ImportReport) -> Option<RawValue> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<f64>() {
        Ok(v) => Some(RawValue::parsed(raw.into(), v, unit)),
        Err(_) => {
            report.err(line_no, format!("{what} for {sat}: unconvertible value '{raw}', kept in place"));
            Some(RawValue::invalid(raw.into(), unit, format!("{what} not parseable as float")))
        }
    }
}

fn parse_sat_line(
    line: &str,
    types_order: &[String],
    report: &mut ImportReport,
    line_no: usize,
    epoch: &mut RawEpoch,
) {
    let sat = field(line, 0, 3).trim().to_string();
    if sat.is_empty() {
        report.err(line_no, "satellite record without satellite id");
        return;
    }
    // RINEX 3: each type occupies 16 columns: F14.3 + LLI(1) + SS(1).
    // Records may wrap across multiple physical lines; we parse all available.
    for (idx, code) in types_order.iter().enumerate() {
        let start = 3 + idx * 16;
        if start >= line.len() {
            break;
        }
        let chunk = field(line, start, 16);
        let num = chunk.get(0..14).unwrap_or("").trim();
        let lli_ch = chunk.get(14..15).unwrap_or(" ").trim();
        let ss_ch = chunk.get(15..16).unwrap_or(" ").trim();
        if num.trim().is_empty() && lli_ch.is_empty() && ss_ch.is_empty() {
            continue; // this frequency is absent at this epoch
        }
        let band = match code.as_bytes().get(1) {
            Some(b'1') => 1u8,
            Some(b'2') => 2,
            Some(b'5') => 5,
            _ => {
                report.warn(line_no, format!("{sat}: frequency band of code '{code}' unknown, raw value retained"));
                0
            }
        };
        let unit = if code.starts_with('C') {
            Unit::Meter
        } else if code.starts_with('L') {
            Unit::Cycle
        } else if code.starts_with('D') {
            Unit::MetersPerSecond
        } else {
            Unit::Dimensionless
        };
        let scalar = parse_scalar(num, unit, code, &sat, line_no, report);
        let lli = lli_ch.parse::<u8>().ok();
        let snr = if ss_ch.is_empty() {
            None
        } else {
            match ssi_to_dbhz(ss_ch) {
                Some(v) => Some(RawValue::parsed(ss_ch.into(), v, Unit::Dimensionless)),
                None => Some(RawValue::invalid(ss_ch.into(), Unit::Dimensionless, "signal strength indicator not 1-9".into())),
            }
        };
        let mut m = RawMeasurement {
            sat: sat.clone(),
            obs_code: code.clone(),
            freq_no: band,
            range: None,
            phase: None,
            doppler: None,
            lli,
            snr,
        };
        match code.as_bytes().first() {
            Some(b'C') => m.range = scalar,
            Some(b'L') => m.phase = scalar,
            Some(b'D') => m.doppler = scalar,
            _ => {
                report.warn(line_no, format!("{sat}: observation code '{code}' retained but not mapped to C/L/D"));
                m.phase = scalar;
            }
        }
        epoch.measurements.push(m);
    }
}

/// RINEX signal strength indicator 1..9 mapped to dB-Hz (spec table mapping).
fn ssi_to_dbhz(s: &str) -> Option<f64> {
    let n: u8 = s.parse().ok()?;
    if n == 0 {
        return None;
    }
    Some(match n {
        1 => 15.0,
        2 => 20.0,
        3 => 25.0,
        4 => 30.0,
        5 => 35.0,
        6 => 40.0,
        7 => 45.0,
        8 => 50.0,
        _ => 55.0,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateDoc {
    pub station: String,
    #[serde(default)]
    pub datum: Datum,
    #[serde(default)]
    pub clock_events: Vec<RawClockEvent>,
    pub rows: Vec<SatStateRowJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateRowJson {
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub sat: String,
    pub x: String,
    pub y: String,
    pub z: String,
    #[serde(default)]
    pub sat_clock_bias: Option<String>,
}

/// Parse the satellite-state JSON sidecar and merge it into the bundle.
pub fn merge_sat_states(bundle: &mut RawBundle, json_text: &str) -> Result<ImportReport, String> {
    let mut report = ImportReport::default();
    let doc: SatStateDoc = serde_json::from_str(json_text)
        .map_err(|e| format!("satellite-state JSON invalid: {e}"))?;
    if doc.rows.is_empty() {
        report.err(0, "satellite-state file contains no rows");
    }
    for (idx, r) in doc.rows.iter().enumerate() {
        let line_no = idx + 1;
        let x = parse_state_scalar(&r.x, Unit::Meter, "x", &r.sat, line_no, &mut report);
        let y = parse_state_scalar(&r.y, Unit::Meter, "y", &r.sat, line_no, &mut report);
        let z = parse_state_scalar(&r.z, Unit::Meter, "z", &r.sat, line_no, &mut report);
        let bias_raw = r.sat_clock_bias.clone().unwrap_or_default();
        let sat_clock_bias = if bias_raw.trim().is_empty() {
            RawValue::invalid(bias_raw, Unit::Second, "satellite clock bias absent".into())
        } else {
            match bias_raw.parse::<f64>() {
                Ok(v) => RawValue::parsed(bias_raw, v, Unit::Second),
                Err(_) => RawValue::invalid(
                    bias_raw,
                    Unit::Second,
                    "satellite clock bias unconvertible; retained in place".into(),
                ),
            }
        };
        bundle.sat_states.push(SatStateRow {
            epoch_no: r.epoch_no,
            gpst_iso: r.gpst_iso.clone(),
            sat: r.sat.clone(),
            x,
            y,
            z,
            sat_clock_bias,
            datum: doc.datum.clone(),
        });
    }
    // Operator-reported clock events are kept verbatim alongside any derived
    // from the RINEX header; duplicates are allowed (evidence, not truth).
    for ev in doc.clock_events {
        bundle.clock_events.push(ev);
    }
    if bundle.station.is_empty() {
        bundle.station = doc.station;
    }
    Ok(report)
}

fn parse_state_scalar(raw: &str, unit: Unit, what: &str, sat: &str, line_no: usize, report: &mut ImportReport) -> RawValue {
    let raw = raw.trim();
    match raw.parse::<f64>() {
        Ok(v) => RawValue::parsed(raw.into(), v, unit),
        Err(_) => {
            report.err(line_no, format!("{what} for {sat}: unconvertible value '{raw}', kept in place"));
            RawValue::invalid(raw.into(), unit, format!("{what} not parseable as float"))
        }
    }
}

/// Deterministic fingerprint of a raw bundle, used for provenance/dedup.
pub fn fingerprint_bundle(b: &RawBundle) -> String {
    let json = serde_json::to_string(b).expect("RawBundle is serializable");
    short_hash(&json)
}
