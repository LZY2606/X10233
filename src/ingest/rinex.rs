//! Minimal, strict RINEX 3.xx observation parser (subset).
//!
//! Supported headers: `RINEX VERSION / TYPE`, `# / TYPES OF OBSERV`,
//! `MARKER NAME`, `APPROX POSITION XYZ`, `TIME OF FIRST OBS`, `END OF HEADER`.
//! Body: `> yyyy mm dd hh mm ss.ffff flag nr [sv ...] [rcv clk offset]`
//! followed by one line per listed SV with 16-char fields per type:
//! value(14) LLI(1) SSI(1).

use super::{gps_frequency_hz, ssi_to_dbhz, IngestOutcome};
use crate::event::{FormatError, RawBatch};
use crate::model::{ObsKind, Observation, Reference, SatState, Value, Id};

#[allow(clippy::too_many_arguments)]
fn obs_id(
    project_id: &Id,
    station: &str,
    epoch_raw: &str,
    sv: &str,
    code: &str,
) -> Id {
    crate::model::stable_id(
        "obs",
        &format!("{}|{}|{}|{}|{}", project_id, station, epoch_raw, sv, code),
    )
}

fn sat_id(project_id: &Id, station: &str, epoch_raw: &str, sv: &str) -> Id {
    crate::model::stable_id("sat", &format!("{}|{}|{}|{}", project_id, station, epoch_raw, sv))
}

/// Seconds since GPS epoch (1980-01-06), UTC calendar, no leap-second handling
/// — appropriate for the synthetic bench; preserved as an explicit reference.
fn gps_seconds(y: i32, m: u32, d: u32, hh: u32, mm: u32, sec: f64) -> Option<f64> {
    use chrono::{NaiveDate, NaiveTime};
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let sec_int = sec.floor() as u32;
    let time = NaiveTime::from_hms_milli_opt(
        hh,
        mm,
        sec_int,
        ((sec - sec.floor()) * 1000.0).round() as u32,
    )?;
    let dt = date.and_time(time);
    let gps0 = NaiveDate::from_ymd_opt(1980, 1, 6)?
        .and_hms_opt(0, 0, 0)
        .unwrap();
    Some((dt - gps0).num_seconds() as f64 + (sec - sec.floor()))
}

fn parse_epoch(raw: &str) -> Result<(String, Option<f64>), String> {
    let toks: Vec<&str> = raw.split_whitespace().collect();
    if toks.len() < 7 {
        return Err(format!("epoch line needs 7 date/time tokens, got {raw:?}"));
    }
    let mut nums = [0i32; 7];
    for (i, t) in toks[..6].iter().enumerate() {
        nums[i] = t
            .parse()
            .map_err(|_| format!("bad epoch component {t:?}"))?;
    }
    let sec: f64 = toks[5].parse().map_err(|_| format!("bad seconds {toks:?}"))?;
    let norm = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:07.4}",
        nums[0], nums[1], nums[2], nums[3], nums[4], sec
    );
    let g = gps_seconds(
        nums[0],
        nums[1] as u32,
        nums[2] as u32,
        nums[3] as u32,
        nums[4] as u32,
        sec,
    );
    Ok((norm, g))
}

pub fn parse_rinex(
    project_id: &Id,
    parent_batch_id: Option<Id>,
    source_name: &str,
    text: &str,
) -> Result<IngestOutcome, String> {
    let mut station = String::from("STATION");
    let mut types: Vec<String> = vec![];
    let mut seen_header_end = false;
    let mut format_errors: Vec<FormatError> = vec![];
    let mut observations: Vec<Observation> = vec![];
    let mut sat_states: Vec<SatState> = vec![];

    // --- headers ---
    let header: Vec<&str> = text.lines().collect();
    let mut version: Option<f64> = None;
    let mut header_end_idx = 0;
    for (idx, raw_line) in header.iter().enumerate() {
        let label = raw_line.get(60..).unwrap_or("").trim();
        let body = raw_line.get(..60).unwrap_or(raw_line);
        match label {
            "RINEX VERSION / TYPE" => {
                let v = body
                    .get(..9)
                    .unwrap_or("")
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "invalid RINEX version".to_string())?;
                if !(3.0..4.0).contains(&v) {
                    return Err(format!(
                        "only RINEX 3 subset supported, found version {v}"
                    ));
                }
                if body.get(20..21).unwrap_or(" ").trim() != "O" {
                    return Err("only OBSERVATION DATA files accepted".to_string());
                }
                version = Some(v);
            }
            "# / TYPES OF OBSERV" => {
                let count: usize = body
                    .get(..6)
                    .unwrap_or("")
                    .trim()
                    .parse()
                    .map_err(|_| "bad observation type count".to_string())?;
                for i in 0..count {
                    let start = 6 + i * 4;
                    let code = body.get(start..start + 4).unwrap_or("").trim().to_string();
                    if code.len() != 3 || !"CLDS".contains(code.chars().next().unwrap_or(' ')) {
                        return Err(format!("bad observation code {code:?}"));
                    }
                    types.push(code);
                }
            }
            "MARKER NAME" => {
                station = body.trim().to_string();
            }
            "END OF HEADER" => {
                seen_header_end = true;
                header_end_idx = idx;
                break;
            }
            _ => {}
        }
    }
    if version.is_none() {
        return Err("missing RINEX VERSION / TYPE header".to_string());
    }
    if !seen_header_end {
        return Err("missing END OF HEADER".to_string());
    }
    if types.is_empty() {
        return Err("no # / TYPES OF OBSERV declared".to_string());
    }

    // --- body ---
    let body_lines: Vec<&str> = header[header_end_idx + 1..].to_vec();
    let mut i = 0;
    while i < body_lines.len() {
        let line = body_lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !line.starts_with('>') {
            format_errors.push(FormatError {
                line_no: Some(i + header_end_idx + 2),
                raw_line: line.to_string(),
                code: "expected-epoch".to_string(),
                message: "body line outside an epoch (must start with >)".to_string(),
            });
            i += 1;
            continue;
        }
        let epoch_spec = line.trim_start_matches('>').trim();
        let toks: Vec<&str> = epoch_spec.split_whitespace().collect();
        if toks.len() < 8 {
            return Err(format!("malformed epoch record: {line:?}"));
        }
        let (epoch_raw, gps) = match parse_epoch(epoch_spec) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        let _flag: i32 = toks[6]
            .parse()
            .map_err(|_| format!("bad epoch flag in {line:?}"))?;
        let n_sat: usize = toks[7]
            .parse()
            .map_err(|_| format!("bad satellite count in {line:?}"))?;
        let sats: Vec<String> = toks[8..8 + n_sat].iter().map(|s| (*s).to_string()).collect();
        if sats.len() != n_sat {
            return Err("satellite list shorter than declared count".to_string());
        }

        for sat in &sats {
            i += 1;
            if i >= body_lines.len() {
                return Err(format!("missing observation record for {sat}"));
            }
            let rec = body_lines[i];
            // missing frequency -> entirely blank 16-char field (allowed)
            for (ti, code) in types.iter().enumerate() {
                let start = ti * 16;
                let field = rec.get(start..start + 16).unwrap_or("");
                let raw_field = field.to_string();
                let value_part = field.get(..14).unwrap_or("").trim().to_string();
                let lli = field
                    .get(14..15)
                    .unwrap_or(" ")
                    .trim()
                    .parse()
                    .unwrap_or(0);
                let ssi = field
                    .get(15..16)
                    .unwrap_or(" ")
                    .trim()
                    .parse()
                    .unwrap_or(0);

                if value_part.is_empty() {
                    // frequency missing at this epoch: retain a diagnostic row
                    let band = code_chars(code).1;
                    observations.push(Observation {
                        id: obs_id(project_id, &station, &epoch_raw, sat, code),
                        station: station.clone(),
                        epoch_raw: epoch_raw.clone(),
                        gps_seconds: gps,
                        sv: sat.clone(),
                        code: code.clone(),
                        kind: code_chars(code).0,
                        band,
                        frequency_hz: gps_frequency_hz(band),
                        lli,
                        ssi,
                        value: Value::unparseable(
                            raw_field.clone(),
                            Reference::Metre,
                            "missing-frequency: blank RINEX field",
                        ),
                    });
                    continue;
                }
                let parsed: Result<f64, _> = value_part.parse();
                let (kind, band, reference) = code_parts(code)?;
                let value = match parsed {
                    Ok(v) => Value::parsed(value_part.clone(), v, reference),
                    Err(_) => Value::unparseable(
                        value_part.clone(),
                        reference,
                        "non-numeric observation value",
                    ),
                };
                observations.push(Observation {
                    id: obs_id(project_id, &station, &epoch_raw, sat, code),
                    station: station.clone(),
                    epoch_raw: epoch_raw.clone(),
                    gps_seconds: gps,
                    sv: sat.clone(),
                    code: code.clone(),
                    kind,
                    band,
                    frequency_hz: gps_frequency_hz(band),
                    lli,
                    ssi,
                    value,
                });
            }
            sat_states.push(SatState {
                id: sat_id(project_id, &station, &epoch_raw, sat),
                station: station.clone(),
                epoch_raw: epoch_raw.clone(),
                gps_seconds: gps,
                sv: sat.clone(),
                azimuth_deg: None,
                elevation_deg: None,
                x_m: None,
                y_m: None,
                z_m: None,
                diagnostics: vec!["rinex-mini: state supplied via JSON sheet".to_string()],
            });
        }
        i += 1;
    }

    let batch = RawBatch {
        batch_id: crate::model::stable_id(
            "batch",
            &format!("{}|{}|{}", project_id, source_name, observations.len()),
        ),
        parent_batch_id,
        source_name: source_name.to_string(),
        input_fragment: text.to_string(),
        observations,
        sat_states,
        declared_clock_events: vec![],
        format_errors,
    };
    Ok(IngestOutcome { batch, warnings: vec![] })
}

fn code_parts(code: &str) -> Result<(ObsKind, u8, Reference), String> {
    let (kind, band) = code_chars(code);
    let reference = match kind {
        ObsKind::Pseudorange => Reference::Metre,
        ObsKind::CarrierPhase => Reference::Cycle,
        ObsKind::Doppler => Reference::MetrePerSecond,
        ObsKind::Snr => Reference::DbHz,
    };
    Ok((kind, band, reference))
}

pub(crate) fn code_chars(code: &str) -> (ObsKind, u8) {
    let mut chars = code.chars();
    let lead = chars.next().unwrap_or(' ');
    let band = chars.next().unwrap_or('0').to_digit(10).unwrap_or(0) as u8;
    let kind = match lead {
        'C' => ObsKind::Pseudorange,
        'L' => ObsKind::CarrierPhase,
        'D' => ObsKind::Doppler,
        'S' => ObsKind::Snr,
        _ => ObsKind::Pseudorange,
    };
    (kind, band)
}

// keep ssi import referenced for the S-type semantics documentation
#[allow(dead_code)]
fn _ssi_doc() -> Option<f64> {
    ssi_to_dbhz(5)
}
