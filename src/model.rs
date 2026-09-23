//! Domain model. Every quantity keeps its unit / coordinate reference and
//! every derived value keeps its algorithm version, parameter fingerprint and
//! direct sources.

use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "carrier-break-core-1.0.0";
pub const C_LIGHT: f64 = 299_792_458.0;

/// Observation code kinds as encoded in RINEX (kept verbatim alongside).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    SignalStrength,
}

impl ObsKind {
    pub fn from_code(code: &str) -> Option<ObsKind> {
        match code.chars().next() {
            Some('C') => Some(ObsKind::Pseudorange),
            Some('L') => Some(ObsKind::Carrier),
            Some('D') => Some(ObsKind::Doppler),
            Some('S') => Some(ObsKind::SignalStrength),
            _ => None,
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "cycle(code)",
            ObsKind::Carrier => "cycle",
            ObsKind::Doppler => "Hz",
            ObsKind::SignalStrength => "snr:1..9",
        }
    }
}

/// One observation field exactly as imported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawField {
    pub code: String,
    pub kind: ObsKind,
    pub raw_text: String,
    pub value: Option<f64>,
    pub lli: i64,
    pub ss: String,
    pub diagnostic: Option<String>,
}

/// One satellite at one epoch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSatEpoch {
    pub sat: String,
    pub fields: Vec<RawField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    pub line_no: i64,
    pub gps_ns: i64,
    pub epoch_flag: i64,
    pub clock_offset_s: Option<f64>,
    pub sat_epochs: Vec<RawSatEpoch>,
    pub raw_header: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub line_no: i64,
    pub at_second: f64,
    pub magnitude_s: f64,
    pub scope: String,
    pub raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreqEntry {
    pub sat: String,
    pub freq_mhz: f64,
    pub freq_number: i64,
    pub wavelength_m: f64,
    pub raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDoc {
    pub station: String,
    pub datum: Option<String>,
    pub frame_epoch: Option<String>,
    pub approx_xyz_m: Option<[f64; 3]>,
    pub codes: Vec<String>,
    pub raw_header_lines: Vec<String>,
    pub epochs: Vec<RawEpoch>,
    pub clock_events: Vec<ClockEvent>,
    pub amendment_of: Option<String>,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDoc {
    pub datum: Option<String>,
    pub frame_epoch: Option<String>,
    pub frequencies: Vec<FreqEntry>,
    pub clock_events: Vec<ClockEvent>,
    pub raw_lines: Vec<String>,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportDiagnostic {
    pub line_no: i64,
    pub severity: String, // ERROR | WARNING
    pub code: String,
    pub message: String,
    pub raw_text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub gf_tol_cyc: f64,
    pub td_tol_cyc: f64,
    pub mw_tol_cyc: f64,
    pub mw_tie_eps_cyc: f64,
    pub gap_factor: f64,
    pub max_search_cyc: i64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            gf_tol_cyc: 0.25,
            td_tol_cyc: 0.25,
            mw_tol_cyc: 0.25,
            mw_tie_eps_cyc: 0.05,
            gap_factor: 6.0,
            max_search_cyc: 12,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorCandidate {
    pub detector: String, // GF_IONO | MELBOURNE_WUBBENA | TIME_DIFF | LLI
    pub n1: i64,
    pub n2: i64,
    pub residual: f64,
    pub residual_unit: String,
    pub score: f64,
    pub rejected_reason: Option<String>,
}
