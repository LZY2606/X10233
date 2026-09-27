//! Shared domain types for the carrier-breakpoint desk.
//! All domain quantities carry a unit or coordinate basis; raw inputs are
//! preserved verbatim and only the derived layer normalizes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SPEED_OF_LIGHT: f64 = 299_792_458.0; // m/s, SI
pub const ALGO_VERSION: &str = "detector/1.2.0";
pub const ARCHIVE_FORMAT: &str = "carrier-breakpoint-desk/archive/1";

pub const DET_GF: &str = "geometry-free-iono";
pub const DET_MW: &str = "melbourne-wubbena";
pub const DET_TD: &str = "time-difference";
pub const DET_CLOCK: &str = "receiver-clock-jump";
pub const DET_LLI: &str = "lli-flag";

/// Carrier frequency in Hz for a RINEX-style frequency number (GPS bands).
pub fn freq_hz(number: i64) -> Option<f64> {
    match number {
        1 => Some(1575.42e6),
        2 => Some(1227.60e6),
        5 => Some(1176.45e6),
        _ => None,
    }
}

/// Wavelength in metres for a frequency number.
pub fn wavelength_m(number: i64) -> Option<f64> {
    freq_hz(number).map(|f| SPEED_OF_LIGHT / f)
}

/// Unit attached to every parsed observation; derived from the observation code.
pub fn unit_for_code(code: &str) -> Option<&'static str> {
    match code.chars().next() {
        Some('C') => Some("m"),
        Some('L') => Some("cycles"),
        Some('D') => Some("Hz"),
        Some('S') => Some("dB-Hz"),
        _ => None,
    }
}

/// Versioned detection rule set. A parameter revision creates a new
/// `RuleVersion` with a parent pointer; the fingerprint identifies the
/// parameter set on every derived value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectParams {
    pub gf_threshold_cycles: f64,
    pub mw_threshold_cycles: f64,
    pub td_threshold_cycles: f64,
    pub clock_threshold_m: f64,
    pub combine_tolerance_cycles: f64,
    pub snr_min_dbhz: f64,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gf_threshold_cycles: 0.30,
            mw_threshold_cycles: 0.80,
            td_threshold_cycles: 0.50,
            clock_threshold_m: 1000.0,
            combine_tolerance_cycles: 0.35,
            snr_min_dbhz: 0.0,
        }
    }
}

impl DetectParams {
    pub fn fingerprint(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        short_hash(json.as_bytes())
    }
}

pub fn short_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())[..16].to_string()
}

pub fn full_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A parsed observation row. `raw_value` is always kept verbatim; `value` is
/// the normalized derived view and stays `None` (with `diag`) when the raw
/// representation cannot be converted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub version_id: String,
    pub epoch_id: String,
    pub t: f64, // seconds, UTC (GPS-handling note: epochs treated on a continuous UTC-like scale)
    pub sat: String,
    pub freq_number: i64,
    pub obs_code: String,
    pub raw_value: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub lli: Option<i64>,
    pub snr: Option<f64>,
    pub diag: Option<String>,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epoch {
    pub id: String,
    pub version_id: String,
    pub t: f64,
    pub t_iso: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub version_id: String,
    pub t: f64,
    pub kind: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub version_id: String,
    pub sat: String,
    pub freq_number: i64,
    pub t_start: f64,
    pub t_end: f64,
    pub src_fp: String,
    pub stale_seq: Option<i64>,
}

/// A cycle-slip (or clock-jump) hypothesis. Candidates are first-class data:
/// score, evidence, rejection reason and unknown states are all stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub branch_id: String,
    pub version_id: String,
    pub sat: String, // "*" for receiver-level clock jumps
    pub t: f64,
    pub detector: String,
    /// slip vector, e.g. {"1": 7, "2": 0} cycles per frequency number
    pub vector_json: String,
    pub score: f64,
    pub evidence_json: String,
    pub algo_version: String,
    pub params_fp: String,
    pub sources_json: String, // observation ids this candidate directly depends on
    pub status_hint: String,  // proposed | unconfirmed | ambiguous | clock-event
    pub reject_reason: Option<String>,
    pub stale_seq: Option<i64>,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub candidate_id: String,
    pub branch_id: String,
    pub action: String, // accept | reject | adjust-params | undo | resolve
    pub rationale: String,
    pub reviewer: String,
    pub seq: i64,
}

/// Repairs are an offset layer only; raw observations are never modified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repair {
    pub id: String,
    pub decision_id: String,
    pub branch_id: String,
    pub version_id: String,
    pub sat: String,
    pub freq_number: i64,
    pub t: f64,
    pub offset_cycles: f64,
    pub algo_version: String,
    pub params_fp: String,
    pub sources_json: String,
    pub stale_seq: Option<i64>,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub kind: String,
    pub payload_json: String,
    pub at: String,
}

/// One parse problem, tied to a line so the importer can report explicit
/// format errors instead of failing silently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseIssue {
    pub line: usize,
    pub message: String,
    pub raw: String,
}
