use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawFrequency {
    pub system: String,
    pub carrier_code: String,
    pub mhz: f64,
    pub unit: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub line_no: i64,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawField {
    pub code: String,
    pub raw: Option<String>,
    pub value: Option<f64>,
    pub unit: String,
    pub lli: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedEpoch {
    pub epoch: String,
    pub t_seconds: f64,
    pub clock_offset_ns: Option<f64>,
    pub raw_line: String,
    pub line_no: i64,
    pub satellites: Vec<ParsedSatellite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedSatellite {
    pub sat: String,
    pub raw_line: String,
    pub line_no: i64,
    pub fields: Vec<RawField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedObservationFile {
    pub frequencies: Vec<RawFrequency>,
    pub obs_types: Vec<String>,
    pub epochs: Vec<ParsedEpoch>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusField {
    pub key: String,
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedStatusEpoch {
    pub epoch: String,
    pub t_seconds: f64,
    pub sat: String,
    pub datum: String,
    pub raw_line: String,
    pub line_no: i64,
    pub fields: Vec<StatusField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEvent {
    pub epoch: String,
    pub t_seconds: f64,
    pub kind: String,
    pub delta_seconds: f64,
    pub raw: String,
    pub line_no: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedStatusFile {
    pub datum: String,
    pub epochs: Vec<ParsedStatusEpoch>,
    pub clock_events: Vec<ClockEvent>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleParams {
    pub integer_search_radius: i64,
    pub td_sigma_cycles: f64,
    pub mw_sigma_cycles: f64,
    pub iono_tolerance_m: f64,
    pub lli_low_snr_dbhz: f64,
}

impl Default for RuleParams {
    fn default() -> Self {
        Self {
            integer_search_radius: 1,
            td_sigma_cycles: 0.20,
            mw_sigma_cycles: 0.20,
            iono_tolerance_m: 0.20,
            lli_low_snr_dbhz: 35.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionRequest {
    pub parent_run_id: String,
    pub action: String,
    pub candidate_id: String,
    pub reason: String,
    #[serde(default)]
    pub integer_search_radius: Option<i64>,
}

pub const ALGORITHM_VERSION: &str = "cycle-slip-engine@2026.09.25-r1";

pub fn obs_kind(code: &str) -> &'static str {
    match code.chars().next() {
        Some('C') => "pseudorange",
        Some('L') => "carrier",
        Some('D') => "doppler",
        Some('S') => "snr",
        _ => "unknown",
    }
}

pub fn band(code: &str) -> String {
    code.chars().skip(1).collect()
}

pub fn carrier_for(code: &str) -> Option<String> {
    if code.starts_with('C') {
        Some(format!("L{}", &code[1..]))
    } else {
        None
    }
}
