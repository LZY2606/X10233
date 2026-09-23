//! Domain data model. Raw rows keep the exact original representation; only
//! derived layers normalize. Every domain quantity carries a unit or datum.

use serde::{Deserialize, Serialize};

/// One observable cell exactly as present in an epoch record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObs {
    pub id: String,
    pub version_id: String,
    pub epoch_s: f64,
    pub epoch_repr: String,
    pub sat: String,
    pub system: String,
    pub code: String,
    pub kind: String, // C=pseudorange(m), L=carrier phase(cycle), D=Doppler(Hz)
    pub band: String, // L1C / L2W / ...
    pub raw_text: String,
    pub value: Option<f64>,
    pub lli: Option<String>,
    pub snr: Option<String>,
    pub unit: String,
    pub parse_diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEvent {
    pub id: String,
    pub version_id: String,
    pub epoch_s: f64,
    pub scope: String, // ALL_SATELLITES | single sat
    pub kind: String,
    pub offset_s: Option<f64>,
    pub unit: String,
    pub source: String,
    pub note: String,
    pub raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSatState {
    pub id: String,
    pub version_id: String,
    pub epoch_s: f64,
    pub sat: String,
    pub health: String,
    pub elevation_deg: Option<f64>,
    pub azimuth_deg: Option<f64>,
    pub coordinate_ref: String,
    pub raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InputBatch {
    pub observations: Vec<RawObs>,
    pub clock_events: Vec<RawClockEvent>,
    pub sat_states: Vec<RawSatState>,
    /// Structural parser diagnostics; a non-empty list rejects the whole file.
    pub file_errors: Vec<String>,
}

/// Tunable detector parameters. Fingerprinted; a revision supersedes parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    pub gf_cycles: f64,
    pub mw_cycles: f64,
    pub td_cycles: f64,
    pub min_elevation_deg: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            gf_cycles: 0.15,
            mw_cycles: 0.5,
            td_cycles: 0.25,
            min_elevation_deg: 10.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub epoch_s: f64,
    pub kind: String,
    pub detector: String,
    pub residual: Option<f64>,
    pub residual_unit: String,
    pub score: f64,
    pub options: Vec<f64>,
    pub ambiguous: bool,
    pub rationale: String,
    pub rejection_reasons: Vec<String>,
    pub run_id: String,
    pub status: String, // proposed | accepted | rejected | stale
    pub direct_sources: Vec<String>,
    pub algorithm_version: String,
    pub params_fp: String,
    pub linked_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcSeg {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub start_epoch_s: f64,
    pub end_epoch_s: f64,
    pub status: String, // current | stale
    pub break_epochs_s: Vec<f64>,
    pub run_id: String,
    pub direct_sources: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetLayer {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub epoch_s: f64,
    pub cycles: f64,
    pub unit: String,
    pub candidate_id: String,
    pub decision_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorStep {
    pub detector: String,
    pub algorithm_version: String,
    pub epoch_s: f64,
    pub sat: String,
    pub band: String,
    pub inputs: Vec<String>,
    pub value: Option<f64>,
    pub unit: String,
    pub threshold: f64,
    pub fired: bool,
}
