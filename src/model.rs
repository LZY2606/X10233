// Domain model: GNSS carrier-phase cycle-slip review.
// All domain quantities carry a unit or coordinate datum explicitly.

use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "algo-carrier-break-2026.09.1";
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyDef {
    pub idx: u32,
    pub band: String,
    pub value_mhz: f64,
    pub system: String,
    pub codes: Vec<String>,
    pub wavelength_m: f64,
}

impl FrequencyDef {
    pub fn new(idx: u32, band: String, value_mhz: f64, system: String, codes: Vec<String>) -> Self {
        let wavelength_m = SPEED_OF_LIGHT_M_S / (value_mhz * 1.0e6);
        Self { idx, band, value_mhz, system, codes, wavelength_m }
    }
}

/// Unit attached to an observation code. Phase stays in cycles (never silently
/// converted to metres); derived layers convert and record their own units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObsUnit {
    Meter,
    Cycle,
    Hertz,
    DbHz,
    Dimensionless,
}

impl ObsUnit {
    pub fn label(self) -> &'static str {
        match self {
            ObsUnit::Meter => "m",
            ObsUnit::Cycle => "cycle",
            ObsUnit::Hertz => "Hz",
            ObsUnit::DbHz => "dB-Hz",
            ObsUnit::Dimensionless => "1",
        }
    }
    pub fn of_code(code: &str) -> Option<ObsUnit> {
        match code.chars().next() {
            Some('C') => Some(ObsUnit::Meter),
            Some('L') => Some(ObsUnit::Cycle),
            Some('D') => Some(ObsUnit::Hertz),
            Some('S') => Some(ObsUnit::DbHz),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationDef {
    pub id: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub height_m: f64,
    pub datum: String,
}

/// One observable token preserved exactly as it appeared in the epoch record.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawToken {
    pub code: String,
    pub freq_idx: Option<u32>,
    pub original: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub lli: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpochLine {
    pub sat: String,
    pub tokens: Vec<RawToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    pub seq: u32,
    pub iso: String,
    pub epoch_s: f64,
    pub system: String,
    pub lines: Vec<RawEpochLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEventLine {
    pub iso: String,
    pub epoch_s: f64,
    pub scope: String,
    pub delta_text: String,
    pub delta_m: Option<f64>,
    pub unit: String,
    pub kind: String,
    pub note: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateLine {
    pub sat: String,
    pub iso: String,
    pub epoch_s: f64,
    pub health: String,
    pub elevation_deg: Option<f64>,
    pub azimuth_deg: Option<f64>,
    pub datum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedCase {
    pub format: String,
    pub station: StationDef,
    pub frequencies: Vec<FrequencyDef>,
    pub notes: Vec<String>,
    pub epochs: Vec<RawEpoch>,
    pub clock_events: Vec<ClockEventLine>,
    pub sat_states: Vec<SatStateLine>,
    /// Hard format errors make the whole import invalid (nothing is stored).
    pub errors: Vec<String>,
}

/// Tunable detection parameters. A revision creates a new immutable version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectParams {
    pub iono_trigger_m: f64,
    pub iono_fit_tol_m: f64,
    pub mw_trigger_cycle: f64,
    pub mw_confirm_cycle: f64,
    pub td_trigger_m: f64,
    pub gap_split_s: f64,
    pub mw_max_gap_epochs: u32,
    pub clock_trigger_m: f64,
    pub clock_agree_fraction: f64,
    pub tie_epsilon_m: f64,
}

impl Default for DetectParams {
    fn default() -> Self {
        Self {
            iono_trigger_m: 0.12,
            iono_fit_tol_m: 0.10,
            mw_trigger_cycle: 0.60,
            mw_confirm_cycle: 0.35,
            td_trigger_m: 0.30,
            gap_split_s: 60.0,
            mw_max_gap_epochs: 2,
            clock_trigger_m: 100.0,
            clock_agree_fraction: 0.60,
            tie_epsilon_m: 0.02,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Proposed,
    Ambiguous,
    Unresolved,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DetectorVerdict {
    Supports,
    Disagrees,
    Unavailable,
    NotTriggered,
}

/// Per-detector raw suggestion. None of the detectors is treated as truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorVote {
    pub detector: String,
    pub observed: Option<f64>,
    pub unit: String,
    pub suggested_n1: Option<i32>,
    pub suggested_n2: Option<i32>,
    pub verdict: DetectorVerdict,
    pub reason: String,
}

/// A legal integer-offset hypothesis. Rejected hypotheses stay in the record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub index: u32,
    pub n1: i32,
    pub n2: i32,
    pub iono_residual_m: f64,
    pub mw_residual_cycle: Option<f64>,
    pub td_residual_m: Option<f64>,
    pub score: f64,
    pub best: bool,
    pub tied: bool,
    pub rejected_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorCandidate {
    pub boundary_seq: u32,
    pub from_epoch_s: f64,
    pub to_epoch_s: f64,
    pub dt_s: f64,
    pub sat: String,
    pub kind: String, // slip | lli_flag | gap_unknown | clock
    pub triggered: Vec<String>,
    pub status: CandidateStatus,
    pub hypotheses: Vec<Hypothesis>,
    pub votes: Vec<DetectorVote>,
    pub reject_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockCandidate {
    pub boundary_seq: u32,
    pub from_epoch_s: f64,
    pub to_epoch_s: f64,
    pub dt_s: f64,
    pub estimated_step_m: f64,
    pub agreeing_sats: u32,
    pub total_sats: u32,
    pub declared_event_id: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComboPoint {
    pub seq: u32,
    pub epoch_s: f64,
    pub sat: String,
    pub gf_m: Option<f64>,
    pub mw_cycle: Option<f64>,
    pub gf_status: String,
    pub mw_status: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcSeg {
    pub sat: String,
    pub start_seq: u32,
    pub end_seq: u32,
    pub reason_open: String,
    pub reason_close: Option<String>,
    pub missing_freq_seqs: Vec<u32>,
    pub note: Option<String>,
}
