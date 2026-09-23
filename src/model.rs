//! Core domain model for the carrier-breakpoint desk.
//!
//! All domain quantities carry a unit or a datum. Raw imported values are
//! preserved verbatim; only the derived layer normalizes.

use serde::{Deserialize, Serialize};

/// Algorithm identifiers recorded on every derived value.
pub const ALGO_ARCS: &str = "arcseg";
pub const ALGO_DETECT: &str = "slipdetect";
pub const ALGO_CLOCK: &str = "clockjump";
pub const ALGO_VERSION: &str = "1.0.0";

/// GPS L1/L2 carrier frequencies (Hz) and speed of light (m/s).
pub const F1_HZ: f64 = 1_575.42e6;
pub const F2_HZ: f64 = 1_227.60e6;
pub const C_MPS: f64 = 299_792_458.0;
pub const LAM1_M: f64 = C_MPS / F1_HZ;
pub const LAM2_M: f64 = C_MPS / F2_HZ;
/// Wide-lane wavelength (m).
pub const LAM_W_M: f64 = C_MPS / (F1_HZ - F2_HZ);

/// Detection parameters. A fingerprint of this struct is stamped on every
/// derived detection result; revising a parameter creates a new fingerprint
/// and supersedes (never overwrites) older results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectParams {
    /// Arc break when the epoch gap exceeds this many seconds.
    pub gap_threshold_s: f64,
    /// Geometry-free (iono) residual flag threshold, meters.
    pub gf_thr_m: f64,
    /// Melbourne-Wubbena residual flag threshold, meters.
    pub mw_thr_m: f64,
    /// Time-difference (carrier minus Doppler) flag threshold, cycles.
    pub td_thr_cyc: f64,
    /// Common-mode pseudorange jump treated as a receiver clock jump, meters.
    pub clock_common_m: f64,
    /// Largest integer slip considered by the offset enumerator, cycles.
    pub max_slip: i32,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gap_threshold_s: 60.0,
            gf_thr_m: 0.05,
            mw_thr_m: 0.40,
            td_thr_cyc: 0.5,
            clock_common_m: 1000.0,
            max_slip: 5,
        }
    }
}

/// Candidate review state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Proposed,
    Accepted,
    Rejected,
    /// Several integer offset pairs fit the evidence; all are kept.
    Undecidable,
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateStatus::Proposed => "proposed",
            CandidateStatus::Accepted => "accepted",
            CandidateStatus::Rejected => "rejected",
            CandidateStatus::Undecidable => "undecidable",
        }
    }
}

/// Adjudication actions a reviewer may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accept,
    Reject,
    MarkUndecidable,
}

impl Decision {
    pub fn parse(s: &str) -> Option<Decision> {
        match s {
            "accept" => Some(Decision::Accept),
            "reject" => Some(Decision::Reject),
            "undecidable" => Some(Decision::MarkUndecidable),
            _ => None,
        }
    }
}

/// State machine: which (from, action) pairs are legal.
pub fn transition_allowed(from: CandidateStatus, action: Decision) -> bool {
    use CandidateStatus::*;
    use Decision::*;
    match (from, action) {
        (Proposed, Accept) | (Proposed, Reject) | (Proposed, MarkUndecidable) => true,
        (Undecidable, Accept) | (Undecidable, Reject) => true,
        // Re-opening a closed candidate requires an undo of the closing
        // adjudication event; direct accepted<->rejected flips are illegal.
        _ => false,
    }
}

/// One detector's vote on one epoch. `available=false` votes are first-class:
/// they record *why* a detector could not speak (e.g. missing frequency).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorVote {
    pub detector: String,
    pub available: bool,
    pub flagged: bool,
    /// Measured jump estimate (unit depends on detector, see `unit`).
    pub estimate: Option<f64>,
    pub unit: String,
    pub reason: Option<String>,
}

/// A legal integer offset pair (dN1, dN2) in cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffsetPair {
    pub dn1: i32,
    pub dn2: i32,
}

/// A cycle-slip candidate on one station-satellite arc at one epoch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub arc_id: String,
    pub station: String,
    pub sat: String,
    pub epoch: String,
    pub t: f64,
    pub score: f64,
    pub votes: Vec<DetectorVote>,
    pub lli_hint: bool,
    /// All integer offset pairs consistent with the evidence.
    pub legal_offsets: Vec<OffsetPair>,
    /// Machine-readable note, e.g. "no-consistent-integer-pair".
    pub note: Option<String>,
}

/// Arc segment: a continuous run of epochs for one station-satellite pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub start_epoch: String,
    pub end_epoch: String,
    pub start_t: f64,
    pub end_t: f64,
    pub n_epochs: usize,
}

/// Receiver clock jump: kept strictly separate from per-satellite slips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockJump {
    pub station: String,
    pub epoch: String,
    pub t: f64,
    /// Jump size in meters of pseudorange (common to all satellites).
    pub size_m: f64,
    /// Same jump expressed in seconds of receiver clock.
    pub size_s: f64,
    pub source: String, // "detected" | "provided"
}

/// Full detection payload stored as a derived value for one arc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectResult {
    pub arc: Arc,
    pub candidates: Vec<Candidate>,
    /// Epochs suppressed because a receiver clock jump explains them.
    pub suppressed_clock_epochs: Vec<String>,
    /// Per-epoch combination residuals for the review UI.
    pub residuals: Vec<ResidualRow>,
}

/// One epoch of combination residuals (derived layer, normalized units).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualRow {
    pub epoch: String,
    pub t: f64,
    pub gf_m: Option<f64>,
    pub gf_resid_m: Option<f64>,
    pub mw_m: Option<f64>,
    pub mw_resid_m: Option<f64>,
    pub td1_cyc: Option<f64>,
    pub td2_cyc: Option<f64>,
    pub arc_boundary: bool,
    pub clock_jump: bool,
    pub repaired: bool,
}

/// Deterministic FNV-1a fingerprint (hex). Stable across runs and machines.
pub fn fnv_hex(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

impl DetectParams {
    pub fn fingerprint(&self) -> String {
        let j = serde_json::to_string(self).unwrap_or_default();
        fnv_hex(&[j.as_str()])
    }
}

/// Unit implied by a RINEX observation code (C/L/D/S).
pub fn unit_of(code: &str) -> &'static str {
    match code.chars().next() {
        Some('C') => "m",
        Some('L') => "cyc",
        Some('D') => "Hz",
        Some('S') => "dB-Hz",
        _ => "?",
    }
}

/// Frequency band number implied by a RINEX observation code.
pub fn freq_of(code: &str) -> Option<i32> {
    code.chars().nth(1).and_then(|c| c.to_digit(10)).map(|d| d as i32)
}
