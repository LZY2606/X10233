//! Persisted domain records (one row each in SQLite) and view DTOs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub counterexample_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawCommit {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub kind: String,         // obs | nav | clocks | obs_patch | narrative
    pub filename: String,
    pub raw_text: String,
    pub sha256: String,
    pub imported_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticRec {
    pub id: String,
    pub commit_id: String,
    pub line_no: i64,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRec {
    pub id: String,
    pub ord: i64,
    pub time: String,
    pub time_raw: String,
    pub flag: String,
    pub clock_offset_raw: Option<String>,
    pub clock_offset_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsRec {
    pub id: String,
    pub epoch_id: String,
    pub sat: String,
    pub code: String,
    pub band: String,
    pub kind: String,         // C|L|D|S
    pub raw: String,
    pub v: Option<f64>,
    pub unit: Option<String>,
    pub lli: Option<String>,
    pub ssi: Option<String>,
    pub diagnostic: Option<String>,
    pub effective_commit_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavRecRow {
    pub id: String,
    pub commit_id: String,
    pub sat: String,
    pub time: String,
    pub time_raw: String,
    pub kind: String,
    pub frequency_number: Option<i32>,
    pub values_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockLedgerRow {
    pub id: String,
    pub commit_id: String,
    pub time: String,
    pub source: String,
    pub magnitude_raw: String,
    pub magnitude_s: Option<f64>,
    pub unit: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub fingerprint: String,
    pub json: String,
    pub created_at: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent_branch_id: Option<String>,
    pub forked_from_event_seq: Option<i64>,
    pub created_at: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRec {
    pub seq: i64,
    pub branch_id: String,
    pub kind: String,
    pub payload_json: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: String,
    pub basis_commit_id: String,
    pub params_id: String,
    pub algo_version: String,
    pub fingerprint: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub run_id: String,
    pub scope: String,         // station | satellite
    pub sat: Option<String>,
    pub band: Option<String>,
    pub epoch_id: String,
    pub detector: String,      // gf | mw | td | lli | clock
    pub status: String,        // ok | candidate | unknown | evidence
    pub qty: Option<f64>,
    pub qty_unit: Option<String>,
    pub n1: Option<i64>,
    pub n2: Option<i64>,
    pub distance: Option<f64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub run_id: String,
    pub sat: String,
    pub epoch_id: String,
    pub n1: i64,
    pub n2: i64,
    pub score: f64,
    pub votes_json: String,
    pub status: String,        // open | accepted | rejected | superseded
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub branch_id: String,
    pub at_event_seq: i64,
    pub target_kind: String,   // candidate | clock_finding | lli_finding
    pub target_id: String,
    pub target_run_id: String,
    pub action: String,        // accept | reject | parameter_change | resolve_conflict
    pub n1: Option<i64>,
    pub n2: Option<i64>,
    pub params_id: Option<String>,
    pub note: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOffset {
    pub id: String,
    pub branch_id: String,
    pub sat: String,
    pub band: String,
    pub from_epoch_id: String,
    pub cycles: i64,
    pub layer: String,         // always "offset_layer"
    pub decision_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcRevision {
    pub id: String,
    pub branch_id: String,
    pub sat: String,
    pub band: String,
    pub at_event_seq: i64,
    pub arc_before_id: String,
    pub arc_after_id: String,
    pub decision_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DetParams {
    pub gf_tol_m: f64,
    pub wl_tol_cycle: f64,
    pub td_sigma_m: f64,
    pub clock_fraction: f64,
    pub clock_common_m: f64,
    pub arc_gap_s: f64,
    pub n_min: i64,
    pub n_max: i64,
    pub iono_drift_est_s: f64,
}

impl DetParams {
    pub fn initial() -> Self {
        DetParams {
            gf_tol_m: 0.04,
            wl_tol_cycle: 0.35,
            td_sigma_m: 0.012,
            clock_fraction: 0.7,
            clock_common_m: 50_000.0,
            arc_gap_s: 20.0,
            n_min: -12,
            n_max: 12,
            iono_drift_est_s: 60.0,
        }
    }
    pub fn fingerprint(&self) -> String {
        format!(
            "gf{}|wl{}|td{}|cf{}|cm{}|gap{}|n{}:{}|drift{}",
            self.gf_tol_m, self.wl_tol_cycle, self.td_sigma_m, self.clock_fraction,
            self.clock_common_m, self.arc_gap_s, self.n_min, self.n_max, self.iono_drift_est_s
        )
    }
}
