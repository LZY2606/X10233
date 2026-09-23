//! RINEX 3.03 observation subset parser plus satellite-state/clock sidecars.
//!
//! Raw fidelity contract: every cell keeps its exact 14-character text, LLI
//! flag and signal strength. A blank cell means "missing frequency / no
//! observable". An unparseable cell stays in place with a diagnostic and a
//! None value (it must never be dropped for plotting). Structural failures are
//! returned as file-level errors and reject the whole import.

use crate::ids::stable_id;
use crate::model::{InputBatch, RawClockEvent, RawObs, RawSatState};
use serde_json::Value;

#[derive(Debug, Clone)]
struct ObsType {
    code: String,
    kind: String,
    band: String,
}

fn classify(code: &str) -> Option<ObsType> {
    let mut chars = code.chars();
    let t = chars.next()?;
    let band = format!("L{}{}", chars.next().unwrap_or('?'), chars.next().unwrap_or('?'));
    let kind = match t {
        'C' => "pseudorange",
        'L' => "carrier_phase",
        'D' => "doppler",
        _ => return None,
    };
    Some(ObsType { code: code.to_string(), kind: kind.to_string(), band })
}

fn unit_for(kind: &str) -> &'static str {
    match kind {
        "pseudorange" => "metre",
        "carrier_phase" => "cycle",
        "doppler" => "hertz",
        _ => "unknown",
    }
}

pub fn parse_rinex(text: &str, version_id: &str) -> InputBatch {
    let mut batch = InputBatch::default();
    let mut lines = text.lines();
    let mut types_by_sys: std::collections::BTreeMap<String, Vec<ObsType>> =
        std::collections::BTreeMap::new();
    let mut seen_header_end = false;
    let mut epoch_index = 0usize;

    while let Some(line) = lines.next() {
        if line.len() < 60 {
            if !seen_header_end && line.trim().is_empty() {
                continue;
            }
        }
        let label = if line.len() >= 60 { line[59..].trim() } else { line.trim() };

        if !seen_header_end {
            match label {
                "END OF HEADER" => seen_header_end = true,
                "SYS / # / OBS TYPES" => {
                    let sys = if line.len() >= 1 {
                        line[..1].trim().to_string()
                    } else {
                        String::new()
                    };
                    let mut codes = Vec::new();
                    for slot in 0..13 {
                        let col = 7 + slot * 4;
                        if col + 3 <= line.len() {
                            let code = line[col..col + 3].trim();
                            if !code.is_empty() {
                                codes.push(code.to_string());
                            }
                        }
                    }
                    let entry = types_by_sys.entry(sys).or_default();
                    for c in codes {
                        if let Some(ot) = classify(&c) {
                            entry.push(ot);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        // Data section.
        if line.starts_with('>') {
            epoch_index += 1;
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 8 {
                batch.file_errors.push(format!(
                    "epoch record #{epoch_index} header has too few fields: {line:?}"
                ));
                continue;
            }
            let parsed_epoch: Result<Vec<i64>, _> =
                fields[1..=6].iter().map(|f| f.parse::<i64>()).collect();
            let (y, mo, d, hh, mm, ss_int) = match parsed_epoch {
                Ok(v) if v.len() == 6 => (v[0], v[1], v[2], v[3], v[4], v[5]),
                _ => {
                    batch.file_errors.push(format!(
                        "epoch record #{epoch_index} has an unreadable timestamp: {line:?}"
                    ));
                    continue;
                }
            };
            let ss: f64 = fields[6].parse().unwrap_or(ss_int as f64);
            let nsats: i64 = fields.get(8).and_then(|s| s.parse().ok()).unwrap_or(0);
            let epoch_s = epoch_to_seconds(y, mo, d, hh, mm, ss);
            let epoch_repr = format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:011.7}");

            // Receiver clock offset field: a non-zero value is a clock event.
            if let Some(clk) = fields.get(9) {
                if let Ok(offset_s) = clk.trim().parse::<f64>() {
                    if offset_s.abs() > 0.0 {
                        let id = stable_id(&[version_id, "clock", &epoch_s.to_string()]);
                        batch.clock_events.push(RawClockEvent {
                            id,
                            version_id: version_id.to_string(),
                            epoch_s,
                            scope: "ALL_SATELLITES".to_string(),
                            kind: "receiver_clock_jump".to_string(),
                            offset_s: Some(offset_s),
                            unit: "second".to_string(),
                            source: "rinex_epoch_header".to_string(),
                            note: "non-zero receiver clock offset on epoch header".to_string(),
                            raw_text: clk.to_string(),
                        });
                    }
                }
            }

            for _ in 0..nsats {
                let rec = match lines.next() {
                    Some(r) => r,
                    None => {
                        batch.file_errors.push(format!(
                            "epoch record #{epoch_index} declares {nsats} satellites but the file ended early"
                        ));
                        break;
                    }
                };
                let sat = rec[..3].trim().to_string();
                if sat.len() < 3 {
                    batch.file_errors
                        .push(format!("epoch record #{epoch_index} has a bad satellite id: {rec:?}"));
                    continue;
                }
                let sys = sat[..1].to_string();
                let types = match types_by_sys.get(&sys) {
                    Some(t) => t.clone(),
                    None => {
                        batch.file_errors.push(format!(
                            "satellite {sat} uses a system with no SYS / # / OBS TYPES header"
                        ));
                        continue;
                    }
                };
                for (i, ot) in types.iter().enumerate() {
                    let start = 3 + i * 16;
                    if start + 16 > rec.len() {
                        continue; // missing cell => frequency absent at this epoch
                    }
                    let cell = &rec[start..start + 16];
                    if cell.trim().is_empty() {
                        continue; // blank: missing observable, deliberately not stored
                    }
                    let value_text = cell[..14].trim().to_string();
                    let lli = cell[14..15].trim();
                    let snr = cell[15..16].trim();
                    let (value, diag) = match value_text.parse::<f64>() {
                        Ok(v) => (Some(v), None),
                        Err(_) => (
                            None,
                            Some(format!("unparseable {unit} value kept in place: {value_text:?}")),
                        ),
                    };
                    let id = stable_id(&[version_id, &sat, &ot.code, &epoch_s.to_string()]);
                    batch.observations.push(RawObs {
                        id,
                        version_id: version_id.to_string(),
                        epoch_s,
                        epoch_repr: epoch_repr.clone(),
                        sat: sat.clone(),
                        system: sys.clone(),
                        code: ot.code.clone(),
                        kind: ot.kind.clone(),
                        band: ot.band.clone(),
                        raw_text: value_text.clone(),
                        value,
                        lli: if lli.is_empty() { None } else { Some(lli.to_string()) },
                        snr: if snr.is_empty() { None } else { Some(snr.to_string()) },
                        unit: unit_for(&ot.kind).to_string(),
                        parse_diagnostic: diag,
                    });
                }
            }
        } else if !line.trim().is_empty() {
            batch.file_errors
                .push(format!("data section line outside an epoch record: {line:?}"));
        }
    }

    if !seen_header_end {
        batch
            .file_errors
            .push("missing END OF HEADER; not a recognizable RINEX observation file".to_string());
    }
    if batch.observations.is_empty() && batch.file_errors.is_empty() {
        batch.file_errors.push("no observation records found".to_string());
    }
    batch
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn epoch_to_seconds(year: i64, month: i64, day: i64, hh: i64, mm: i64, ss: f64) -> f64 {
    let mut days = 0i64;
    for y in 1980..year {
        days += if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
    }
    for m in 1..month {
        days += days_in_month(year, m);
    }
    days += day - 6; // 1980-01-06 was the GPS epoch Sunday
    (days as f64) * 86_400.0 + (hh as f64) * 3600.0 + (mm as f64) * 60.0 + ss
}

pub fn parse_clock_sidecar(text: &str, version_id: &str) -> Result<Vec<RawClockEvent>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("clock sidecar line {}: invalid JSON: {e}", n + 1))?;
        let epoch_s = v
            .get("t_epoch_s")
            .and_then(|x| x.as_f64())
            .ok_or_else(|| format!("clock sidecar line {}: missing numeric t_epoch_s", n + 1))?;
        let scope = v
            .get("scope")
            .and_then(|x| x.as_str())
            .unwrap_or("ALL_SATELLITES")
            .to_string();
        let id = stable_id(&[version_id, "clock-side", &epoch_s.to_string(), &scope]);
        out.push(RawClockEvent {
            id,
            version_id: version_id.to_string(),
            epoch_s,
            scope,
            kind: v.get("kind").and_then(|x| x.as_str()).unwrap_or("receiver_clock_jump").to_string(),
            offset_s: v.get("clock_offset_s_at_epoch").and_then(|x| x.as_f64()),
            unit: v.get("unit").and_then(|x| x.as_str()).unwrap_or("second").to_string(),
            source: v.get("source").and_then(|x| x.as_str()).unwrap_or("sidecar").to_string(),
            note: v.get("note").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            raw_text: line.to_string(),
        });
    }
    Ok(out)
}

pub fn parse_sat_states(text: &str, version_id: &str) -> Result<Vec<RawSatState>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("satellite state line {}: invalid JSON: {e}", n + 1))?;
        let epoch_s = v
            .get("epoch_s")
            .and_then(|x| x.as_f64())
            .ok_or_else(|| format!("satellite state line {}: missing numeric epoch_s", n + 1))?;
        let sat = v
            .get("sat")
            .and_then(|x| x.as_str())
            .ok_or_else(|| format!("satellite state line {}: missing sat", n + 1))?;
        let id = stable_id(&[version_id, "satstate", sat, &epoch_s.to_string()]);
        out.push(RawSatState {
            id,
            version_id: version_id.to_string(),
            epoch_s,
            sat: sat.to_string(),
            health: v.get("health").and_then(|x| x.as_str()).unwrap_or("UNKNOWN").to_string(),
            elevation_deg: v.get("elevation_deg").and_then(|x| x.as_f64()),
            azimuth_deg: v.get("azimuth_deg").and_then(|x| x.as_f64()),
            coordinate_ref: v
                .get("coordinate_ref")
                .and_then(|x| x.as_str())
                .unwrap_or("unspecified")
                .to_string(),
            raw_text: line.to_string(),
        });
    }
    Ok(out)
}
