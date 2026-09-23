//! Core domain model: identifiers, quantities with units, events and nodes.
//!
//! Rule of the house: imported material is stored exactly as authored
//! (`raw_token`). Normalization to SI happens only on the derived layer, and a
//! quantity that cannot be converted stays at its source location with a
//! diagnostic instead of being dropped.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ALGO_VERSION: &str = "engine/cycle-slip-1.0.0";

/// Stable identity. Content-derived hashes are prefixed so the kind of an id
/// is self-describing in UI and export bundles.
pub fn sid(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b"\x1f");
        }
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    format!("{}_{}", prefix, hex8(&d))
}

fn hex8(d: &[u8]) -> String {
    let mut s = String::with_capacity(16);
    for b in &d[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Fingerprint of detector parameters (parameter payload + version).
pub fn fingerprint(params: &crate::engine::DetParams) -> String {
    let j = serde_json::to_string(params).expect("params serialize");
    sid("pfp", &[ALGO_VERSION, &j])
}

/// One raw token pair, e.g. `L1C[cyc]=105749636.3371`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawToken {
    pub key: String,
    pub unit: Option<String>,
    pub raw_value: String,
    /// Normalized value in the declared unit family (m, s, Hz, cyc, dBHz, deg).
    /// `None` when the token cannot be converted; `diagnostic` then explains it.
    pub value: Option<f64>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrequencyDef {
    pub id: String,
    pub system: String,
    pub band: String,
    pub freq_hz: f64,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpochRec {
    pub epoch_id: String,
    pub ord: i64,
    pub t0: String,
    pub offset_s: f64,
}

/// One SAT line: position/status fields plus observation tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatObs {
    pub row_id: String,
    pub batch_id: String,
    pub epoch_id: String,
    pub ord: i64,
    pub satellite: String,
    pub tokens: Vec<RawToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEventRec {
    pub id: String,
    pub batch_id: String,
    pub epoch_id: String,
    pub kind: String,
    pub delta_s: f64,
    pub source: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub batch_id: String,
    pub parent_batch_id: Option<String>,
    pub epochs: usize,
    pub rows: usize,
    pub clock_events: usize,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportDiagnostic {
    pub line_no: usize,
    pub severity: String, // error | warning
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRec {
    pub id: String,
    pub seq: i64,
    pub branch: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub parent_event_id: Option<String>,
}

/// A derived node: arc, boundary candidate set, or repair overlay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProducedNode {
    pub id: String,
    pub slot: String,
    pub kind: String,
    pub producing_event: String,
    pub param_fp: String,
    pub direct_sources: Vec<String>,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewNode {
    pub node: ProducedNode,
    /// "current" at the viewed event, or "stale" because a non-ancestor event
    /// produced a newer node for the same slot.
    pub status: String,
    /// Id of the newer node when stale.
    pub current_node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BranchState {
    pub head_event_id: String,
    pub head_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcBody {
    pub project_id: String,
    pub satellite: String,
    pub band: String,
    pub arc_seq: i64,
    pub start_epoch_id: String,
    pub end_epoch_id: String,
    pub start_offset_s: f64,
    pub end_offset_s: f64,
    pub n_epochs: usize,
    pub split_reasons: Vec<String>,
    pub mean_elevation_deg: Option<f64>,
    pub mw_bias_wl_cycles: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorHit {
    pub detector: String,
    pub residual: f64,
    pub residual_unit: String,
    pub threshold: f64,
    pub verdict: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub offset_b1_cycles: i64,
    pub offset_b2_cycles: i64,
    pub score: i64,
    pub tied: bool,
    pub supported_by: Vec<String>,
    pub rejected_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryBody {
    pub project_id: String,
    pub satellite: String,
    pub band: String,
    pub left_epoch_id: String,
    pub right_epoch_id: String,
    pub left_offset_s: f64,
    pub right_offset_s: f64,
    pub dt_s: f64,
    pub detectors: Vec<DetectorHit>,
    pub candidates: Vec<Candidate>,
    pub clock_finding: Option<String>,
    pub lli_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairBody {
    pub project_id: String,
    pub slot: String,
    pub boundary_node_id: String,
    pub candidate_id: Option<String>,
    pub offset_b1_cycles: Option<i64>,
    pub offset_b2_cycles: Option<i64>,
    pub decision: String, // accept | reject
    pub reason: Option<String>,
    pub decided_by_event: String,
}
