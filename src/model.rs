use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub id: String,
    pub raw_id: String,
    pub epoch_idx: usize,
    pub tow_s: f64,
    pub epoch_text: String,
    pub epoch_flag: String,
    pub sat: String,
    pub code: String,
    pub band: Option<u8>,
    pub obs_class: String,
    pub value_raw: String,
    pub value: Option<f64>,
    pub lli: Option<u8>,
    pub ssi: Option<u8>,
    pub unit: String,
    pub datum: String,
    pub raw_line: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub tow_s: f64,
    pub kind: String,
    pub offset: Quantity,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFile {
    pub id: String,
    pub filename: String,
    pub kind: String,
    pub content: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub raw_id: String,
    pub kind: String,
    pub observations: Vec<RawObservation>,
    pub clock_events: Vec<ClockEvent>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectParams {
    pub gap_threshold_s: f64,
    pub td_sigma_threshold: f64,
    pub gf_sigma_threshold: f64,
    pub mw_sigma_threshold: f64,
    pub elev_mask_deg: f64,
    pub clock_jump_common_ratio: f64,
    pub clock_jump_min_s: f64,
    pub lli_confirm_sigma: f64,
    pub max_gf_slip_cycles: i32,
    pub max_mw_slip_cycles: i32,
    pub weight_td: f64,
    pub weight_gf: f64,
    pub weight_mw: f64,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gap_threshold_s: 120.0,
            td_sigma_threshold: 4.0,
            gf_sigma_threshold: 4.0,
            mw_sigma_threshold: 4.0,
            elev_mask_deg: 5.0,
            clock_jump_common_ratio: 0.8,
            clock_jump_min_s: 1e-7,
            lli_confirm_sigma: 2.0,
            max_gf_slip_cycles: 5,
            max_mw_slip_cycles: 5,
            weight_td: 1.0,
            weight_gf: 1.0,
            weight_mw: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamsVersion {
    pub id: String,
    pub params: DetectParams,
    pub fingerprint: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryKey {
    pub station: String,
    pub sat: String,
    pub band: u8,
    pub code: String,
    pub arc: u32,
    pub boundary_epoch: usize,
}

impl BoundaryKey {
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.station, self.sat, self.band, self.code, self.arc, self.boundary_epoch
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorCandidate {
    pub detector: String,
    pub slip_cycles: Option<f64>,
    pub slip_pair: Option<(i32, i32)>,
    #[serde(default)]
    pub alternates: Vec<(i32, i32, f64)>,
    pub residual: f64,
    pub sigma: f64,
    pub score: f64,
    pub status: String,
    pub reason: Option<String>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub run_id: String,
    pub boundary: BoundaryKey,
    pub slip_cycles: f64,
    pub unit: String,
    pub score: f64,
    pub status: String,
    pub supporting: Vec<String>,
    pub rank: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryEval {
    pub id: String,
    pub run_id: String,
    pub boundary: BoundaryKey,
    pub deps_fingerprint: String,
    pub kind: String,
    pub detectors: Vec<DetectorCandidate>,
    pub candidates: Vec<Candidate>,
    pub clock_jump: bool,
    pub lli_flag: bool,
    pub lli_verdict: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComboPoint {
    pub epoch_idx: usize,
    pub tow_s: f64,
    pub l4_m: Option<f64>,
    pub mw_m: Option<f64>,
    pub td_phase_m: Option<f64>,
    pub td_code_m: Option<f64>,
    pub elev_deg: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcSeries {
    pub station: String,
    pub sat: String,
    pub band: u8,
    pub code: String,
    pub arc: u32,
    pub points: Vec<ComboPoint>,
    pub boundaries: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcInfo {
    pub station: String,
    pub sat: String,
    pub band: u8,
    pub code: String,
    pub arc: u32,
    pub start_epoch: usize,
    pub end_epoch: usize,
    pub start_tow_s: f64,
    pub end_tow_s: f64,
    pub n_epochs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub algo_version: String,
    pub params_fingerprint: String,
    pub params_id: String,
    pub raw_ids: Vec<String>,
    pub created_event: i64,
    pub stale: bool,
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub candidate_id: String,
    pub boundary_key: String,
    pub action: String,
    pub rationale: String,
    pub actor: String,
    pub created_event: i64,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetLayer {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub boundary_key: String,
    pub slip_cycles: f64,
    pub unit: String,
    pub source_decision: String,
    pub created_event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub project_id: String,
    pub branch: String,
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub parents: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created_seq: i64,
}
