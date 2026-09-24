//! Domain model. All domain quantities carry a unit (or a coordinate datum);
//! raw representations are preserved at import and only the derived layer
//! normalizes.
use serde::{Deserialize, Serialize};

use crate::ids;

pub const ALGO_VERSION: &str = "detectors/1.0.0";
pub const C_M_PER_S: f64 = 299_792_458.0;
pub const GPS_F1_HZ: f64 = 1_575.42e6;
pub const GPS_F2_HZ: f64 = 1_227.60e6;

/// Carrier wavelength in metres for a frequency number (GPS L1/L2).
pub fn lambda_m(freq_num: i32) -> Option<f64> {
    match freq_num {
        1 => Some(C_M_PER_S / GPS_F1_HZ),
        2 => Some(C_M_PER_S / GPS_F2_HZ),
        _ => None,
    }
}

pub fn carrier_freq_hz(freq_num: i32) -> Option<f64> {
    match freq_num {
        1 => Some(GPS_F1_HZ),
        2 => Some(GPS_F2_HZ),
        _ => None,
    }
}

/// Detector / repair parameters. The fingerprint of this struct is stamped on
/// every derived value so a parameter revision is traceable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// Time-difference residual threshold, cycles.
    pub td_threshold_cycles: f64,
    /// Geometry-free jump threshold, metres.
    pub gf_threshold_m: f64,
    /// Melbourne-Wubbena jump threshold, wide-lane cycles.
    pub mw_threshold_cycles: f64,
    /// Pairing tolerance when solving integer (n1, n2) from a GF jump, metres.
    pub gf_pair_tol_m: f64,
    /// Max |n| considered when enumerating integer slip pairs.
    pub max_slip_cycles: i32,
    /// Arc break gap, seconds.
    pub max_gap_s: f64,
    /// Minimum number of satellites that must jump coherently (in metres) at
    /// one epoch for the event to be classified as a receiver clock jump
    /// instead of per-satellite slips.
    pub clock_min_sats: usize,
    /// Consistency tolerance across satellites for a clock jump, metres.
    pub clock_consistency_m: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            td_threshold_cycles: 0.5,
            gf_threshold_m: 0.10,
            mw_threshold_cycles: 0.5,
            gf_pair_tol_m: 0.02,
            max_slip_cycles: 12,
            max_gap_s: 30.0,
            clock_min_sats: 2,
            clock_consistency_m: 1.0,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let canon = serde_json::to_string(self).unwrap_or_default();
        ids::content_id("params", &[&canon])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateKind {
    /// Per-satellite, per-frequency integer cycle slip.
    SingleSatSlip,
    /// Whole-receiver clock jump; kept strictly separate from slips.
    ClockJump,
    /// Several legal integer offset vectors explain the data; all are kept.
    AmbiguousGroup,
    /// LLI flag raised but no detector confirms a discontinuity.
    LliUnconfirmed,
    /// Detector could not decide (missing inputs); unknown is first-class.
    Unknown,
}

impl CandidateKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SingleSatSlip => "single_sat_slip",
            Self::ClockJump => "clock_jump",
            Self::AmbiguousGroup => "ambiguous_group",
            Self::LliUnconfirmed => "lli_unconfirmed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detector {
    GeometryFree,
    MelbourneWubbena,
    TimeDifference,
    LliFlag,
    ClockScan,
}

impl Detector {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GeometryFree => "geometry_free",
            Self::MelbourneWubbena => "melbourne_wubbena",
            Self::TimeDifference => "time_difference",
            Self::LliFlag => "lli_flag",
            Self::ClockScan => "clock_scan",
        }
    }
}

/// One legal repair hypothesis: integer offsets per frequency number, in
/// cycles. Repairs only ever live in this offset layer; raw observations are
/// never rewritten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlipOption {
    /// freq_num -> cycles
    pub offsets: Vec<(i32, i32)>,
}

/// Machine-readable judgement basis attached to candidates and decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgementBasis {
    /// e.g. "detectors_disagree_not_ground_truth", "clock_vs_slip_separated",
    /// "repair_offset_layer_only", "lli_false_alarm", "multi_equal_offsets"
    pub code: String,
    pub detail: String,
}

/// Channel key: one station-satellite pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Channel {
    pub station: String,
    pub sys: String,
    pub sat: String,
}

impl Channel {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.station, self.sys, self.sat)
    }
}
