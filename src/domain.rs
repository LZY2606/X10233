//! Core domain model for the carrier-break workbench.
//!
//! The model is deliberately split into two layers:
//!
//! * **raw layer** — imported exactly as authored (`ObsLine`, `ClockLine`,
//!   `SatStateLine`). Values carry both their original textual representation
//!   and a normalised value+unit/datum. Unconvertible tokens stay in place as
//!   diagnostics instead of being dropped.
//! * **derived layer** — arcs, combination residuals, detector candidates,
//!   hypotheses and offset-only repairs. Every derived value carries an
//!   algorithm version, a parameter fingerprint and direct source references.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Quantities with units / datums
// ---------------------------------------------------------------------------

/// Physical unit or coordinate/time datum attached to a numeric quantity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Meter,
    Cycle,
    MetersPerSecond,
    Hertz,
    Second,
    Nanosecond,
    EpochSeconds,
    Igs08,
    Itrf2020,
    Unitless,
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cycle",
            Unit::MetersPerSecond => "m/s",
            Unit::Hertz => "Hz",
            Unit::Second => "s",
            Unit::Nanosecond => "ns",
            Unit::EpochSeconds => "epoch_seconds",
            Unit::Igs08 => "IGS08",
            Unit::Itrf2020 => "ITRF2020",
            Unit::Unitless => "1",
        }
    }
}

/// A measured value that keeps its original representation. `normalised` is
/// `None` when the token could not be converted; the raw text is never lost.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Qty {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: Unit,
}

impl Qty {
    pub fn parsed(raw: impl Into<String>, value: f64, unit: Unit) -> Self {
        Self { raw: raw.into(), value: Some(value), unit }
    }

    pub fn unconvertible(raw: impl Into<String>, unit: Unit) -> Self {
        Self { raw: raw.into(), value: None, unit }
    }
}

// ---------------------------------------------------------------------------
// Raw layer
// ---------------------------------------------------------------------------

/// Station definition block (`STATION`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StationDef {
    pub marker: String,
    pub name: Option<String>,
    pub x: Qty,
    pub y: Qty,
    pub z: Qty,
    pub datum: String,
}

/// Frequency definition (`FREQUENCY`): frequency number and observation code
/// metadata kept verbatim per station/system/band.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrequencyDef {
    pub station: String,
    pub system: String,
    pub band: String,
    pub code: String,
    pub freq_hz: Qty,
}

/// One satellite-state line, kept per epoch. Position carries a coordinate
/// datum; clock bias carries a unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatStateLine {
    pub line_no: i64,
    pub epoch: i64,
    pub sat: String,
    pub x: Qty,
    pub y: Qty,
    pub z: Qty,
    pub datum: String,
    pub clock_bias: Qty,
}

/// A receiver clock event (`CLOCK`). Whole-receiver jumps are deliberately a
/// distinct stream from per-satellite phase breaks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockLine {
    pub line_no: i64,
    pub epoch: i64,
    pub station: String,
    pub jump_ns: Qty,
    pub cause: Option<String>,
}

/// One RINEX-style observation line, kept exactly as authored.
///
/// `ll` is the loss-of-lock indicator (0/1 plus raw text), `snr` is the
/// signal-to-noise ratio (carried with its raw ratio string, e.g. `42` dBHz),
/// `freq_no` is the frequency number (RINEX `frq`), and `code` the observation
/// code (e.g. `1C`). Any token that fails conversion is retained with a
/// diagnostic in `SourceBundle.diagnostics`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObsLine {
    pub line_no: i64,
    pub epoch: i64,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub code: String,
    pub freq_no: Option<i64>,
    pub pseudorange: Option<Qty>,
    pub carrier: Option<Qty>,
    pub doppler: Option<Qty>,
    pub lli: Option<LLI>,
    pub snr: Option<Qty>,
}

/// Loss-of-lock indicator with original bit string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LLI {
    pub raw: String,
    pub flag: i64,
}

/// Result of parsing a source blob. Raw input is immutable once submitted;
/// revisions are expressed as a new version with a parent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceBundle {
    pub kind: String,
    pub format_version: String,
    pub stations: Vec<StationDef>,
    pub frequencies: Vec<FrequencyDef>,
    pub obs: Vec<ObsLine>,
    pub sat_states: Vec<SatStateLine>,
    pub clocks: Vec<ClockLine>,
    /// Parse diagnostics carrying the 1-based source line number. Invalid
    /// lines stay visible here rather than silently dropping data.
    pub diagnostics: Vec<ParseDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParseDiagnostic {
    pub line_no: i64,
    pub code: String,
    pub message: String,
    pub raw_line: String,
    pub severity: String,
}

// ---------------------------------------------------------------------------
// Derived layer — parameters
// ---------------------------------------------------------------------------

/// Tunable parameters of the v1 detector pipeline. Parameter revisions are
/// content-addressed (`fingerprint`) and never mutate in place.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// Algorithm identification.
    pub algorithm: String,
    pub algorithm_version: String,
    /// Epoch gap larger than this (seconds) opens a gap boundary.
    pub gap_threshold_s: f64,
    /// |GF jump| (cycles) above which a break candidate is emitted.
    pub gf_threshold_cycles: f64,
    /// |MW wide-lane ambiguity jump| (wide-lane cycles) threshold.
    pub mw_threshold_cycles: f64,
    /// |single-band time-difference jump| (cycles) threshold.
    pub td_threshold_cycles: f64,
    /// Candidates scoring within this distance (cycles) of the best integer
    /// solution are retained as "equally divided" legal candidates.
    pub tie_tolerance_cycles: f64,
    /// Minimum satellites that must co-jump for a receiver clock event.
    pub clock_min_satellites: i64,
    /// An LLI flag with no independent detector support above this fraction
    /// is reported as a possible false alarm rather than a confirmed break.
    pub lli_false_alarm_confirm_fraction: f64,
    /// Robust window (epochs) used for jump normalisation.
    pub robust_window: i64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            algorithm: "carrierbreak-pipeline".into(),
            algorithm_version: "1.0.0",
            gap_threshold_s: 30.0,
            // Loose default: multiple "equally divided" integer offsets survive.
            gf_threshold_cycles: 3.0,
            mw_threshold_cycles: 3.0,
            td_threshold_cycles: 3.0,
            tie_tolerance_cycles: 2.0,
            clock_min_satellites: 3,
            lli_false_alarm_confirm_fraction: 0.5,
            robust_window: 3,
        }
    }
}

impl DetectorParams {
    /// Tight revision used in the case pack: ties collapse to a single
    /// legal candidate.
    pub fn tight_revision() -> Self {
        Self {
            gf_threshold_cycles: 0.75,
            mw_threshold_cycles: 0.75,
            td_threshold_cycles: 0.75,
            tie_tolerance_cycles: 0.25,
            ..Self::default()
        }
    }

    /// Canonical JSON used both for fingerprinting and replay.
    pub fn fingerprint(&self) -> String {
        let json = serde_json::to_string(self).expect("params serialize");
        crate::ids::stable_id("par", &[json])
    }
}

// ---------------------------------------------------------------------------
// Derived layer — residuals, candidates, hypotheses, arcs
// ---------------------------------------------------------------------------

/// Which detector produced a candidate / residual series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    /// Geometry-free (ionosphere) combination: L1 - L2 in L1 cycles.
    Gf,
    /// Melbourne-Wubbena wide-lane ambiguity.
    Mw,
    /// Single-band carrier time difference (cycles/s).
    Td,
}

impl Detector {
    pub fn as_str(&self) -> &'static str {
        match self {
            Detector::Gf => "gf",
            Detector::Mw => "mw",
            Detector::Td => "td",
        }
    }
}

/// One point of a detector residual series. Identity is stable across reruns
/// as long as its direct sources (the two obs lines) and algorithm version do
/// not change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResidualPoint {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub detector: String,
    pub band: Option<String>,
    pub epoch: i64,
    pub dt_s: f64,
    pub value: f64,
    pub value_unit: String,
    /// Jump relative to the previous point, in the detector's native unit.
    pub jump: Option<f64>,
    /// 1-based direct source obs-line ids (stable).
    pub source_line_nos: Vec<i64>,
    pub source_version_id: String,
    pub algorithm_version: String,
}

/// An integer offset proposed by one detector at one boundary. Candidates are
/// first-class data: rejected or ambiguous candidates remain reviewable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub boundary_epoch: i64,
    pub detector: String,
    pub band: Option<String>,
    /// Proposed integer offset (cycles) attributable to this detector.
    pub offset_cycles: i64,
    /// Absolute mismatch between the measured jump and the integer.
    pub residual_cycles: f64,
    /// Raw measured jump before integer rounding.
    pub measured_jump: f64,
    pub score: f64,
    /// Why this candidate is/isn't usable.
    pub status: CandidateStatus,
    pub rejection: Option<Rejection>,
    pub source_point_ids: Vec<String>,
    pub algorithm_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    /// A legal candidate retained for review/adoption.
    Live,
    /// Equally divided: tied with at least one other legal candidate.
    Tied,
    /// Explained by a receiver clock event; not a per-satellite slip.
    ClockExplained,
    /// LLI flag fired but no independent detector corroborates.
    LliFalseAlarm,
    /// Rejected by a human or by a later revision.
    Rejected,
    /// Inputs insufficient to decide uniquely; all legal offsets kept.
    Ambiguous,
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateStatus::Live => "live",
            CandidateStatus::Tied => "tied",
            CandidateStatus::ClockExplained => "clock_explained",
            CandidateStatus::LliFalseAlarm => "lli_false_alarm",
            CandidateStatus::Rejected => "rejected",
            CandidateStatus::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rejection {
    pub reason_code: String,
    pub detail: String,
}

/// A per-band integer repair hypothesis that fuses detector evidence. The
/// repair is stored **only as an offset layer** — raw carrier values are never
/// rewritten.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hypothesis {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub boundary_epoch: i64,
    /// band -> integer cycles applied from this boundary onward.
    pub offsets: Vec<BandOffset>,
    pub total_score: f64,
    /// True when more than one hypothesis is legal within tie tolerance.
    pub ambiguous: bool,
    pub contributing_candidate_ids: Vec<String>,
    pub status: HypothesisStatus,
    pub algorithm_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BandOffset {
    pub band: String,
    pub offset_cycles: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
}

impl HypothesisStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            HypothesisStatus::Proposed => "proposed",
            HypothesisStatus::Accepted => "accepted",
            HypothesisStatus::Rejected => "rejected",
            HypothesisStatus::Superseded => "superseded",
        }
    }
}

/// Reason an arc boundary exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryKind {
    Start,
    EpochGap,
    FrequencyMissing,
    LliFlag,
    DetectorBreak,
    AcceptedSlip,
    End,
}

impl BoundaryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BoundaryKind::Start => "start",
            BoundaryKind::EpochGap => "epoch_gap",
            BoundaryKind::FrequencyMissing => "frequency_missing",
            BoundaryKind::LliFlag => "lli_flag",
            BoundaryKind::DetectorBreak => "detector_break",
            BoundaryKind::AcceptedSlip => "accepted_slip",
            BoundaryKind::End => "end",
        }
    }
}

/// An arc boundary between two epochs for one station/satellite/band.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArcBoundary {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub epoch_before: Option<i64>,
    pub epoch_after: i64,
    pub kind: String,
    pub detail: String,
    pub candidate_ids: Vec<String>,
}

/// A continuous arc. Staleness is computed per arc: only arcs whose direct
/// source inputs changed become expired.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Arc {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub epoch_start: i64,
    pub epoch_end: i64,
    pub boundary_start_id: String,
    pub boundary_end_id: Option<String>,
    pub source_line_nos: Vec<i64>,
    pub source_version_id: String,
    pub algorithm_version: String,
}

/// A clock-jump interpretation, kept separate from per-satellite slips.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockInterpretation {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub epoch: i64,
    pub common_mode_cycles_l1: f64,
    pub affected_sats: Vec<String>,
    pub source_clock_line_no: Option<i64>,
    pub algorithm_version: String,
}

/// Complete output of one pipeline execution. Immutable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RunResult {
    pub run_id: String,
    pub project_id: String,
    pub branch: String,
    pub seq: i64,
    pub source_version_id: String,
    pub param_version_id: String,
    pub params: DetectorParams,
    pub virtual_clock_tag: String,
    pub residuals: Vec<ResidualPoint>,
    pub candidates: Vec<Candidate>,
    pub hypotheses: Vec<Hypothesis>,
    pub boundaries: Vec<ArcBoundary>,
    pub arcs: Vec<Arc>,
    pub clocks: Vec<ClockInterpretation>,
    pub created_virtual_time: String,
}

// ---------------------------------------------------------------------------
// Human decisions and offset layer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    AcceptHypothesis,
    RejectCandidate,
    MarkLliFalseAlarm,
    MarkClockEvent,
    ParameterOverride,
}

impl DecisionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionKind::AcceptHypothesis => "accept_hypothesis",
            DecisionKind::RejectCandidate => "reject_candidate",
            DecisionKind::MarkLliFalseAlarm => "mark_lli_false_alarm",
            DecisionKind::MarkClockEvent => "mark_clock_event",
            DecisionKind::ParameterOverride => "parameter_override",
        }
    }
}

/// A human adjudication. Decisions are append-only events; conflicts and
/// undo are represented by later events rather than mutation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub seq: i64,
    pub kind: String,
    pub target_id: String,
    pub run_id: String,
    pub note: String,
    /// Present for accept; the offset-only layer applied.
    pub offsets: Vec<BandOffset>,
    pub created_virtual_time: String,
}
