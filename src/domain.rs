//! Core domain types.
//!
//! Everything reviewable is first-class data: raw observations, clock events,
//! satellite states, detector parameter versions, immutable recomputed runs,
//! arcs, candidates (accepted *and* rejected), repairs (offset layer only),
//! findings, and human decisions.

use crate::units::Si;
use serde::{Deserialize, Serialize};

/// Algorithm version of the derived layer. Bumped when detector semantics change.
pub const ALGO_VERSION: &str = "carrier-break-desk.algo.v1";

/// Trigger that caused a run to be recomputed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    InitialImport,
    ParamRevision,
    SourceAmend,
    Decision,
}

// ---------------------------------------------------------------------------
// Source layer (immutable once committed; amendments are new versions)
// ---------------------------------------------------------------------------

/// One RINEX-like observation row at one epoch, stored verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Stable identity: sha1 of the logical row key + event id.
    pub id: String,
    pub event_id: String,
    /// For amendment rows: id of the previous version of this logical row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub station: String,
    pub satellite: String,
    /// Original epoch token (e.g. RFC3339), retained as written.
    pub epoch_raw: String,
    /// Parsed epoch seconds since the project time origin (virtual clock).
    pub epoch_t: f64,
    /// RINEX-like observation code, e.g. "C1C" / "L1C" / "D1C" / "S1C".
    pub code: String,
    /// Frequency number (band) as resolved at import.
    pub freq_number: Option<u8>,
    /// Value with raw text + unit retained; normalised SI value on derived use.
    pub value: Si,
    /// Loss-of-lock indicator raw flag ("0".."7"), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lli: Option<String>,
    /// Monotonic version of the logical row (station,sat,epoch,code).
    pub version: u32,
}

/// A receiver (whole-unit) clock event recorded in the source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub event_id: String,
    pub station: String,
    pub epoch_raw: String,
    pub epoch_t: f64,
    /// Clock jump/slew magnitude, seconds; raw form retained.
    pub shift: Si,
    pub kind: String, // "jump" | "slew"
    pub note: String,
}

/// Satellite state row (position + clock) used by the time-difference detector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatState {
    pub id: String,
    pub event_id: String,
    pub satellite: String,
    pub epoch_raw: String,
    pub epoch_t: f64,
    /// ECEF metres; frame recorded in [`SatState::frame`].
    pub x: Si,
    pub y: Si,
    pub z: Si,
    /// Satellite clock bias, seconds.
    pub clock_bias_s: Si,
    pub frame: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceiverPosition {
    pub station: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub frame: String,
}

/// Detector parameters; every run stores the fingerprint of the exact params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorParams {
    /// Geometry-free / ionosphere residual gate (metres).
    pub gf_tolerance_m: f64,
    /// Melbourne-Wubbena wide-lane gate (cycles).
    pub mw_tolerance_cycles: f64,
    /// Time-difference carrier gate (cycles).
    pub td_tolerance_cycles: f64,
    /// Fraction of tracked satellites sharing an apparent jump that flags a
    /// *receiver* clock event rather than per-satellite slips.
    pub clock_common_fraction: f64,
    /// Maximum gap (seconds) that time-difference propagation is trusted over.
    pub td_max_gap_s: f64,
    /// Minimum history epochs for an MW running mean.
    pub mw_min_history: usize,
    /// LLI flag bits treated as loss-of-lock (RINEX: bit 0 and bit 1).
    pub lli_loss_bits: u8,
    /// Search half-width (cycles) for integer candidate enumeration.
    pub search_half_width: i64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            gf_tolerance_m: 0.03,
            mw_tolerance_cycles: 0.25,
            td_tolerance_cycles: 2.0,
            clock_common_fraction: 0.8,
            td_max_gap_s: 120.0,
            mw_min_history: 2,
            lli_loss_bits: 0b011,
            search_half_width: 60,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let j = serde_json::to_string(self).expect("params serialize");
        format!("{}:{}", ALGO_VERSION, crate::store::digest_short(j.as_bytes()))
    }
}

// ---------------------------------------------------------------------------
// Event log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    ProjectCreated { project: String, seed: u64 },
    ObservationsBatch { rows: Vec<Observation> },
    ClockEventsBatch { rows: Vec<ClockEvent> },
    SatStatesBatch { rows: Vec<SatState>, receiver: Option<ReceiverPosition> },
    ParamsRevised { params: DetectorParams, note: String },
    DecisionLogged(Decision),
    /// A commit closes a group of events and produces one derived run.
    Commit {
        trigger: Trigger,
        title: String,
        note: String,
    },
    BranchForked { from_event: String, from_branch: String, title: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    /// sha1 content id, stable across export/import.
    pub id: String,
    pub seq: i64,
    pub branch: String,
    pub parent_id: Option<String>,
    pub payload: EventPayload,
    /// Virtual-clock ticks (deterministic), never wall clock.
    pub created_ticks: i64,
}

// ---------------------------------------------------------------------------
// Derived layer: runs, arcs, candidates, repairs, findings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Active,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    /// Geometry-free ionosphere combination.
    Gf,
    /// Melbourne-Wubbena wide-lane combination.
    Mw,
    /// Time-differenced carrier (with satellite-state propagation).
    Td,
    /// Loss-of-lock indicator flag only (no numeric confirmation).
    Lli,
    /// Receiver whole-unit clock event, kept separate from per-sat slips.
    Clock,
}

impl Detector {
    pub fn code(&self) -> &'static str {
        match self {
            Detector::Gf => "gf",
            Detector::Mw => "mw",
            Detector::Td => "td",
            Detector::Lli => "lli",
            Detector::Clock => "clock",
        }
    }
}

/// One integer repair hypothesis from a detector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffsetHypothesis {
    /// Offset in whole cycles per observation code, e.g. {"L1C":18,"L2W":14}.
    pub cycles_by_code: std::collections::BTreeMap<String, i64>,
    pub score: f64,
    pub note: String,
}

/// A cycle-slip candidate at a station/satellite/epoch boundary.
///
/// Multiple detectors may produce *different* candidates for the same
/// boundary; each detector is stored independently — none is treated as truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable content identity (logical boundary + detector + params fingerprint).
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub satellite: String,
    pub epoch_t: f64,
    pub epoch_raw: String,
    pub detector: Detector,
    pub status: CandidateStatus,
    /// 0..1 detector confidence/score.
    pub score: f64,
    /// Combination residual (metres for GF, cycles otherwise).
    pub residual: f64,
    pub residual_unit: String,
    /// All legal integer hypotheses; may contain several equal alternatives.
    pub hypotheses: Vec<OffsetHypothesis>,
    /// Structured rejection reason when rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    /// Direct source observation ids this candidate is computed from.
    pub direct_sources: Vec<String>,
    /// Decision that resolved this candidate, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// Human note carried through to the run.
    pub note: String,
}

/// A continuous carrier arc for station/satellite/code.
#[derive(Debug,Clone, PartialEq, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub satellite: String,
    pub code: String,
    pub index: usize,
    pub start_t: f64,
    pub end_t: f64,
    pub epochs: Vec<f64>,
    pub direct_sources: Vec<String>,
    /// Accepted cumulative repair offset (cycles) carried on this arc — offset
    /// layer only, raw observations are never rewritten.
    pub applied_offset_cycles: i64,
}

/// A receiver-clock finding: detected separately from per-satellite slips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockFinding {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub epoch_t: f64,
    pub epoch_raw: String,
    /// Common apparent jump (metres of range-equivalent phase).
    pub common_jump_m: f64,
    pub agreement_fraction: f64,
    pub matched_source_event: Option<String>,
    pub satellites: Vec<String>,
}

/// Diagnostics/unknowns are first-class: missing frequencies, unconvertible
/// values, untracked gaps, ambiguous-enumeration notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub run_id: String,
    pub severity: String, // "info" | "warning" | "error"
    pub kind: String,
    pub station: String,
    pub satellite: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch_t: Option<f64>,
    pub message: String,
    pub source_ids: Vec<String>,
}

/// An accepted repair lives only on the offset layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub satellite: String,
    pub epoch_t: f64,
    pub code: String,
    pub cycles: i64,
    pub decision_id: String,
}

/// Human adjudication. Logged as an event; replaying it branches recomputation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    /// Run whose candidates were reviewed.
    pub run_id: String,
    pub station: String,
    pub satellite: String,
    pub epoch_t: f64,
    pub action: String, // "accept" | "reject"
    /// Targeted candidate detector code (gf/mw/td/lli).
    pub target_detector: String,
    /// Chosen hypothesis offsets when accepting; empty for a reject.
    pub cycles_by_code: std::collections::BTreeMap<String, i64>,
    pub reason: String,
    /// Index of chosen hypothesis on the targeted candidate (for provenance).
    pub hypothesis_index: Option<usize>,
    /// Set when a later decision reversed this one (conflict resolution/undo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversed_by: Option<String>,
    pub note: String,
}

/// One derived run: immutable snapshot of every computed artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub seq: i64,
    pub branch: String,
    pub parent_run_id: Option<String>,
    pub trigger: Trigger,
    pub title: String,
    pub note: String,
    pub commit_event_id: String,
    pub algo_version: String,
    pub param_fingerprint: String,
    pub params: DetectorParams,
    pub arcs: Vec<Arc>,
    pub candidates: Vec<Candidate>,
    pub clock_findings: Vec<ClockFinding>,
    pub findings: Vec<Finding>,
    pub repairs: Vec<Repair>,
    /// Input observation versions included (observation id set).
    pub source_ids: Vec<String>,
}

/// A station/satellite/epoch observation across codes, as the detectors see it.
#[derive(Debug, Clone, Default)]
pub struct EpochCodes {
    pub t: f64,
    pub epoch_raw: String,
    /// code -> (value m or cyc as applicable, raw observation id, lli)
    pub vals: std::collections::BTreeMap<String, (f64, String, Option<String>)>,
    /// Codes present but whose value could not be converted.
    pub unconverted: Vec<String>,
}
