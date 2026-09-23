//! Cycle-slip / multi-frequency combination engine.
//!
//! Three independent detectors contribute candidates:
//! * **GF** — geometry-free (ionosphere) carrier combination, in metres;
//! * **MW** — Melbourne-Wubbena wide-lane combination, in WL cycles;
//! * **TD** — time-difference (phase vs Doppler/code prediction), in cycles.
//! No single detector is treated as truth. Clock jumps are classified as a
//! separate common-mode candidate. Repairs are emitted only as an integer
//! offset layer; raw observations are never mutated.
//!
//! See submodules: [`combo`] for the combination math, [`build`] for the
//! arc/candidate state machine.

use crate::domain::{Band, Params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Numeric view of one imported observation cell available to derivation.
#[derive(Debug, Clone)]
pub struct CellNum {
    pub value: Option<f64>,
    pub unit: String,
    pub original: String,
    pub diagnostic: Option<String>,
}

/// Current effective observation for an epoch/satellite/band.
#[derive(Debug, Clone)]
pub struct BandObs {
    pub epoch_order: i64,
    pub epoch_id: String,
    pub sat: String,
    pub band: Band,
    pub lli: i64,
    pub source_obs_version: String,
    pub cells: BTreeMap<&'static str, CellNum>,
}

#[derive(Debug, Clone)]
pub struct ClockPoint {
    pub epoch_order: i64,
    pub epoch_id: String,
    pub receiver_offset_s: Option<f64>,
    pub has_clock_event: bool,
    pub magnitude_s: Option<f64>,
    pub source_obs_version: String,
}

#[derive(Debug, Clone)]
pub struct EngineInput {
    pub case_id: String,
    pub import_id: String,
    pub params_version: String,
    pub params: Params,
    pub epochs: Vec<EpochMeta>,
    pub bands: Vec<Band>,
    pub signals: BTreeMap<Band, SignalMeta>,
    pub rows: Vec<BandObs>,
    pub clocks: Vec<ClockPoint>,
    pub input_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct EpochMeta {
    pub order: i64,
    pub id: String,
    pub gps_seconds: f64,
}

#[derive(Debug, Clone)]
pub struct SignalMeta {
    pub frequency_hz: f64,
    pub wavelength_m: f64,
}

/// One multi-frequency residual sample in the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualPoint {
    pub epoch_id: String,
    pub sat: String,
    pub gf_m: Option<f64>,
    pub gf_delta_m: Option<f64>,
    pub mw_wl_cycle: Option<f64>,
    pub mw_delta_cycle: Option<f64>,
    pub td_l1_cycle: Option<f64>,
    pub td_l2_cycle: Option<f64>,
    pub doppler_l1_hz: Option<f64>,
    pub doppler_l2_hz: Option<f64>,
    pub arc_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcOut {
    pub arc_id: String,
    pub sat: String,
    pub band_scope: String, // `1+2`, `1`, ...
    pub start_epoch: String,
    pub end_epoch: String,
    pub length: i64,
    pub boundaries: Vec<ArcBoundary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcBoundary {
    pub epoch_id: String,
    pub kinds: Vec<String>, // gap | frequency_missing | clock_jump | lli
    pub candidate_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypothesisOut {
    pub n1: i64,
    pub n2: Option<i64>,
    pub score: f64,
    pub adopted: bool,
    pub rank: i64,
    pub admissible: bool,
    pub rejections: Vec<RejectionOut>,
    pub gf_residual_m: Option<f64>,
    pub mw_residual_cycle: Option<f64>,
    pub td_residual_l1_cycle: Option<f64>,
    pub td_residual_l2_cycle: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectionOut {
    pub detector: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateOut {
    pub candidate_id: String,
    pub sat: String,
    pub epoch_id: String,
    pub kind: String, // slip | clock_jump
    pub status: String, // open | accepted | rejected | superseded | ambiguous
    pub triggers: Vec<String>,
    pub gf_jump_m: Option<f64>,
    pub mw_jump_cycle: Option<f64>,
    pub td_l1_cycle: Option<f64>,
    pub td_l2_cycle: Option<f64>,
    pub gf_predicted_m: Option<f64>,
    pub mw_predicted_cycle: Option<f64>,
    pub ambiguity: bool,
    pub ambiguity_tie_count: i64,
    pub hypotheses: Vec<HypothesisOut>,
    pub evidence: Vec<EvidenceOut>,
    pub note: String,
    /// satellite ids sharing this common-mode jump (clock candidates only).
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceOut {
    pub detector: String,
    pub residual: Option<f64>,
    pub unit: String,
    pub predicted: Option<f64>,
    pub observed: Option<f64>,
    pub threshold: f64,
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildReport {
    pub arcs: Vec<ArcOut>,
    pub residuals: Vec<ResidualPoint>,
    pub candidates: Vec<CandidateOut>,
    pub affected_arcs: Vec<String>,
}

pub mod combo;
pub mod build;
