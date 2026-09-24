//! Domain model.
//!
//! Layering rule enforced by the type names:
//! * `Raw*` structs preserve epoch tokens verbatim (`raw_value`, `raw_epoch`,
//!   `raw_unit`) and may carry an unconvertible value (`value == None` plus a
//!   diagnostic). They are the *source layer* and are immutable once imported.
//! * Engine outputs (`EdgeStatistic`, `CycleCandidate`, `OffsetEntry` ...) are
//!   the *derived layer*. Every one carries algorithm version, parameter
//!   fingerprint and direct-source identities.
//!
//! Every domain quantity carries its unit or coordinate datum as a string;
//! values are never plotted at the cost of dropping raw representations.

use serde::{Deserialize, Serialize};

pub const U_M: &str = "m";
pub const U_CYCLE: &str = "cycle";
pub const U_HZ: &str = "Hz";
pub const U_DBHZ: &str = "dBHz";
pub const U_S: &str = "s";
pub const U_NS: &str = "ns";
pub const U_DEG: &str = "deg";
pub const U_HZ_BAND: &str = "Hz";
/// Time datum for parsed epochs.
pub const DATUM_TIME: &str = "gps_seconds_since_2000-01-01";
/// Elevation/azimuth datum used by the bundled satellite-state records.
pub const DATUM_TOPO: &str = "topocentric_azimuth_north_clockwise";
/// Pseudorange coordinate basis.
pub const DATUM_RANGE: &str = "receiver_antenna_to_satellite_transmission_time";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange,
    Phase,
    Doppler,
    Snr,
    Lli,
}

impl ObsKind {
    pub fn from_code(code: &str) -> ObsKind {
        let base = code.trim_end_matches(|c: char| c.is_ascii_digit());
        match base {
            "P" | "C" => ObsKind::Pseudorange,
            "L" => ObsKind::Phase,
            "D" => ObsKind::Doppler,
            "S" => ObsKind::Snr,
            "LLI" => ObsKind::Lli,
            _ => ObsKind::Phase,
        }
    }
    pub fn expected_unit(self) -> &'static str {
        match self {
            ObsKind::Pseudorange => U_M,
            ObsKind::Phase => U_CYCLE,
            ObsKind::Doppler => U_HZ,
            ObsKind::Snr => U_DBHZ,
            ObsKind::Lli => "bitmask",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line_no: i64,
    pub code: String,
    pub message: String,
    pub severity: String, // "error" | "warning"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandDef {
    pub band: String,
    pub phase_code: String,
    pub pseudo_code: String,
    pub doppler_code: String,
    pub snr_code: String,
    /// Raw declared frequency token (e.g. "1575.42e6"); preserved verbatim.
    pub frequency_raw: String,
    pub frequency_hz: Option<f64>,
    pub wavelength_raw: String,
    pub wavelength_m: Option<f64>,
    pub unit: String,
    pub datum: String,
}

impl BandDef {
    pub fn wavelength(&self) -> Option<f64> {
        self.wavelength_m.filter(|w| *w > 0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    /// Natural key inside its batch: `batch@epoch/sat/code`.
    pub natural_id: String,
    pub batch_id: String,
    pub line_no: i64,
    pub epoch_raw: String,
    pub epoch_secs: Option<f64>,
    pub epoch_datum: String,
    pub sat: String,
    pub code: String,
    pub kind: ObsKind,
    pub band: String,
    pub raw_value: String,
    pub raw_unit: String,
    pub value: Option<f64>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEvent {
    pub natural_id: String,
    pub batch_id: String,
    pub line_no: i64,
    pub epoch_raw: String,
    pub epoch_secs: Option<f64>,
    pub epoch_datum: String,
    pub raw_delta: String,
    pub raw_unit: String,
    pub delta_ns: Option<f64>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSatState {
    pub natural_id: String,
    pub batch_id: String,
    pub line_no: i64,
    pub epoch_raw: String,
    pub epoch_secs: Option<f64>,
    pub epoch_datum: String,
    pub sat: String,
    pub az_raw: String,
    pub el_raw: String,
    pub health_raw: String,
    pub datum: String,
    pub az_deg: Option<f64>,
    pub el_deg: Option<f64>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StationHeader {
    pub marker: String,
    pub receiver: String,
    pub antenna: String,
    pub approx_xyz_raw: String,
    pub approx_xyz_m: Option<[f64; 3]>,
    pub coord_datum: String,
    pub coord_unit: String,
}

/// Result of parsing one submitted source text. Nothing here has been
/// normalized away: unconvertible tokens stay in place with diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedImport {
    pub format: String,
    pub raw_sha: String,
    pub header: StationHeader,
    pub bands: Vec<BandDef>,
    pub observations: Vec<RawObservation>,
    pub clock_events: Vec<RawClockEvent>,
    pub sat_states: Vec<RawSatState>,
    pub diagnostics: Vec<Diagnostic>,
}

// ---- Derived layer ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Proposed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeFlag {
    Normal,
    IrregularGap,
    ClockJumpDeclared,
    MissingFrequency,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeStatistic {
    pub id: String,
    pub detector: String, // "TD" | "GF" | "MW" | "CLOCK" | "LLI"
    pub alg_version: String,
    pub param_fp: String,
    pub sat: String,
    /// "1", "2", "1-2" for combinations.
    pub band: String,
    pub at_seq: i64,
    pub at_epoch: String,
    pub dt_secs: f64,
    pub value: Option<f64>,
    pub value_unit: String,
    pub flag: EdgeFlag,
    pub sources: Vec<String>,
    pub diagnostic: Option<String>,
}

/// One legal integer-offset hypothesis at an arc boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleCandidate {
    pub id: String,
    /// Hypotheses with the same `ambiguity_group` are alternatives (e.g.
    /// several equally dividing integer offsets). None is privileged as truth.
    pub ambiguity_group: String,
    pub ambiguity_n: usize,
    pub detector: String, // "TD" | "GF" | "MW" | "LLI"
    pub alg_version: String,
    pub param_fp: String,
    pub sat: String,
    pub band: String,
    pub at_seq: i64,
    pub at_epoch: String,
    /// Primary-band correction (cycles added to raw phase at/after edge).
    /// TD band hypotheses set only this; combined GF/MW hypotheses also set
    /// `offset_b_cycles`.
    pub offset_cycles: f64,
    pub offset_b_cycles: Option<f64>,
    pub band_b: Option<String>,
    /// `true` when no unique integer offset can be assigned (LLI only).
    pub offset_unknown: bool,
    pub score: f64,
    pub evidence: Vec<String>,
    /// Detector proposals that contributed, including disagreements — no
    /// single detector is treated as ground truth.
    pub detector_proposals: Vec<String>,
    /// Reviewer hint that the hypothesis should be rejected (the human still
    /// adjudicates).
    pub suggest_reject: bool,
    pub status: CandidateStatus,
    pub status_reason: Option<String>,
    /// Offset-layer entry identities created when this hypothesis is taken.
    pub offset_ids: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArcState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub start_seq: i64,
    pub end_seq: Option<i64>,
    pub start_epoch: String,
    pub end_epoch: Option<String>,
    pub state: ArcState,
    pub close_reason: Option<String>,
    /// Candidate ids located at this arc's opening edge (for the first arc at
    /// the track start the list is empty).
    pub boundary_candidates: Vec<String>,
}

/// Repair layer only: a decision never rewrites a phase observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetEntry {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub at_seq: i64,
    pub at_epoch: String,
    pub cycles: f64,
    pub unit: String,
    pub decision_event_id: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedCandidate {
    pub candidate_id: String,
    pub reason_code: String,
    pub reason: String,
    pub decision_event_id: String,
}

/// Full deterministic computation result at one viewpoint (event id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewState {
    pub viewpoint_event_id: String,
    pub params_version: String,
    pub alg_fingerprint: String,
    pub epochs: Vec<EpochInfo>,
    pub arcs: Vec<Arc>,
    pub statistics: Vec<EdgeStatistic>,
    pub candidates: Vec<CycleCandidate>,
    pub offsets: Vec<OffsetEntry>,
    pub rejected: Vec<RejectedCandidate>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochInfo {
    pub seq: i64,
    pub epoch_raw: String,
    pub t_secs: f64,
    pub dt_secs: Option<f64>,
    pub irregular_gap: bool,
    pub declared_clock_delta_ns: Option<f64>,
}
