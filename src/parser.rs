//! Small, strict RINEX 3 observation-file parser covering the exact subset the
//! field team uses: C/L/D/S on GPS L1C / L2W, epoch flags, receiver clock
//! offset and the headers we persist.
//!
//! The rule is "store the epoch as presented": the numeric slot in a field is
//! kept verbatim; conversion to SI is the job of the derived normalization
//! layer, and a value that cannot convert stays in place with a diagnostic.

use crate::model::*;

pub fn parse_rinex_obs(text: &str) -> Result<RinexFile, Vec<ImportDiagnostic>> {
    let mut errors: Vec<ImportDiagnostic> = Vec::new();
    let mut warnings: Vec<ImportDiagnostic> = Vec::new();
    let mut header = RinexHeader::default();
    let mut epochs: Vec<EpochRecord> = Vec::new();
    let mut seq: i64 = 0;

    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
    let mut idx = 0;
    let mut seen_end_header = false;

    while idx < lines.len() {
        let (lineno, line) = lines[idx];
        idx += 1;
        if line.len() < 60 {
            if line.trim().is_empty() {
                continue;
            }
            errors.push(fatal(
                lineno,
                "SHORT_LINE",
                format!("line shorter than 60 columns: {line:?}"),
            ));
            continue;
        }
        let label = line[60..].trim();
        match label {
            "RINEX VERSION / TYPE" => {
                header.version = line[..9].trim().to_string();
                let file_type = line[20..21].trim();
                if file_type != "O" {
                    errors.push(fatal(
                        lineno,
                        "NOT_OBSERVATION_FILE",
                        format!("file type {file_type:?} is not observation 'O'"),
                    ));
                }
                let sys = line[40..41].trim();
                if sys != "G" && sys != "M" {
                    errors.push(fatal(
                        lineno,
                        "UNSUPPORTED_SATELLITE_SYSTEM",
                        format!("system {sys:?} (case package is GPS/MIXED)"),
                    ));
                }
            }
            "MARKER NAME" => header.marker_name = line[..60].trim().to_string(),
            "APPROX POSITION XYZ" => {
                for (k, col) in [0..14, 14..28, 28..42].iter().enumerate() {
                    header.approx_position_xyz_m[k] = line[col.clone()].trim().to_string();
                }
            }
            "SYS / # / OBS TYPES" => {
                let sys = line[0..3].trim().to_string();
                let n: usize = line[3..6].trim().parse().map_err(|_| {
                    fatal(lineno, "BAD_SYS_OBS_COUNT", "cannot parse obs type count")
                })?;
                let mut types = Vec::new();
                for i in 0..n {
                    let start = 7 + i * 4;
                    let end = start + 3;
                    if end > 60 {
                        errors.push(fatal(
                            lineno,
                            "SYS_OBS_OVERFLOW",
                            format!("obs type index {i} crosses header boundary"),
                        ));
                        continue;
                    }
                    let t = line[start..end].trim().to_string();
                    if t.is_empty() {
                        errors.push(fatal(
                            lineno,
                            "SYS_OBS_BLANK",
                            format!("blank obs type at index {i}"),
                        ));
                    }
                    types.push(t);
                }
                header.sys_obs.push((sys, types));
            }
            "TIME OF FIRST OBS" => header.time_system = line[48..60].trim().to_string(),
            "END OF HEADER" => seen_end_header = true,
            other if other.is_empty() || other.starts_with("PGM") || other.starts_with("COMMENT") => {}
            other => {
                // Unknown header lines are ignored but the data section has a
                // strict format.
                if seen_end_header {
                    errors.push(fatal(
                        lineno,
                        "UNEXPECTED_HEADER_AFTER_END",
                        format!("header label {other:?} after END OF HEADER"),
                    ));
                }
            }
        }
        if seen_end_header {
            break;
        }
    }

    if !seen_end_header {
        errors.push(fatal(0, "MISSING_END_OF_HEADER", "no END OF HEADER found"));
    }
    let mut known_types: Vec<String> = Vec::new();
    for (sys, types) in &header.sys_obs {
        if sys == "G" || sys == "M" {
            known_types = types.clone();
        }
    }
    if known_types.is_empty() {
        errors.push(fatal(0, "MISSING_SYS_OBS", "no GPS SYS / # / OBS TYPES header"));
    }

    // Data section.
    while idx < lines.len() {
        let (lineno, line) = lines[idx];
        if line.trim().is_empty() {
            idx += 1;
            continue;
        }
        if line.len() < 32 {
            errors.push(fatal(lineno, "BAD_EPOCH_LINE", "epoch line too short"));
            idx += 1;
            continue;
        }
        let epoch_flag: u8 = match line[31..32].trim().parse() {
            Ok(v) => v,
            Err(_) => {
                errors.push(fatal(
                    lineno,
                    "BAD_EPOCH_FLAG",
                    format!("epoch flag {:?}", &line[31..32]),
                ));
                idx += 1;
                continue;
            }
        };
        if epoch_flag > 1 {
            errors.push(fatal(
                lineno,
                "UNSUPPORTED_EPOCH_FLAG",
                format!("epoch flag {epoch_flag} (only 0/1 supported by this case)"),
            ));
            // event / moving antenna records are still skipped structurally
            let nsat: usize = line[29..32].trim().parse().unwrap_or(0);
            idx += 1 + nsat.div_ceil(12).max(1);
            continue;
        }
        let nsat: usize = match line[29..32].trim().parse() {
            Ok(v) => v,
            Err(_) => {
                errors.push(fatal(lineno, "BAD_NSAT", "cannot parse satellite count"));
                idx += 1;
                continue;
            }
        };
        let year: i32 = match line[1..4].trim().parse() {
            Ok(v) => v,
            Err(_) => {
                errors.push(fatal(lineno, "BAD_EPOCH_DATE", "cannot parse year"));
                idx += 1;
                continue;
            }
        };
        let month: u32 = line[6..9].trim().parse().unwrap_or(0);
        let day: u32 = line[9..12].trim().parse().unwrap_or(0);
        let hour: u32 = line[12..15].trim().parse().unwrap_or(0);
        let minute: u32 = line[15..18].trim().parse().unwrap_or(0);
        let second: f64 = line[18..29].trim().parse().unwrap_or(f64::NAN);
        if second.is_nan() {
            errors.push(fatal(lineno, "BAD_EPOCH_SECOND", "cannot parse seconds"));
            idx += 1;
            continue;
        }
        let full_year = if year < 80 { 2000 + year } else { 1900 + year };
        let epoch = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:09.6}Z",
            full_year,
            month,
            day,
            hour,
            minute,
            second
        );
        let clock = if line.len() >= 80 {
            line[68..80].trim().to_string()
        } else {
            String::new()
        };
        if !clock.is_empty() && clock.parse::<f64>().is_err() {
            warnings.push(ImportDiagnostic {
                severity: "warning",
                code: "UNCONVERTIBLE_CLOCK_OFFSET",
                line: Some(lineno),
                message: format!("receiver clock offset {clock:?} kept verbatim"),
            });
        }
        idx += 1;
        seq += 1;
        let mut epoch_record = EpochRecord {
            seq,
            epoch,
            recv_clock_offset_s: clock,
            obs: Vec::new(),
        };

        // Satellite id continuation lines live in columns 32.. of the epoch
        // line and subsequent 32-char lines.
        let mut sat_ids: Vec<String> = Vec::new();
        for i in 0..nsat {
            let s = if i < 12 {
                line[32 + i * 3..35 + i * 3].trim().to_string()
            } else {
                let cont_idx = idx + (i / 12) - 1;
                if cont_idx < lines.len() {
                    let col = (i % 12) * 3;
                    lines[cont_idx].1[col..col + 3].trim().to_string()
                } else {
                    String::new()
                }
            };
            sat_ids.push(s);
        }
        if nsat > 12 {
            idx += nsat.div_ceil(12) - 1;
        }

        for sat in &sat_ids {
            if !(sat.starts_with('G') && sat.len() == 3) {
                errors.push(fatal(
                    lineno,
                    "UNSUPPORTED_SATELLITE_ID",
                    format!("satellite id {sat:?} (GPS Gnn only)"),
                ));
                continue;
            }
            let (slineno, sline) = if idx < lines.len() {
                (lines[idx].0, lines[idx].1)
            } else {
                errors.push(fatal(lineno, "TRUNCATED_SATELLITE_LINE", format!("missing line for {sat}")));
                continue;
            };
            idx += 1;
            for (i, code) in known_types.iter().enumerate() {
                let start = i * 16;
                let end = start + 16;
                if sline.len() < end {
                    errors.push(fatal(
                        slineno,
                        "OBS_LINE_TOO_SHORT",
                        format!("{sat} {code}: need {end} columns, got {}", sline.len()),
                    ));
                    continue;
                }
                let field = sline[start..end].to_string();
                let raw_value = field[..14].trim().to_string();
                let lli_s = field[14..15].trim().to_string();
                let ssi_s = field[15..16].trim().to_string();
                if raw_value.is_empty() {
                    continue; // genuine frequency/measurement gap, nothing stored
                }
                let kind = classify_code(code);
                let band = match band_frequency_hz(code.as_str()) {
                    Some((b, _)) => b.to_string(),
                    None => {
                        errors.push(fatal(
                            slineno,
                            "UNSUPPORTED_OBS_CODE",
                            format!("obs code {code:?} has no known frequency number"),
                        ));
                        continue;
                    }
                };
                let (_, freq_hz) = band_frequency_hz(code.as_str()).unwrap();
                let mut diagnostic = None;
                if raw_value.parse::<f64>().is_err() {
                    diagnostic = Some(ParseDiagnostic {
                        code: "UNCONVERTIBLE_VALUE",
                        message: format!("value {raw_value:?} for {sat} {code} kept verbatim; no SI row derived"),
                        raw: raw_value.clone(),
                    });
                    warnings.push(ImportDiagnostic {
                        severity: "warning",
                        code: "UNCONVERTIBLE_VALUE",
                        line: Some(slineno),
                        message: diagnostic.as_ref().unwrap().message.clone(),
                    });
                }
                let lli = if lli_s.is_empty() {
                    None
                } else {
                    match lli_s.parse::<u8>() {
                        Ok(v) => Some(v),
                        Err(_) => {
                            warnings.push(ImportDiagnostic {
                                severity: "warning",
                                code: "UNCONVERTIBLE_LLI",
                                line: Some(slineno),
                                message: format!("LLI flag {lli_s:?} for {sat} {code} kept verbatim"),
                            });
                            None
                        }
                    }
                };
                let ssi_raw = if ssi_s.is_empty() { None } else { Some(ssi_s) };
                epoch_record.obs.push(RawObs {
                    sat: sat.clone(),
                    code: code.clone(),
                    band,
                    freq_hz,
                    raw_field: field.clone(),
                    raw_value,
                    lli,
                    ssi_raw,
                    kind: if diagnostic.is_some() { ObsKind::Unconvertible } else { kind },
                    diagnostic,
                });
            }
        }
        epochs.push(epoch_record);
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(RinexFile {
        header,
        epochs,
        diagnostics: warnings,
    })
}

fn fatal(line: usize, code: &str, message: impl Into<String>) -> ImportDiagnostic {
    ImportDiagnostic {
        severity: "error",
        code: code.to_string(),
        line: Some(line),
        message: message.into(),
    }
}

fn classify_code(code: &str) -> ObsKind {
    match code.chars().next() {
        Some('C') => ObsKind::Pseudorange,
        Some('L') => ObsKind::Carrier,
        Some('D') => ObsKind::Doppler,
        Some('S') => ObsKind::Ssi,
        _ => ObsKind::Unconvertible,
    }
}
