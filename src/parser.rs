//! RINEX 2.11 observation parser (subset used by the offline case pack) plus
//! the two companion text formats: satellite state/frequency table and
//! receiver clock-jump events.
//!
//! Original textual representation is always preserved. Fields that cannot be
//! converted to numbers remain in place with a diagnostic and `None` value.

use crate::error::{DeskError, Result};
use crate::model::*;
use crate::units::{Quantity, Unit};

pub fn parse_obs_file(text: &str) -> Result<ParsedObsFile> {
    let lines: Vec<&str> = text.lines().collect();
    let mut marker_name = String::new();
    let mut receiver = String::new();
    let mut antenna = String::new();
    let mut obs_codes: Vec<ObsCode> = Vec::new();
    let mut header_raw: Vec<String> = Vec::new();
    let mut diagnostics: Vec<ParseDiagnostic> = Vec::new();
    let mut epochs: Vec<RawEpoch> = Vec::new();

    let n = lines.len();
    let mut i = 0usize;
    while i < n && !lines[i].get(60..).unwrap_or("").trim().starts_with("END OF HEADER") {
        let line = lines[i];
        header_raw.push(line.to_string());
        let label = line.get(60..).unwrap_or("").trim();
        if label.starts_with("# / TYPES OF OBSERV")
            || (label.is_empty() && has_obs_tokens(line))
        {
            for c in scan_obs_codes(line) {
                if !obs_codes.contains(&c) {
                    obs_codes.push(c);
                }
            }
        } else if label.starts_with("MARKER NAME") {
            marker_name = line.get(..60).unwrap_or("").trim().to_string();
        } else if label.starts_with("RECEIVER NUMBER / TYPE / VERS") {
            receiver = line.get(..60).unwrap_or("").trim().to_string();
        } else if label.starts_with("ANT # / TYPE") {
            antenna = line.get(..60).unwrap_or("").trim().to_string();
        }
        i += 1;
    }
    if i < n {
        header_raw.push(lines[i].to_string());
        i += 1;
    } else {
        return Err(DeskError::Parse {
            location: "header".into(),
            message: "not a RINEX observation file: END OF HEADER missing".into(),
        });
    }

    while i < n {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !looks_like_epoch_header(line) {
            diagnostics.push(ParseDiagnostic {
                location: format!("line {}", i + 1),
                code: "E007".into(),
                message: "unparsed content between epochs".into(),
                raw: line.to_string(),
                severity: "warning".into(),
            });
            i += 1;
            continue;
        }
        let start_line = i;
        let head = line;
        let mut sat_area = head.get(..68).unwrap_or(head).to_string();
        let mut recv_offset: Option<String> =
            head.get(68..80).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let want = declared_sat_count(head).unwrap_or(0);
        i += 1;
        while count_sat_tokens(&sat_area) < want && i < n && is_sat_continuation(lines[i]) {
            sat_area.push(' ');
            sat_area.push_str(lines[i].get(..68).unwrap_or("").trim_end());
            if recv_offset.is_none() {
                recv_offset = lines[i]
                    .get(68..80)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
            }
            i += 1;
        }

        let sats = scan_sats(&sat_area);
        if sats.len() != want {
            diagnostics.push(ParseDiagnostic {
                location: format!("line {}", start_line + 1),
                code: "E002".into(),
                message: format!(
                    "epoch header declares {want} satellites but found {}",
                    sats.len()
                ),
                raw: head.to_string(),
                severity: "error".into(),
            });
        }
        let epoch_index = epochs.len() as u64;
        let mut records = Vec::with_capacity(sats.len());
        let lines_per_sat = obs_codes.len().div_ceil(5).max(1);
        for sat in &sats {
            let mut chunks: Vec<String> = Vec::new();
            for _ in 0..lines_per_sat {
                if i >= n {
                    diagnostics.push(ParseDiagnostic {
                        location: format!("line {}", i + 1),
                        code: "E006".into(),
                        message: format!(
                            "observation lines truncated for satellite {}",
                            sat.raw()
                        ),
                        raw: String::new(),
                        severity: "error".into(),
                    });
                    break;
                }
                let ol = lines[i].trim_end_matches('\r');
                i += 1;
                chunks.extend(split_obs_fields(ol));
            }
            let mut slots = Vec::with_capacity(obs_codes.len());
            for (col, code) in obs_codes.iter().enumerate() {
                let chunk = chunks.get(col).cloned().unwrap_or_default();
                slots.push(parse_slot(chunk, code.clone(), col, &mut diagnostics, i));
            }
            records.push(EpochRecord {
                sat: sat.clone(),
                slots,
                receiver_clock_offset_s: recv_offset.clone(),
            });
        }

        match parse_epoch_head_fields(head, epoch_index, start_line + 1, sats, records) {
            Ok(mut e) => {
                e.receiver_clock_offset_s = recv_offset;
                e.location = format!("{}-{}", start_line + 1, i);
                epochs.push(e);
            }
            Err(d) => diagnostics.push(d),
        }
    }

    Ok(ParsedObsFile {
        marker_name,
        receiver,
        antenna,
        header_raw,
        obs_codes,
        epochs,
        diagnostics,
    })
}

fn has_obs_tokens(line: &str) -> bool {
    let area = line.get(..60).unwrap_or(line);
    let mut seen = 0;
    for tok in area.split_whitespace() {
        if let Some(c) = ObsCode::parse(tok) {
            let _ = c;
            seen += 1;
        }
    }
    seen > 0
}

fn scan_obs_codes(line: &str) -> Vec<ObsCode> {
    let area = line.get(..60).unwrap_or(line);
    let mut out = Vec::new();
    let mut toks: Vec<&str> = area.split_whitespace().collect();
    if let Some(first) = toks.first() {
        if first.parse::<i64>().is_ok() {
            toks.remove(0);
        }
    }
    for tok in toks {
        if let Some(c) = ObsCode::parse(tok) {
            out.push(c);
        }
    }
    out
}

fn looks_like_epoch_header(line: &str) -> bool {
    if line.len() < 30 {
        return false;
    }
    match (line[0..1].parse::<i32>(), line[1..3].trim().parse::<u32>()) {
        (Ok(_), Ok(_)) => {}
        _ => return false,
    }
    line.get(29..32).map(str::trim).and_then(|s| s.parse::<u32>().ok()).is_some()
}

fn declared_sat_count(line: &str) -> Option<usize> {
    line.get(29..32).map(str::trim).and_then(|s| s.parse::<usize>().ok())
}

fn is_sat_continuation(line: &str) -> bool {
    let area = line.get(..60).unwrap_or(line);
    area.split_whitespace()
        .all(|tok| SatId::parse(tok).is_some() || tok.parse::<f64>().is_ok() == false)
        && area.split_whitespace().any(|tok| SatId::parse(tok).is_some())
}

fn count_sat_tokens(area: &str) -> usize {
    scan_sats(area).len()
}

fn scan_sats(area: &str) -> Vec<SatId> {
    // Sat ids live in columns 32..68 of the epoch header / continuation:
    // 3-char blocks Gpp etc.
    let bytes = area.as_bytes();
    let mut out = Vec::new();
    let mut col = 32usize;
    while col + 3 <= bytes.len().min(68) {
        let tok = &area[col..col + 3];
        if let Some(s) = SatId::parse(tok) {
            out.push(s);
        }
        col += 3;
    }
    out
}

fn parse_epoch_head_fields(
    line: &str,
    epoch_index: u64,
    line_no: usize,
    sats: Vec<SatId>,
    records: Vec<EpochRecord>,
) -> std::result::Result<RawEpoch, ParseDiagnostic> {
    let take = |a: usize, b: usize| line.get(a..b).unwrap_or("").trim();
    let year: i32 = take(1, 3).parse::<i32>().map_err(|_| ParseDiagnostic {
        location: format!("line {line_no}"),
        code: "E002".into(),
        message: "bad epoch year".into(),
        raw: line.to_string(),
        severity: "error".into(),
    })? + 2000;
    let month: u32 = take(3, 6).parse::<u32>().map_err(|_| ParseDiagnostic {
        location: format!("line {line_no}"),
        code: "E002".into(),
        message: "bad epoch month".into(),
        raw: line.to_string(),
        severity: "error".into(),
    })?;
    let day: u32 = take(6, 9).parse::<u32>().map_err(|_| ParseDiagnostic {
        location: format!("line {line_no}"),
        code: "E002".into(),
        message: "bad epoch day".into(),
        raw: line.to_string(),
        severity: "error".into(),
    })?;
    let hour: u32 = take(9, 12).parse::<u32>().map_err(|_| ParseDiagnostic {
        location: format!("line {line_no}"),
        code: "E002".into(),
        message: "bad epoch hour".into(),
        raw: line.to_string(),
        severity: "error".into(),
    })?;
    let minute: u32 = take(12, 15).parse::<u32>().map_err(|_| ParseDiagnostic {
        location: format!("line {line_no}"),
        code: "E002".into(),
        message: "bad epoch minute".into(),
        raw: line.to_string(),
        severity: "error".into(),
    })?;
    let second: f64 = take(15, 26).parse::<f64>().map_err(|_| ParseDiagnostic {
        location: format!("line {line_no}"),
        code: "E002".into(),
        message: "bad epoch seconds".into(),
        raw: line.to_string(),
        severity: "error".into(),
    })?;
    let epoch_flag = take(26,29).to_string();
    Ok(RawEpoch {
        epoch_index,
        location: format!("line {line_no}"),
        year,
        month,
        day,
        hour,
        minute,
        second,
        epoch_flag,
        records: {
            let _ = sats;
            records
        },
        receiver_clock_offset_s: None,
    })
}

/// Split a RINEX observation line into 16-character field strings.
fn split_obs_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < line.len() {
        let end = (i + 16).min(line.len());
        let chunk = line[i..end].to_string();
        if chunk.trim().is_empty() {
            out.push(String::new());
        } else {
            out.push(chunk);
        }
        i += 16;
    }
    out
}

fn parse_slot(
    chunk: String,
    code: ObsCode,
    column: usize,
    diags: &mut Vec<ParseDiagnostic>,
    current_line: usize,
) -> ObsSlot {
    let missing = chunk.trim().is_empty();
    let value_raw: String = chunk.get(..14).unwrap_or("").trim().to_string();
    let lli = chunk
        .get(14..15)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let signal_strength = chunk
        .get(15..16)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let unit = match code.kind_name() {
        KIND_CODE => Unit::Meters,
        KIND_PHASE => Unit::Cycles,
        KIND_DOPPLER => Unit::CyclesPerSecond,
        KIND_SNR => Unit::DbHz,
        _ => Unit::Dimensionless,
    };
    let mut diagnostic = None;
    let quantity = if value_raw.is_empty() {
        Quantity::raw_only(unit, "")
    } else {
        match value_raw.parse::<f64>() {
            Ok(v) => Quantity::parsed(v, unit, value_raw.clone()),
            Err(_) => {
                let d = format!(
                    "non-numeric observation value '{}' for code {} kept in place",
                    value_raw, code.raw
                );
                diagnostic = Some(d.clone());
                diags.push(ParseDiagnostic {
                    location: format!("line {}", current_line),
                    code: "E005".into(),
                    message: d,
                    raw: value_raw.clone(),
                    severity: "warning".into(),
                });
                Quantity::raw_only(unit, value_raw)
            }
        }
    };
    ObsSlot { code, quantity, lli, signal_strength, missing, column, diagnostic }
}

#[derive(Debug, Clone)]
pub struct ParsedTable<T> {
    pub rows: Vec<T>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

/// Satellite state table (companion to a RINEX obs file):
/// `# satellite-state/v1`
/// `sat band obs_code frequency_hz wavelength_m note`
pub fn parse_sat_states(text: &str) -> Result<ParsedTable<SatStateRow>> {
    let mut lines = text.lines();
    let header = lines
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| DeskError::Parse {
            location: "sat-state".into(),
            message: "empty satellite state file".into(),
        })?;
    if !header.trim().starts_with("# satellite-state/v1") {
        return Err(DeskError::Parse {
            location: "sat-state".into(),
            message: format!("expected '# satellite-state/v1' header, got '{header}'"),
        });
    }
    let mut rows = Vec::new();
    let mut diagnostics = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.splitn(6, '\t').collect();
        if cols.len() < 4 {
            diagnostics.push(ParseDiagnostic {
                location: format!("sat-state line {idx}"),
                code: "S001".into(),
                message: "expected at least 4 tab-separated columns: sat band code frequency_hz".into(),
                raw: line.to_string(),
                severity: "error".into(),
            });
            continue;
        }
        let sat = match SatId::parse(cols[0]) {
            Some(s) => s,
            None => {
                diagnostics.push(ParseDiagnostic {
                    location: format!("sat-state line {idx}"),
                    code: "S002".into(),
                    message: format!("bad satellite id '{}'", cols[0]),
                    raw: cols[0].to_string(),
                    severity: "error".into(),
                });
                continue;
            }
        };
        let freq_raw = cols[3].trim().to_string();
        let frequency_hz = match freq_raw.parse::<f64>() {
            Ok(v) => Quantity::parsed(v, Unit::Hertz, freq_raw.clone()),
            Err(_) => {
                diagnostics.push(ParseDiagnostic {
                    location: format!("sat-state line {idx}"),
                    code: "S003".into(),
                    message: format!("non-numeric frequency_hz '{freq_raw}' kept in place"),
                    raw: freq_raw.clone(),
                    severity: "warning".into(),
                });
                Quantity::raw_only(Unit::Hertz, freq_raw)
            }
        };
        let wavelength_m = cols.get(4).copied().map(str::trim).filter(|s| !s.is_empty()).map(|w: &str| {
            match w.parse::<f64>() {
                Ok(v) => Quantity::parsed(v, Unit::Meters, (*w).to_string()),
                Err(_) => {
                    diagnostics.push(ParseDiagnostic {
                        location: format!("sat-state line {idx}"),
                        code: "S004".into(),
                        message: format!("non-numeric wavelength_m '{w}' kept in place"),
                        raw: (*w).to_string(),
                        severity: "warning".into(),
                    });
                    Quantity::raw_only(Unit::Meters, (*w).to_string())
                }
            }
        });
        rows.push(SatStateRow {
            sat,
            band: cols[1].trim().to_string(),
            code: cols[2].trim().to_string(),
            frequency_hz,
            wavelength_m,
            note: cols.get(5).map(|s| s.trim().to_string()),
            location: format!("sat-state line {idx}"),
        });
    }
    Ok(ParsedTable { rows, diagnostics })
}

/// Receiver clock events (companion file):
/// `# clock-events/v1`
/// `epoch_index jump_ns kind note`
pub fn parse_clock_events(text: &str) -> Result<ParsedTable<ClockEvent>> {
    let mut lines = text.lines();
    let header = lines.find(|l| !l.trim().is_empty()).ok_or_else(|| DeskError::Parse {
        location: "clock-events".into(),
        message: "empty clock events file".into(),
    })?;
    if !header.trim().starts_with("# clock-events/v1") {
        return Err(DeskError::Parse {
            location: "clock-events".into(),
            message: format!("expected '# clock-events/v1' header, got '{header}'"),
        });
    }
    let mut rows = Vec::new();
    let mut diagnostics = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.splitn(4, '\t').collect();
        if cols.len() < 3 {
            diagnostics.push(ParseDiagnostic {
                location: format!("clock-events line {idx}"),
                code: "C001".into(),
                message: "expected 3 tab-separated columns: epoch_index jump_ns kind".into(),
                raw: line.to_string(),
                severity: "error".into(),
            });
            continue;
        }
        let epoch_index = match cols[0].trim().parse::<i64>() {
            Ok(v) if v >= 0 => v as u64,
            _ => {
                diagnostics.push(ParseDiagnostic {
                    location: format!("clock-events line {idx}"),
                    code: "C002".into(),
                    message: format!("bad epoch_index '{}'", cols[0]),
                    raw: cols[0].to_string(),
                    severity: "error".into(),
                });
                continue;
            }
        };
        let jump_raw = cols[1].trim().to_string();
        let jump_ns = match jump_raw.parse::<f64>() {
            Ok(v) => Quantity::parsed(v, Unit::Nanoseconds, jump_raw.clone()),
            Err(_) => {
                diagnostics.push(ParseDiagnostic {
                    location: format!("clock-events line {idx}"),
                    code: "C003".into(),
                    message: format!("non-numeric jump_ns '{jump_raw}' kept in place"),
                    raw: jump_raw.clone(),
                    severity: "warning".into(),
                });
                Quantity::raw_only(Unit::Nanoseconds, jump_raw)
            }
        };
        rows.push(ClockEvent {
            event_index: rows.len() as u64,
            location: format!("clock-events line {idx}"),
            epoch_index,
            jump_ns,
            kind: cols[2].trim().to_string(),
            raw: line.to_string(),
        });
    }
    Ok(ParsedTable { rows, diagnostics })
}
