use crate::error::{AppError, AppResult};
use crate::frequency::SPEED_OF_LIGHT_M_S;
use crate::hashing::content_id;
use crate::types::{InputBundle, Ruleset, ALGORITHM_VERSION};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Support status of one detector for an integer hypothesis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Support {
    Supports,
    Rejects,
    Unknown,
}

impl Support {
    fn tag(self) -> &'static str {
        match self {
            Support::Supports => "supports",
            Support::Rejects => "rejects",
            Support::Unknown => "unknown",
        }
    }
    fn parse(s: &str) -> Support {
        match s {
            "supports" => Support::Supports,
            "rejects" => Support::Rejects,
            _ => Support::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
struct FreqData {
    frequency_no: i64,
    obs_code: String,
    wavelength_m: f64,
    pseudorange_m: Option<f64>,
    carrier_cycles: Option<f64>,
    doppler_hz: Option<f64>,
    lli: Option<i64>,
    snr: Option<f64>,
    pseudorange_diag: Vec<String>,
    carrier_diag: Vec<String>,
    doppler_diag: Vec<String>,
    snr_diag: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct SatEpoch {
    freqs: BTreeMap<i64, FreqData>,
}

#[derive(Debug, Clone)]
struct EpochInfo {
    seq: i64,
    sow: f64,
    clock_offset_s: Option<f64>,
    sats: HashMap<String, SatEpoch>,
}

#[derive(Debug, Clone, Default)]
struct PairAccum {
    gf: Vec<f64>,
    mw_phase: Vec<f64>,
    mw_code: Vec<f64>,
    td1: Vec<f64>,
    td2: Vec<f64>,
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

fn mad_scale(values: &[f64], med: f64) -> f64 {
    let dev: Vec<f64> = values.iter().map(|x| (x - med).abs()).collect();
    median(&dev) * 1.4826
}

fn nearest_integer(x: f64) -> i64 {
    x.round() as i64
}

fn num(measure: &crate::types::Measure, expected_unit: &str) -> (Option<f64>, Vec<String>) {
    let mut diag = measure.diagnostics.clone();
    if let Some(v) = measure.value {
        if let Some(u) = &measure.unit {
            if u != expected_unit {
                diag.push(format!(
                    "unit mismatch: raw value '{}' carries unit '{}', expected '{}'",
                    measure.raw, u, expected_unit
                ));
                return (None, diag);
            }
        } else {
            diag.push(format!("raw value '{}' has no unit; left in place", measure.raw));
            return (None, diag);
        }
        (Some(v), diag)
    } else {
        diag.push(format!(
            "non-numeric raw value '{}' left in place, no derived normalization",
            measure.raw
        ));
        (None, diag)
    }
}

fn build_epochs(
    bundle: &InputBundle,
) -> (Vec<EpochInfo>, Vec<Value>) {
    let mut wl: HashMap<(String, i64), f64> = HashMap::new();
    for f in &bundle.frequencies {
        wl.insert((f.system.clone(), f.frequency_no), f.wavelength_m);
    }
    let mut epochs = Vec::new();
    let mut diagnostics = Vec::new();
    for epoch in &bundle.epochs {
        let mut info = EpochInfo {
            seq: epoch.seq,
            sow: epoch.time.gpst_sow,
            clock_offset_s: num(&epoch.receiver_clock_offset, "s").0,
            sats: HashMap::new(),
        };
        for obs in &epoch.observations {
            let system: String = obs.sat.chars().take(1).collect();
            let (pr, prd) = num(&obs.pseudorange, "m");
            let (ph, phd) = num(&obs.carrier, "cycle");
            let (dop, dopd) = num(&obs.doppler, "Hz");
            let (snr, snrd) = num(&obs.snr, "dB-Hz");
            if !wl.contains_key(&(system.clone(), obs.frequency_no)) {
                diagnostics.push(json!({
                    "severity": "error",
                    "at": {"epoch_seq": epoch.seq, "sat": obs.sat, "frequency_no": obs.frequency_no},
                    "message": format!("no declared wavelength for system {} frequency {}", system, obs.frequency_no)
                }));
            }
            let se = info.sats.entry(obs.sat.clone()).or_default();
            se.freqs.insert(
                obs.frequency_no,
                FreqData {
                    frequency_no: obs.frequency_no,
                    obs_code: obs.obs_code.clone(),
                    wavelength_m: wl
                        .get(&(system, obs.frequency_no))
                        .copied()
                        .unwrap_or(f64::NAN),
                    pseudorange_m: pr,
                    carrier_cycles: ph,
                    doppler_hz: dop,
                    lli: obs.lli,
                    snr,
                    pseudorange_diag: prd,
                    carrier_diag: phd,
                    doppler_diag: dopd,
                    snr_diag: snrd,
                },
            );
        }
        epochs.push(info);
    }
    epochs.sort_by_key(|e| e.seq);
    (epochs, diagnostics)
}
