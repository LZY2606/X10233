//! Explicit JSON sheet parser.
//!
//! Schema:
//! ```json
//! {
//!   "station": "STATION",
//!   "epochs": [ {
//!     "epoch": "2026-09-24T00:00:00.0000",
//!     "receiver_clock_seconds": 0.0,
//!     "clock_event": { "declared": true, "jump_seconds": 3.3356e-3 },
//!     "satellites": [ {
//!       "sv": "G01",
//!       "azimuth_deg": 42.0, "elevation_deg": 55.0,
//!       "ecef_m": [1.0e7, 2.0e7, 1.5e7],
//!       "obs": [ {"code":"C1W","value":21000000.123,"lli":0,"ssi":6} ]
//!     } ]
//!   } ]
//! }
//! ```
//! `value` may be a number, a numeric string kept verbatim, or `null` with a
//! `diagnostic`. Unknown observation codes / bands are retained with a
//! diagnostic rather than discarded.

use super::rinex::code_chars;
use super::{gps_frequency_hz, IngestOutcome};
use crate::event::{FormatError, RawBatch};
use crate::model::{
    ClockEvent, ClockEventType, Observation, Reference, SatState, Value, Id,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Sheet {
    station: String,
    epochs: Vec<EpochIn>,
}

#[derive(Deserialize)]
struct EpochIn {
    epoch: String,
    gps_seconds: Option<f64>,
    #[serde(default)]
    receiver_clock_seconds: Option<f64>,
    clock_event: Option<ClockIn>,
    #[serde(default)]
    satellites: Vec<SatIn>,
}

#[derive(Deserialize)]
struct ClockIn {
    declared: Option<bool>,
    jump_seconds: f64,
}

#[derive(Deserialize)]
struct SatIn {
    sv: String,
    azimuth_deg: Option<f64>,
    elevation_deg: Option<f64>,
    ecef_m: Option<[f64; 3]>,
    #[serde(default)]
    obs: Vec<ObsIn>,
}

#[derive(Deserialize)]
struct ObsIn {
    code: String,
    value: Option<serde_json::Value>,
    #[serde(default)]
    raw: Option<String>,
    #[serde(default)]
    lli: u8,
    #[serde(default)]
    ssi: u8,
    #[serde(default)]
    diagnostic: Option<String>,
}

fn reference_for(code: &str) -> Reference {
    match code.chars().next() {
        Some('C') => Reference::Metre,
        Some('L') => Reference::Cycle,
        Some('D') => Reference::MetrePerSecond,
        Some('S') => Reference::DbHz,
        _ => Reference::One,
    }
}

pub fn parse_sheet(
    project_id: &Id,
    parent_batch_id: Option<Id>,
    source_name: &str,
    text: &str,
) -> Result<IngestOutcome, String> {
    let sheet: Sheet =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON sheet: {e}"))?;
    if sheet.station.trim().is_empty() {
        return Err("sheet.station is required".to_string());
    }
    if sheet.epochs.is_empty() {
        return Err("sheet.epochs must not be empty".to_string());
    }

    let mut observations = vec![];
    let mut sat_states = vec![];
    let mut declared_clock_events = vec![];
    let mut format_errors = vec![];

    for (li, ep) in sheet.epochs.iter().enumerate() {
        let gps = ep.gps_seconds;
        if let Some(ce) = &ep.clock_event {
            if ce.declared.unwrap_or(true) {
                let id = crate::model::stable_id(
                    "clock",
                    &format!("{}|{}|{}", project_id, sheet.station, ep.epoch),
                );
                declared_clock_events.push(ClockEvent {
                    id,
                    station: sheet.station.clone(),
                    epoch_raw: ep.epoch.clone(),
                    gps_seconds: gps,
                    event_type: ClockEventType::Declared,
                    jump_seconds: Value::parsed(
                        format!("{:.10e}", ce.jump_seconds),
                        ce.jump_seconds,
                        Reference::Second,
                    ),
                    sources: vec![],
                });
            }
        }
        for sat in &ep.satellites {
            if sat.sv.len() < 2 {
                format_errors.push(FormatError {
                    line_no: Some(li + 1),
                    raw_line: format!("{:?}", sat.sv),
                    code: "bad-sv".to_string(),
                    message: "satellite id must be like G01".to_string(),
                });
                continue;
            }
            let ecef = sat.ecef_m.unwrap_or([f64::NAN, f64::NAN, f64::NAN]);
            let mut state_diag = vec![];
            if sat.azimuth_deg.is_none() {
                state_diag.push("azimuth missing".to_string());
            }
            if sat.elevation_deg.is_none() {
                state_diag.push("elevation missing".to_string());
            }
            if sat.ecef_m.is_none() {
                state_diag.push("ecef missing".to_string());
            }
            sat_states.push(SatState {
                id: crate::model::stable_id(
                    "sat",
                    &format!("{}|{}|{}|{}", project_id, sheet.station, ep.epoch, sat.sv),
                ),
                station: sheet.station.clone(),
                epoch_raw: ep.epoch.clone(),
                gps_seconds: gps,
                sv: sat.sv.clone(),
                azimuth_deg: sat.azimuth_deg,
                elevation_deg: sat.elevation_deg,
                x_m: if ecef[0].is_nan() { None } else { Some(ecef[0]) },
                y_m: if ecef[1].is_nan() { None } else { Some(ecef[1]) },
                z_m: if ecef[2].is_nan() { None } else { Some(ecef[2]) },
                diagnostics: state_diag,
            });

            for cell in &sat.obs {
                if cell.code.len() != 3 {
                    format_errors.push(FormatError {
                        line_no: Some(li + 1),
                        raw_line: cell.code.clone(),
                        code: "bad-code".to_string(),
                        message: "observation code must be 3 chars, e.g. C1W".to_string(),
                    });
                    continue;
                }
                let (kind, band) = code_chars(&cell.code);
                let reference = reference_for(&cell.code);
                let raw = match &cell.raw {
                    Some(r) => r.clone(),
                    None => match &cell.value {
                        Some(serde_json::Value::Number(n)) => n.to_string(),
                        Some(serde_json::Value::String(s)) => s.clone(),
                        _ => String::new(),
                    },
                };
                let mut diagnostics = vec![];
                let mut numeric = match &cell.value {
                    Some(serde_json::Value::Number(n)) => n.as_f64(),
                    Some(serde_json::Value::String(s)) => s.parse::<f64>().ok(),
                    Some(serde_json::Value::Null) | None => None,
                    Some(_) => None,
                };
                if gps_frequency_hz(band).is_none() {
                    diagnostics.push(format!("unknown RINEX band '{band}'; frequency not resolved"));
                }
                if let Some(d) = &cell.diagnostic {
                    diagnostics.push(d.clone());
                }
                if numeric.is_none() {
                    diagnostics.push("unconvertible value retained at origin".to_string());
                    numeric = None;
                }
                observations.push(Observation {
                    id: crate::model::stable_id(
                        "obs",
                        &format!(
                            "{}|{}|{}|{}|{}",
                            project_id, sheet.station, ep.epoch, sat.sv, cell.code
                        ),
                    ),
                    station: sheet.station.clone(),
                    epoch_raw: ep.epoch.clone(),
                    gps_seconds: gps,
                    sv: sat.sv.clone(),
                    code: cell.code.clone(),
                    kind,
                    band,
                    frequency_hz: gps_frequency_hz(band),
                    lli: cell.lli,
                    ssi: cell.ssi,
                    value: Value { raw, value: numeric, reference, diagnostics },
                });
            }
        }
    }

    let batch = RawBatch {
        batch_id: crate::model::stable_id(
            "batch",
            &format!("{}|{}|{}|{}", project_id, source_name, sheet.station, observations.len()),
        ),
        parent_batch_id,
        source_name: source_name.to_string(),
        input_fragment: text.to_string(),
        observations,
        sat_states,
        declared_clock_events,
        format_errors,
    };
    Ok(IngestOutcome { batch, warnings: vec![] })
}
