use serde::{Deserialize, Serialize};

pub const ALGORITHM_VERSION: &str = "carrier-break-engine-2026.09.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CasePackage {
    pub schema_version: String,
    pub case_id: String,
    pub title: String,
    pub narrative: String,
    pub coordinate_reference: String,
    pub time_scale: String,
    #[serde(default)]
    pub seed: i64,
    pub station: Station,
    pub frequencies: Vec<FrequencyDef>,
    pub observation_columns: Vec<String>,
    pub observations: Vec<RawObservationInput>,
    pub epochs: Vec<EpochDef>,
    pub satellites: Vec<SatelliteDef>,
    #[serde(default)]
    pub satellite_states: Vec<SatelliteState>,
    #[serde(default)]
    pub clock_events: Vec<ClockEvent>,
    #[serde(default)]
    pub initial_parameters: DetectorParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionPackage {
    pub schema_version: String,
    pub revision_id: String,
    pub parent_event_id: String,
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub parameters: Option<DetectorParameters>,
    #[serde(default)]
    pub observation_key: Option<ObservationKey>,
    #[serde(default)]
    pub raw_text: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub lli: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Station {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub receiver: String,
    #[serde(default)]
    pub antenna: String,
    #[serde(default)]
    pub position_m: Vec<f64>,
    #[serde(default)]
    pub coordinate_reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyDef {
    pub constellation: String,
    pub frequency_number: i64,
    pub code: String,
    pub carrier_hz: f64,
    pub wavelength_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochDef {
    pub id: String,
    pub timestamp: String,
    pub virtual_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteDef {
    pub id: String,
    pub constellation: String,
    pub prn: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteState {
    pub epoch_id: String,
    pub satellite: String,
    pub position_m: Vec<f64>,
    #[serde(default)]
    pub velocity_mps: Vec<f64>,
    #[serde(default)]
    pub coordinate_reference: String,
    #[serde(default)]
    pub clock_bias_s: f64,
    #[serde(default)]
    pub health: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub epoch_id: String,
    pub scope: String,
    pub delta_seconds: f64,
    pub delta_m: f64,
    pub source_text: String,
    #[serde(default)]
    pub applies_to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservationInput(
    pub String,
    pub String,
    pub Option<String>,
    pub Option<i64>,
    pub Option<String>,
    pub Option<String>,
    pub Option<i64>,
    pub Option<String>,
);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ObservationKey {
    pub epoch_id: String,
    pub satellite: String,
    pub observation_code: String,
    pub frequency_number: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParameters {
    #[serde(default = "default_algorithm_version")]
    pub algorithm_version: String,
    #[serde(default = "default_iono_threshold")]
    pub iono_cycles_threshold: f64,
    #[serde(default = "default_mw_threshold")]
    pub mw_cycles_threshold: f64,
    #[serde(default = "default_td_threshold")]
    pub time_difference_cycles_threshold: f64,
    #[serde(default = "default_lli_weight")]
    pub lli_score_weight: f64,
    #[serde(default = "default_tie_half_cycle")]
    pub tie_half_cycle: f64,
}

fn default_algorithm_version() -> String {
    ALGORITHM_VERSION.to_string()
}
fn default_iono_threshold() -> f64 { 0.55 }
fn default_mw_threshold() -> f64 { 0.45 }
fn default_td_threshold() -> f64 { 0.45 }
fn default_lli_weight() -> f64 { 0.35 }
fn default_tie_half_cycle() -> f64 { 0.5 }

impl Default for DetectorParameters {
    fn default() -> Self {
        Self {
            algorithm_version: default_algorithm_version(),
            iono_cycles_threshold: default_iono_threshold(),
            mw_cycles_threshold: default_mw_threshold(),
            time_difference_cycles_threshold: default_td_threshold(),
            lli_score_weight: default_lli_weight(),
            tie_half_cycle: default_tie_half_cycle(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub stable_id: String,
    pub event_id: String,
    pub epoch_id: String,
    pub satellite: String,
    pub observation_code: String,
    pub frequency_number: i64,
    pub raw_text: String,
    pub unit: String,
    pub lli: Option<i64>,
    pub snr_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedValue {
    pub stable_id: String,
    pub raw_id: String,
    pub value: Option<f64>,
    pub unit: String,
    pub diagnostics: Vec<String>,
    pub direct_source_event_id: String,
    pub algorithm_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Residual {
    pub stable_id: String,
    pub detector: String,
    pub from_epoch_id: String,
    pub epoch_id: String,
    pub satellite: String,
    pub frequency_number: Option<i64>,
    pub value: Option<f64>,
    pub unit: String,
    pub status: String,
    pub diagnostics: Vec<String>,
    pub direct_sources: Vec<String>,
    pub algorithm_version: String,
    pub parameter_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateOption {
    pub stable_id: String,
    pub group_id: String,
    pub detector: String,
    pub observed_slip_cycles: Option<f64>,
    pub offset_cycles: f64,
    pub score: f64,
    pub status: String,
    pub reject_reason: Option<String>,
    pub direct_residual_ids: Vec<String>,
    pub parameter_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateGroup {
    pub stable_id: String,
    pub from_epoch_id: String,
    pub epoch_id: String,
    pub satellite: String,
    pub frequency_number: Option<i64>,
    pub kind: String,
    pub status: String,
    pub reject_reason: Option<String>,
    pub options: Vec<CandidateOption>,
    pub decision: Option<Decision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub event_id: String,
    pub branch_id: String,
    pub action: String,
    pub selected_offset_cycles: Option<f64>,
    pub reason: String,
    pub parent_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOffset {
    pub stable_id: String,
    pub candidate_group_id: String,
    pub satellite: String,
    pub frequency_number: i64,
    pub observed_slip_cycles: f64,
    pub repair_offset_cycles: f64,
    pub unit: String,
    pub decision_event_id: String,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcSegment {
    pub stable_id: String,
    pub satellite: String,
    pub frequency_number: i64,
    pub start_epoch_id: String,
    pub end_epoch_id: String,
    pub state: String,
    pub boundary_reason: Option<String>,
    pub candidate_group_id: Option<String>,
    pub generated_by_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: String,
    pub branch_id: String,
    pub seq: i64,
    pub parent_event_id: Option<String>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_virtual_time: String,
}

pub fn raw_stable_id(key: &ObservationKey) -> String {
    format!("raw:{}:{}:{}:f{}", key.epoch_id, key.satellite, key.observation_code, key.frequency_number)
}

pub fn normalize_value(raw_text: &str, unit: &str) -> (Option<f64>, Vec<String>) {
    match raw_text.trim().parse::<f64>() {
        Ok(value) => (Some(value), Vec::new()),
        Err(_) => (
            None,
            vec![format!("unconvertible_value:{raw_text}:{unit}; raw representation retained at source")],
        ),
    }
}

pub fn supported_unit(unit: &str) -> bool {
    matches!(unit, "m" | "cycles" | "Hz" | "dB-Hz" | "s")
}
