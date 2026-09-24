//! Domain model. Every value that can be displayed carries a stable identity.
//! Raw observations are immutable once imported; everything derived records
//! algorithm version, parameter fingerprint and direct source ids.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ALGO_VERSION: &str = "cbps-detect/1.2.0";

pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    for p in parts {
        h.update(b"\x1f");
        h.update(p.as_bytes());
    }
    let hex = hex_encode(&h.finalize());
    format!("{}_{}", prefix, &hex[..16])
}

pub fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    RinexObs,
    ClockEvents,
    SatStates,
    Params,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::RinexObs => "rinex_obs",
            SourceKind::ClockEvents => "clock_events",
            SourceKind::SatStates => "sat_states",
            SourceKind::Params => "params",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rinex_obs" => Some(SourceKind::RinexObs),
            "clock_events" => Some(SourceKind::ClockEvents),
            "sat_states" => Some(SourceKind::SatStates),
            "params" => Some(SourceKind::Params),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceVersion {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: SourceKind,
    pub label: String,
    pub content: String,
    pub content_hash: String,
    pub submitted_seq: i64,
    pub submitted_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
}

impl ObsKind {
    pub fn from_code(code: &str) -> Option<Self> {
        match code.chars().next()? {
            'C' => Some(ObsKind::Pseudorange),
            'L' => Some(ObsKind::Carrier),
            'D' => Some(ObsKind::Doppler),
            'S' => Some(ObsKind::Snr),
            _ => None,
        }
    }
    pub fn unit(&self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "m",
            ObsKind::Carrier => "cycles",
            ObsKind::Doppler => "Hz",
            ObsKind::Snr => "dB-Hz",
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "pseudorange",
            ObsKind::Carrier => "carrier",
            ObsKind::Doppler => "doppler",
            ObsKind::Snr => "snr",
        }
    }
}

/// One raw observation value, kept exactly as imported. `value` is None when
/// the field was blank or unconvertible; the raw text is always preserved and
/// a diagnostic row explains why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsValue {
    pub id: String,
    pub source_id: String,
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub epoch_raw: String,
    pub code: String,
    pub band: Option<String>,
    pub channel: Option<i32>,
    pub kind: ObsKind,
    pub value: Option<f64>,
    pub value_raw: String,
    pub unit: String,
    pub lli: u8,
    pub ssi: u8,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub source_id: String,
    pub station: String,
    pub epoch_ms: i64,
    pub epoch_raw: String,
    pub jump_ms: f64,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub id: String,
    pub source_id: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub epoch_raw: String,
    pub x_km: f64,
    pub y_km: f64,
    pub z_km: f64,
    pub clock_us: f64,
    pub datum: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub id: String,
    pub source_id: String,
    pub severity: String, // "info" | "warning" | "error"
    pub message: String,
    pub raw_fragment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub channel: Option<i32>,
    pub start_epoch_ms: i64,
    pub end_epoch_ms: i64,
    pub epoch_count: i64,
    pub input_fp: String,
    pub algo_version: String,
    pub param_fp: String,
    pub created_seq: i64,
    pub superseded_seq: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Proposed,
    Undetermined,
    Accepted,
    Rejected,
    AutoRejected,
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateStatus::Proposed => "proposed",
            CandidateStatus::Undetermined => "undetermined",
            CandidateStatus::Accepted => "accepted",
            CandidateStatus::Rejected => "rejected",
            CandidateStatus::AutoRejected => "auto_rejected",
        }
    }
}

/// A cycle-slip candidate. No detector is treated as ground truth; candidates
/// from different detectors may disagree and all are kept. `alternatives`
/// holds equally-legal integer offsets when the evidence cannot decide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub arc_id: String,
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub detector: String, // "gf_iono" | "mw" | "td_doppler" | "lli_flag" | "joint_l1l2" | "clock_common_mode"
    pub combination: String,
    pub jump: f64,
    pub jump_unit: String,
    pub score: f64,
    pub alternatives: Vec<(i32, i32)>,
    pub status: CandidateStatus,
    pub reason: Option<String>,
    pub input_fp: String,
    pub algo_version: String,
    pub param_fp: String,
    pub source_ids: Vec<String>,
    pub created_seq: i64,
    pub superseded_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub seq: i64,
    pub branch_id: String,
    pub parent_decision_id: Option<String>,
    pub action: String, // "accept" | "reject" | "undo" | "resolve_conflict"
    pub candidate_ids: Vec<String>,
    pub chosen_alternative: Option<(i32, i32)>,
    pub note: String,
    pub actor: String,
    pub decided_ms: i64,
}

/// Repairs live only in an offset layer; raw observations are never modified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOffset {
    pub id: String,
    pub arc_id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub from_epoch_ms: i64,
    pub cycles: i32,
    pub decision_id: String,
    pub branch_id: String,
    pub active: bool,
    pub created_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub ts_ms: i64,
    pub payload: serde_json::Value,
}

/// Detector parameters. A parameter revision is itself a versioned source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub arc_max_gap_s: f64,
    pub gf_threshold_m: f64,
    pub gf_sigma_m: f64,
    pub mw_threshold_cycles: f64,
    pub mw_sigma_cycles: f64,
    pub td_threshold_cycles: f64,
    pub td_sigma_cycles: f64,
    pub clock_common_mode_cycles: f64,
    pub lli_corroboration_window_s: f64,
    pub ambiguity_tie_epsilon: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            arc_max_gap_s: 300.0,
            gf_threshold_m: 0.10,
            gf_sigma_m: 0.03,
            mw_threshold_cycles: 0.5,
            mw_sigma_cycles: 0.15,
            td_threshold_cycles: 0.5,
            td_sigma_cycles: 0.2,
            clock_common_mode_cycles: 1000.0,
            lli_corroboration_window_s: 90.0,
            ambiguity_tie_epsilon: 0.05,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        let s = serde_json::to_string(self).unwrap();
        stable_id("pfp", &[&s])
    }
    pub fn merge_json(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let mut cur = serde_json::to_value(&self).map_err(|e| e.to_string())?;
        let obj = v.as_object().ok_or("params must be a JSON object")?;
        for (k, val) in obj {
            if !cur.get(k).is_some() {
                return Err(format!("unknown parameter: {}", k));
            }
            cur[k] = val.clone();
        }
        *self = serde_json::from_value(cur).map_err(|e| format!("bad parameter value: {}", e))?;
        Ok(())
    }
}
