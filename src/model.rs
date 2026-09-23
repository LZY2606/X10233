//! Domain model for the carrier-breakpoint desk (载波断点台).
//!
//! Every domain quantity carries a unit or a datum. Raw observation values are
//! stored exactly as submitted (`value_raw`); only the derived layer
//! normalizes. Stable identities are content-derived so that export -> wipe ->
//! re-import preserves identity, order and provenance.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Id = String;

/// Virtual time base: the desk never uses wall-clock time. Every event gets a
/// logical sequence number and a virtual timestamp derived from it, so tests
/// and replays are fully deterministic.
pub const VTIME_BASE: i64 = 1_000_000_000;

pub fn make_id(prefix: &str, parts: &[&str]) -> Id {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    for p in parts {
        h.update(b"|");
        h.update(p.as_bytes());
    }
    let hex = hex_encode(&h.finalize());
    format!("{}_{}", prefix, &hex[..16])
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn content_hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    hex_encode(&h.finalize())
}

/// Observation kind derived from the RINEX observation code's first char.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange, // C.. [m]
    Carrier,     // L.. [cycles]
    Doppler,     // D.. [Hz]
    Snr,         // S.. [dB-Hz]
}

impl ObsKind {
    pub fn from_code(code: &str) -> Option<ObsKind> {
        match code.chars().next()? {
            'C' => Some(ObsKind::Pseudorange),
            'L' => Some(ObsKind::Carrier),
            'D' => Some(ObsKind::Doppler),
            'S' => Some(ObsKind::Snr),
            _ => None,
        }
    }
    pub fn unit(&self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "m",
            ObsKind::Carrier => "cyc",
            ObsKind::Doppler => "Hz",
            ObsKind::Snr => "dB-Hz",
        }
    }
}

/// One observation value, kept exactly as submitted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsValue {
    pub code: String,  // e.g. "L1C"
    pub freq: u8,      // frequency number, e.g. 1
    pub kind: ObsKind,
    pub value_raw: String, // original representation, never rewritten
    pub lli: u8,       // loss-of-lock indicator as submitted
    pub unit: String,
}

/// A receiver clock-jump event line, kept as-is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockEventRaw {
    pub epoch_raw: String,
    pub epoch_gps_s: Option<f64>,
    pub millis: String, // raw text, parsed in derived layer
    pub raw_line: String,
}

/// Parsed import payload (records are stored per-row in the DB; this is the
/// in-memory shape used by the parser and the derived layer).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsedObs {
    pub station: String,
    pub sat: String,
    pub epoch_raw: String,
    pub epoch_gps_s: Option<f64>, // derived; datum GPS
    pub event_flag: u8,
    pub obs: Vec<ObsValue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub id: Id,
    pub input_id: Id,
    pub record_id: Option<Id>,
    pub severity: String, // "error" | "warning"
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputVersion {
    pub id: Id,
    pub parent_id: Option<Id>,
    pub seq: i64,
    pub vtime: i64,
    pub raw_text: String,
    pub content_hash: String,
    pub station: String,
    pub record_count: usize,
}

/// Detection / repair parameters. Fingerprinted; any revision invalidates
/// derived results that used the old fingerprint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DetectParams {
    pub gf_threshold_m: f64,
    pub mw_threshold_cycles: f64,
    pub td_threshold_m: f64,
    pub iono_tolerance_m: f64,
    pub max_gap_s: f64,
    pub clock_jump_min_ms: i64,
    pub sigma: f64,
    pub seed: u64,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gf_threshold_m: 0.15,
            mw_threshold_cycles: 0.6,
            td_threshold_m: 0.20,
            iono_tolerance_m: 0.06,
            max_gap_s: 60.0,
            clock_jump_min_ms: 1,
            sigma: 0.05,
            seed: 42,
        }
    }
}

impl DetectParams {
    pub fn fingerprint(&self) -> String {
        let canon = serde_json::to_string(self).unwrap_or_default();
        content_hash(&canon)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamSet {
    pub id: Id,
    pub parent_id: Option<Id>,
    pub seq: i64,
    pub name: String,
    pub params: DetectParams,
    pub fingerprint: String,
}

/// Scope of a derived result, used for arc-local staleness.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scope {
    pub sats: Vec<String>, // empty = all sats of the input
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DerivedResult {
    pub id: Id,
    pub kind: String, // "arcset" | "candidateset"
    pub branch: String,
    pub seq: i64,
    pub algo_version: String,
    pub param_fingerprint: String,
    pub sources: Vec<Id>, // direct sources: input version + param set
    pub scope: Scope,
    pub stale_since: Option<i64>,
    pub payload: serde_json::Value,
}

/// One integer-offset option for a candidate, e.g. L1:+7, L2:+3.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SlipOption {
    pub offsets: Vec<(u8, i64)>, // (freq, cycles)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub id: Id,
    pub result_id: Id,
    pub branch: String,
    pub seq: i64,
    pub station: String,
    pub sat: String,
    pub arc_id: String,
    pub epoch_gps_s: f64,
    pub detector: String, // "GF" | "MW" | "TD" | "LLI" | "CLOCK"
    pub score: f64,
    pub options: Vec<SlipOption>, // >1 => ambiguous, all legal options kept
    pub status: String,           // "proposed" | "undecidable" | "accepted" | "rejected"
    pub reasons: Vec<String>,
}

/// Repair layer: integer cycle offsets only. Raw observations are never
/// modified; repairs are applied as a separate layer from `from_epoch_gps_s`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repair {
    pub id: Id,
    pub branch: String,
    pub seq: i64,
    pub station: String,
    pub sat: String,
    pub freq: u8,
    pub from_epoch_gps_s: f64,
    pub cycles: i64,
    pub decision_id: Id,
    pub undone: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub id: Id,
    pub branch: String,
    pub seq: i64,
    pub candidate_id: Id,
    pub action: String, // "accept" | "reject" | "adjust" | "fork"
    pub option_index: Option<usize>,
    pub adjust: Option<SlipOption>,
    pub rationale: String,
    pub supersedes: Option<Id>,
    pub undone: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRow {
    pub seq: i64,
    pub id: Id,
    pub vtime: i64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// GPS constants (derived-layer normalization only).
pub const F1_HZ: f64 = 1_575_420_000.0;
pub const F2_HZ: f64 = 1_227_600_000.0;
pub const C_M_PER_S: f64 = 299_792_458.0;
pub const LAMBDA1_M: f64 = C_M_PER_S / F1_HZ;
pub const LAMBDA2_M: f64 = C_M_PER_S / F2_HZ;
pub const LAMBDA_WL_M: f64 = C_M_PER_S / (F1_HZ - F2_HZ);

pub const ALGO_VERSION: &str = "detect/1.0.0";
