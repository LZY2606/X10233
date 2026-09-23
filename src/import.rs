//! Import layer.
//!
//! Accepts a small, copyable JSON sample format. Structural problems are
//! reported as explicit format errors; individual unconvertible quantities are
//! *kept in place* with diagnostics. Pseudorange, carrier, Doppler, LLI, SNR,
//! clock events, frequency number and observation code are all saved exactly as
//! they appeared epoch by epoch.

use crate::model::{
    parse_band, Diagnostic, EpochRecord, ImportPayload, Observation, Quantity, SatObs, SatState,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservationWire {
    pub code: String,
    #[serde(default)]
    pub pseudorange: String,
    #[serde(default)]
    pub carrier: String,
    #[serde(default)]
    pub doppler: String,
    #[serde(default)]
    pub lli: String,
    #[serde(default)]
    pub ssi: String,
    #[serde(default)]
    pub snr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawSatWire {
    pub sat: String,
    #[serde(default)]
    pub obs: Vec<RawObservationWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawSatStateWire {
    pub sat: String,
    #[serde(default)]
    pub x: String,
    #[serde(default)]
    pub y: String,
    #[serde(default)]
    pub z: String,
    #[serde(default)]
    pub datum: String,
    #[serde(default)]
    pub clock_bias: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEventWire {
    pub jump: serde_json::Value,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpochWire {
    pub time: String,
    #[serde(default = "default_flag")]
    pub flag: String,
    #[serde(default)]
    pub sats: Vec<RawSatWire>,
    #[serde(default)]
    pub sat_states: Vec<RawSatStateWire>,
    #[serde(default)]
    pub clock_events: Vec<RawClockEventWire>,
}
fn default_flag() -> String {
    "0".to_string()
}

fn number(raw: &str, unit: &str, kind: &str, diag: &mut Vec<String>) -> Quantity {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Quantity::raw_only(raw, unit, format!("missing {kind} value"));
    }
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() => Quantity::ok(raw, v, unit),
        _ => {
            diag.push(format!("unconvertible {kind} token {raw:?} kept in place ({unit})"));
            Quantity::raw_only(raw, unit, format!("cannot parse {kind} as f64"))
        }
    }
}

/// RINEX SSI 1..9 maps to dB-Hz (ssi-1)*6+20; a direct numeric token is taken
/// as dB-Hz already.
fn snr(raw: &str, ssi: &str, diag: &mut Vec<String>) -> Quantity {
    let trimmed = raw.trim();
    if !trimmed.is_empty() {
        if let Ok(v) = trimmed.parse::<f64>() {
            if v.is_finite() {
                return Quantity::ok(raw, v, "dB-Hz");
            }
        }
        diag.push(format!("unconvertible SNR token {raw:?} kept in place (dB-Hz)"));
        return Quantity::raw_only(raw, "dB-Hz", "cannot parse SNR as f64");
    }
    if let Ok(n) = ssi.trim().parse::<i64>() {
        if (1..=9).contains(&n) {
            return Quantity::ok(ssi, (n - 1) as f64 * 6.0 + 20.0, "dB-Hz");
        }
    }
    Quantity::raw_only(raw, "dB-Hz", "no SNR value and SSI outside RINEX 1..=9".to_string())
}

pub fn parse_import(text: &str) -> Result<(ImportPayload, Vec<Diagnostic>), String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "import root must be a JSON object".to_string())?;
    let station = require_str(obj, "station")?;
    let mut diagnostics = Vec::new();

    let coord = |key: &str| -> Result<f64, String> {
        match obj.get(key) {
            Some(serde_json::Value::Number(n)) => n
                .as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| format!("station coordinate {key} is not finite")),
            Some(_) => Err(format!("station coordinate {key} must be a number")),
            None => Err(format!("missing station coordinate {key}")),
        }
    };
    let sx = coord("station_x_m")?;
    let sy = coord("station_y_m")?;
    let sz = coord("station_z_m")?;

    let epochs_val = obj
        .get("epochs")
        .ok_or_else(|| "missing required `epochs` array".to_string())?;
    let epochs_wire: Vec<RawEpochWire> = serde_json::from_value(epochs_val.clone())
        .map_err(|e| format!("epochs structure invalid: {e}"))?;
    if epochs_wire.is_empty() {
        return Err("epochs must contain at least one record".to_string());
    }

    let mut epochs = Vec::new();
    for ew in &epochs_wire {
        let mut epoch_diag: Vec<String> = Vec::new();
        // time must be structurally valid (a hard ordering key).
        crate::model::parse_time(&ew.time).map_err(|e| format!("epoch {}: {e}", ew.time))?;
        let mut sats = Vec::new();
        for sw in &ew.sats {
            if sw.sat.trim().is_empty() {
                return Err(format!("epoch {} has a satellite with empty id", ew.time));
            }
            let mut sat_diag: Vec<String> = Vec::new();
            let mut obs = Vec::new();
            for ow in &sw.obs {
                if ow.code.trim().is_empty() {
                    return Err(format!(
                        "epoch {} sat {} has an observation with empty code",
                        ew.time, sw.sat
                    ));
                }
                let mut local: Vec<String> = Vec::new();
                let band = match parse_band(&ow.code) {
                    Ok(b) => Some(b.code().to_string()),
                    Err(e) => {
                        local.push(e);
                        None
                    }
                };
                let fnum = ow
                    .code
                    .chars()
                    .nth(1)
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                let pseudorange_m = number(&ow.pseudorange, "m", "pseudorange", &mut local);
                let carrier_cycles = number(&ow.carrier, "cycle", "carrier phase", &mut local);
                let doppler_hz = number(&ow.doppler, "Hz", "Doppler", &mut local);
                let snr_db_hz = snr(&ow.snr, &ow.ssi, &mut local);
                for d in &local {
                    sat_diag.push(format!("{}: {d}", ow.code));
                }
                obs.push(Observation {
                    code: ow.code.clone(),
                    frequency_number: fnum,
                    band,
                    pseudorange_m,
                    carrier_cycles,
                    doppler_hz,
                    lli: ow.lli.clone(),
                    ssi: ow.ssi.clone(),
                    snr_db_hz,
                });
            }
            // duplicate observation codes are explicit format errors
            let mut codes: Vec<&str> = obs.iter().map(|o| o.code.as_str()).collect();
            codes.sort_unstable();
            if let Some(w) = codes.windows(2).find(|w| w[0] == w[1]) {
                return Err(format!(
                    "epoch {} sat {} lists duplicate observation code {}",
                    ew.time, sw.sat, w[0]
                ));
            }
            for d in &sat_diag {
                epoch_diag.push(format!("{}: {d}", sw.sat));
            }
            diagnostics.extend(sat_diag.iter().map(|m| Diagnostic {
                code: "UNCONVERTIBLE_VALUE".into(),
                message: m.clone(),
                time: Some(ew.time.clone()),
                sat: Some(sw.sat.clone()),
                obs_code: None,
            }));
            sats.push(SatObs {
                sat: sw.sat.clone(),
                obs,
            });
        }
        let mut sat_ids: Vec<&str> = sats.iter().map(|s| s.sat.as_str()).collect();
        sat_ids.sort_unstable();
        if let Some(w) = sat_ids.windows(2).find(|w| w[0] == w[1]) {
            return Err(format!(
                "epoch {} lists satellite {} more than once",
                ew.time, w[0]
            ));
        }
        let mut sat_states = Vec::new();
        for stw in &ew.sat_states {
            let mut local: Vec<String> = Vec::new();
            let x = number(&stw.x, "m", "sat X", &mut local);
            let y = number(&stw.y, "m", "sat Y", &mut local);
            let z = number(&stw.z, "m", "sat Z", &mut local);
            let clock = number(&stw.clock_bias, "m", "sat clock bias", &mut local);
            for d in local {
                diagnostics.push(Diagnostic {
                    code: "UNCONVERTIBLE_VALUE".into(),
                    message: format!("{}: {d}", stw.sat),
                    time: Some(ew.time.clone()),
                    sat: Some(stw.sat.clone()),
                    obs_code: None,
                });
            }
            sat_states.push(SatState {
                sat: stw.sat.clone(),
                x_m: x,
                y_m: y,
                z_m: z,
                datum: if stw.datum.trim().is_empty() {
                    "UNKNOWN_DATUM".to_string()
                } else {
                    stw.datum.clone()
                },
                clock_bias_m: clock,
            });
        }
        let mut clock_events = Vec::new();
        for cw in &ew.clock_events {
            let jump = match &cw.jump {
                serde_json::Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
                _ => None,
            };
            let jump = match jump {
                Some(v) => v,
                None => {
                    epoch_diag.push(format!("clock event jump at {} is not numeric", ew.time));
                    continue;
                }
            };
            clock_events.push(crate::model::ClockJumpEvent {
                time: ew.time.clone(),
                jump_s: jump,
                source: if cw.source.is_empty() {
                    "import".to_string()
                } else {
                    cw.source.clone()
                },
                note: cw.note.clone(),
            });
        }
        for d in epoch_diag {
            diagnostics.push(Diagnostic {
                code: "EPOCH_WARNING".into(),
                message: d,
                time: Some(ew.time.clone()),
                sat: None,
                obs_code: None,
            });
        }
        epochs.push(EpochRecord {
            time: ew.time.clone(),
            flag: if ew.flag.is_empty() {
                "0".to_string()
            } else {
                ew.flag.clone()
            },
            sats,
            sat_states,
            clock_events,
        });
    }
    // monotonic epoch ordering
    let mut times: Vec<&str> = epochs.iter().map(|e| e.time.as_str()).collect();
    if times.windows(2).any(|w| w[0] >= w[1]) {
        return Err("epoch times must be strictly increasing".to_string());
    }
    times.sort();

    Ok((
        ImportPayload {
            station,
            receiver: obj
                .get("receiver")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            antenna: obj
                .get("antenna")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            station_datum: obj
                .get("station_datum")
                .and_then(|v| v.as_str())
                .unwrap_or("ITRF2014")
                .to_string(),
            station_x_m: sx,
            station_y_m: sy,
            station_z_m: sz,
            epochs,
        },
        diagnostics,
    ))
}

fn require_str<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Result<&'a str, String> {
    match obj.get(key).and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(format!("missing required non-empty string `{key}`")),
    }
}
