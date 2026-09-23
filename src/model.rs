//! Core domain types and stable-identity helpers.
//!
//! Every derived value carries an algorithm version, a parameter fingerprint
//! and its direct sources. Identities are content-derived so that deleting the
//! database and re-importing the same archive reproduces the exact same ids.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.len().to_string().as_bytes());
        h.update(b":");
        h.update(p.as_bytes());
        h.update(b"|");
    }
    hex::encode(h.finalize())
}

pub fn short_id(prefix: &str, parts: &[&str]) -> String {
    format!("{}_{}", prefix, &digest(parts)[..16])
}

/// Deterministic JSON string used inside fingerprints / id hashes.
pub fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap(),
        serde_json::Value::Array(a) => {
            let body: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", body.join(","))
        }
        serde_json::Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(&o[*k])))
                .collect();
            format!("{{{}}}", body.join(","))
        }
    }
}

/// A physical quantity is never passed around without unit or datum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub code: String,
    pub message: String,
    /// Exact original token that could not be converted, retained in place.
    pub raw: String,
}

/// One observation slot for one satellite/frequency at one epoch, stored
/// exactly as the RINEX record presented it (normalization happens in a
/// separate derived row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObs {
    pub sat: String,
    pub code: String,
    pub band: String,
    pub freq_hz: i64,
    pub raw_field: String,
    pub raw_value: String,
    pub lli: Option<u8>,
    pub ssi_raw: Option<String>,
    pub kind: ObsKind,
    pub diagnostic: Option<ParseDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    Ssi,
    Unconvertible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRecord {
    pub seq: i64,
    pub epoch: String,
    pub recv_clock_offset_s: String,
    pub obs: Vec<RawObs>,
}

/// RINEX observation-file header fields kept as original text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RinexHeader {
    pub version: String,
    pub marker_name: String,
    pub approx_position_xyz_m: [String; 3],
    pub sys_obs: Vec<(String, Vec<String>)>,
    pub time_system: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RinexFile {
    pub header: RinexHeader,
    pub epochs: Vec<EpochRecord>,
    /// Non-fatal problems (unconvertible values kept in place, etc.).
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportDiagnostic {
    pub severity: String, // "error" | "warning"
    pub code: String,
    pub line: Option<usize>,
    pub message: String,
}

/// Receiver clock events are first-class imported material, separate from
/// any per-satellite cycle-slip hypothesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub epoch: String,
    pub event: String, // CLOCK_JUMP ...
    pub jump_s: Option<f64>,
    pub source: String,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteState {
    pub epoch: String,
    pub sat: String,
    pub position_xyz_m: [f64; 3],
    pub datum: String,
    pub health: String,
    pub source: String,
}

/// Tunable detector parameters. A revision creates a new rule version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorParams {
    pub gf_rate_limit_m_per_s: f64,
    pub mw_limit_cycles: f64,
    pub td_residual_limit_m: f64,
    pub max_gap_s: f64,
    pub max_cycles: i64,
    pub min_mw_history: usize,
    pub clock_jump_min_s: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            gf_rate_limit_m_per_s: 0.02,
            mw_limit_cycles: 1.0,
            td_residual_limit_m: 0.05,
            max_gap_s: 35.0,
            max_cycles: 200,
            min_mw_history: 2,
            clock_jump_min_s: 1.0e-6,
        }
    }
}

pub const ALGORITHM_VERSION: &str = "gnss-cycle-slip-0.1.0";

/// GPS nominal frequencies (RINEX channel numbers 1 and 2).
pub fn band_frequency_hz(code_or_band: &str) -> Option<(&'static str, i64)> {
    match code_or_band {
        "1" | "L1" | "C1C" | "L1C" | "D1C" | "S1C" => Some(("1", 1_575_420_000)),
        "2" | "L2" | "C2W" | "L2W" | "D2W" | "S2W" => Some(("2", 1_227_600_000)),
        _ => None,
    }
}

pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;
pub const WAVELENGTH_L1_M: f64 = SPEED_OF_LIGHT_M_S / 1_575_420_000.0;
pub const WAVELENGTH_L2_M: f64 = SPEED_OF_LIGHT_M_S / 1_227_600_000.0;
