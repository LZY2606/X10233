//! Domain model. Every domain quantity carries a unit or a datum basis;
//! raw representations are preserved verbatim at import, and normalization
//! happens only in the derived layer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const GPS_L1_HZ: f64 = 1_575_420_000.0;
pub const GPS_L2_HZ: f64 = 1_227_600_000.0;
pub const C_MPS: f64 = 299_792_458.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Band {
    L1,
    L2,
    L5,
    Unknown(u8),
}

impl Band {
    pub fn from_num(n: u8) -> Band {
        match n {
            1 => Band::L1,
            2 => Band::L2,
            5 => Band::L5,
            o => Band::Unknown(o),
        }
    }
    pub fn num(&self) -> u8 {
        match self {
            Band::L1 => 1,
            Band::L2 => 2,
            Band::L5 => 5,
            Band::Unknown(n) => *n,
        }
    }
    pub fn label(&self) -> String {
        match self {
            Band::L1 => "L1".into(),
            Band::L2 => "L2".into(),
            Band::L5 => "L5".into(),
            Band::Unknown(n) => format!("L{}", n),
        }
    }
    pub fn freq_hz(&self) -> Option<f64> {
        match self {
            Band::L1 => Some(GPS_L1_HZ),
            Band::L2 => Some(GPS_L2_HZ),
            _ => None,
        }
    }
    pub fn wavelength_m(&self) -> Option<f64> {
        self.freq_hz().map(|f| C_MPS / f)
    }
}

/// A raw observation value as submitted: the original text is preserved;
/// the parsed numeric value may be absent when it cannot be converted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawValue {
    pub raw: String,
    pub value: Option<f64>,
    /// Diagnostic when `value` is None or conversion was lossy.
    pub diag: Option<String>,
}

impl RawValue {
    pub fn ok(raw: &str, v: f64) -> Self {
        RawValue { raw: raw.to_string(), value: Some(v), diag: None }
    }
    pub fn bad(raw: &str, diag: &str) -> Self {
        RawValue { raw: raw.to_string(), value: None, diag: Some(diag.to_string()) }
    }
}

/// One (sat, band) observation at one epoch, verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObs {
    pub station: String,
    pub sat: String,
    pub epoch_iso: String,
    pub band: Band,
    /// Original observation code, e.g. "C1C", "L2W".
    pub code: String,
    /// Raw field name -> raw value. Keys preserved verbatim (C, L, D, S, LLI).
    pub fields: BTreeMap<String, RawValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectorKind {
    TimeDifference,
    GeometryFree,
    MelbourneWubbena,
}

impl DetectorKind {
    pub fn label(&self) -> &'static str {
        match self {
            DetectorKind::TimeDifference => "TD",
            DetectorKind::GeometryFree => "GF",
            DetectorKind::MelbourneWubbena => "MW",
        }
    }
    pub fn all() -> [DetectorKind; 3] {
        [DetectorKind::TimeDifference, DetectorKind::GeometryFree, DetectorKind::MelbourneWubbena]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Proposed,
    Ambiguous,
    Rejected,
    Accepted,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionKind {
    Accept,
    Reject,
    AdjustParams,
}

/// Machine-readable judgement event kinds (the counterexample archive schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JudgeKind {
    StationClockJump,
    SatCycleSlip,
    LliFalsePositive,
    DetectorDisagreement,
    AmbiguousOffsets,
    ArcLocalExpiry,
    DecisionConflict,
    ParamRevision,
}

impl JudgeKind {
    pub fn label(&self) -> &'static str {
        match self {
            JudgeKind::StationClockJump => "station_clock_jump",
            JudgeKind::SatCycleSlip => "sat_cycle_slip",
            JudgeKind::LliFalsePositive => "lli_false_positive",
            JudgeKind::DetectorDisagreement => "detector_disagreement",
            JudgeKind::AmbiguousOffsets => "ambiguous_offsets",
            JudgeKind::ArcLocalExpiry => "arc_local_expiry",
            JudgeKind::DecisionConflict => "decision_conflict",
            JudgeKind::ParamRevision => "param_revision",
        }
    }
}

/// Detector observation (residual) with unit attached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorObs {
    pub id: String,
    pub run_id: String,
    pub arc_id: String,
    pub boundary_epoch_id: String,
    pub detector: DetectorKind,
    pub band: Option<Band>,
    /// Residual in cycles of the detector's natural combination.
    pub residual_cycles: Option<f64>,
    pub unit: String,
    pub status: String, // "ok" | "abstain:<reason>" | "error:<reason>"
    pub source_obs_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub run_id: String,
    pub arc_id: String,
    pub boundary_epoch_id: String,
    pub sat: String,
    pub band: Band,
    pub slip_cycles: f64,
    pub score: f64,
    pub status: CandidateStatus,
    pub supporting_detectors: Vec<DetectorKind>,
    pub reject_reason: Option<String>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub event_id: String,
    pub branch: String,
    pub candidate_id: String,
    pub kind: DecisionKind,
    pub note: String,
    pub actor: String,
    pub at_iso: String,
    pub clock_basis: String,
}

/// A repair is only an offset layer: raw observations are never modified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOffset {
    pub id: String,
    pub sat: String,
    pub band: Band,
    pub from_epoch_id: String,
    pub offset_cycles: f64,
    pub decision_id: String,
    pub branch: String,
}

/// Computation parameters; a revision creates a new version with a parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub version: u32,
    pub parent_version: Option<u32>,
    pub fingerprint: String,
    pub gap_split_s: f64,
    pub td_sigma_cycles: f64,
    pub td_threshold_sigma: f64,
    pub gf_threshold_cycles: f64,
    pub mw_threshold_cycles: f64,
    pub clock_jump_min_sats: usize,
    pub clock_jump_threshold_cycles: f64,
    pub clock_jump_code_threshold_m: f64,
    pub search_radius_cycles: i64,
    pub lli_requires_phase_evidence: bool,
    pub note: String,
}

impl Default for Params {
    fn default() -> Self {
        let mut p = Params {
            version: 1,
            parent_version: None,
            fingerprint: String::new(),
            gap_split_s: 90.0,
            td_sigma_cycles: 0.05,
            td_threshold_sigma: 4.0,
            gf_threshold_cycles: 0.35,
            mw_threshold_cycles: 0.45,
            clock_jump_min_sats: 3,
            clock_jump_threshold_cycles: 0.8,
            clock_jump_code_threshold_m: 200.0,
            search_radius_cycles: 64,
            lli_requires_phase_evidence: true,
            note: "baseline".into(),
        };
        p.fingerprint = p.compute_fingerprint();
        p
    }
}

impl Params {
    pub fn compute_fingerprint(&self) -> String {
        crate::identity::fp(&[
            &self.version.to_string(),
            &fmt_f(self.gap_split_s),
            &fmt_f(self.td_sigma_cycles),
            &fmt_f(self.td_threshold_sigma),
            &fmt_f(self.gf_threshold_cycles),
            &fmt_f(self.mw_threshold_cycles),
            &self.clock_jump_min_sats.to_string(),
            &fmt_f(self.clock_jump_threshold_cycles),
            &fmt_f(self.clock_jump_code_threshold_m),
            &self.search_radius_cycles.to_string(),
            &self.lli_requires_phase_evidence.to_string(),
        ])
    }
}

pub fn fmt_f(v: f64) -> String {
    format!("{:.10}", v)
}
