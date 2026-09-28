//! Domain model. All domain quantities carry a unit (or a coordinate datum);
//! raw representations are preserved alongside normalized values.

use serde::{Deserialize, Serialize};

/// GPS time: week + seconds-of-week. Supports unequal epoch intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GpsTime {
    pub week: i32,
    /// seconds of week; stored as integer milliseconds to stay exact
    pub sow_ms: i64,
}

impl GpsTime {
    pub fn new(week: i32, sow_s: f64) -> Self {
        GpsTime { week, sow_ms: (sow_s * 1000.0).round() as i64 }
    }
    pub fn to_seconds(&self) -> f64 {
        self.week as f64 * 604800.0 + self.sow_ms as f64 / 1000.0
    }
    pub fn sow_s(&self) -> f64 {
        self.sow_ms as f64 / 1000.0
    }
    /// seconds from self to other (other - self)
    pub fn dt_to(&self, other: &GpsTime) -> f64 {
        other.to_seconds() - self.to_seconds()
    }
    pub fn label(&self) -> String {
        format!("W{}:{:010.3}", self.week, self.sow_s())
    }
}

/// One raw observation value, exactly as imported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub import_id: String,
    pub station: String,
    pub sat: String,
    pub epoch: GpsTime,
    /// observation code as written, e.g. C1C / L1C / D1C / S1C / L2W
    pub obs_code: String,
    /// frequency slot parsed from the code (1,2,5,...) or 0 if unknown
    pub freq_slot: i32,
    /// value exactly as written in the source (raw representation preserved)
    pub value_raw: String,
    /// normalized value; None when not convertible (diagnostic attached)
    pub value: Option<f64>,
    /// unit of the quantity: m | cyc | Hz | dBHz | unknown
    pub unit: String,
    /// loss-of-lock indicator bits as written (0-7), -1 when absent
    pub lli: i32,
    /// diagnostic attached at import time (e.g. "unparseable value")
    pub diag: Option<String>,
}

/// Receiver clock-jump event, imported as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub import_id: String,
    pub station: String,
    pub epoch: GpsTime,
    pub jump_ms_raw: String,
    pub jump_ms: Option<f64>,
    pub diag: Option<String>,
}

/// An arc: contiguous observations for one (station, sat, freq_slot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub freq_slot: i32,
    pub start: GpsTime,
    pub end: GpsTime,
    pub epoch_count: usize,
    pub import_id: String,
    /// true when any dependency (import version or params) moved on
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detector {
    /// time-difference of carrier phase vs Doppler prediction
    TimeDifference,
    /// geometry-free ionosphere combination L1-L2
    IonosphereCombo,
    /// Melbourne-Wubbena wide-lane
    MelbourneWubbena,
}

impl Detector {
    pub fn as_str(&self) -> &'static str {
        match self {
            Detector::TimeDifference => "TD",
            Detector::IonosphereCombo => "GF",
            Detector::MelbourneWubbena => "MW",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "TD" => Some(Detector::TimeDifference),
            "GF" => Some(Detector::IonosphereCombo),
            "MW" => Some(Detector::MelbourneWubbena),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateKind {
    /// single-satellite cycle slip
    CycleSlip,
    /// whole-receiver clock jump (kept strictly separate from slips)
    ClockJump,
    /// detector could not decide (e.g. missing frequency) - first-class unknown
    Unknown,
}

impl CandidateKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateKind::CycleSlip => "cycle_slip",
            CandidateKind::ClockJump => "clock_jump",
            CandidateKind::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "cycle_slip" => CandidateKind::CycleSlip,
            "clock_jump" => CandidateKind::ClockJump,
            _ => CandidateKind::Unknown,
        }
    }
}

/// A detector candidate. Detection output is global (keyed by content);
/// adjudication status lives in the event layer, per branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub arc_id: String,
    pub epoch: GpsTime,
    pub detector: Detector,
    pub kind: CandidateKind,
    /// proposed integer offsets in cycles; more than one entry means the
    /// evidence is ambiguous and *all* legal candidates are retained
    pub magnitudes: Vec<i32>,
    pub score: f64,
    /// residual that produced this candidate (cycles, detector-specific combo)
    pub residual: f64,
    /// machine-readable rationale / rejection reason / error state
    pub reason: String,
    pub algo_version: String,
    pub param_fp: String,
    /// direct source observation ids
    pub sources: Vec<String>,
    pub import_id: String,
    pub stale: bool,
}

/// Repair offset: the *only* form of repair. Original observations untouched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offset {
    pub id: String,
    pub arc_id: String,
    /// applied to epochs >= from
    pub from: GpsTime,
    pub cycles: i32,
    pub decision_event_seq: i64,
}

/// Event kinds in the machine-readable judgment log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    Import { import_id: String, label: String, parent: Option<String>, diagnostics: Vec<String> },
    ParamsRevised { old_fp: String, new_fp: String, params: Vec<(String, String)> },
    DetectorRun { import_id: String, param_fp: String, candidate_ids: Vec<String>, invalidated: Vec<String> },
    CandidateAccepted { candidate_id: String, chosen_cycles: i32, rationale: String },
    CandidateRejected { candidate_id: String, reason: String },
    ClockJumpSeparated { candidate_id: String, note: String },
    BranchForked { branch_id: String, name: String, fork_seq: i64 },
    Undo { target_seq: i64, note: String },
    ConflictRecorded { candidate_id: String, branches: Vec<String>, note: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub branch_id: String,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub fork_seq: i64,
}

/// Detection parameters (versioned; fingerprinted into every derived value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    /// max epoch gap inside an arc, seconds
    pub max_gap_s: f64,
    /// TD residual threshold, cycles
    pub td_threshold: f64,
    /// GF combination jump threshold, cycles
    pub gf_threshold: f64,
    /// MW wide-lane jump threshold, cycles
    pub mw_threshold: f64,
    /// >= this many sats with equal simultaneous TD residual => clock jump
    pub clock_min_sats: usize,
    /// tolerance for "equal" residuals across sats, cycles
    pub clock_equal_tol: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 60.0,
            td_threshold: 0.5,
            gf_threshold: 0.5,
            mw_threshold: 0.8,
            clock_min_sats: 3,
            clock_equal_tol: 0.25,
        }
    }
}

impl Params {
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        vec![
            ("max_gap_s".into(), fmt_num(self.max_gap_s)),
            ("td_threshold".into(), fmt_num(self.td_threshold)),
            ("gf_threshold".into(), fmt_num(self.gf_threshold)),
            ("mw_threshold".into(), fmt_num(self.mw_threshold)),
            ("clock_min_sats".into(), self.clock_min_sats.to_string()),
            ("clock_equal_tol".into(), fmt_num(self.clock_equal_tol)),
        ]
    }
    pub fn fingerprint(&self) -> String {
        crate::ids::param_fingerprint(&self.to_pairs())
    }
}

pub fn fmt_num(x: f64) -> String {
    let s = format!("{:.6}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-0" { "0".into() } else { s }
}

/// Unit implied by an observation code's first letter.
pub fn unit_of(obs_code: &str) -> &'static str {
    match obs_code.chars().next() {
        Some('C') => "m",
        Some('L') => "cyc",
        Some('D') => "Hz",
        Some('S') => "dBHz",
        _ => "unknown",
    }
}

/// Frequency slot from the second character of an obs code.
pub fn freq_slot_of(obs_code: &str) -> i32 {
    obs_code.chars().nth(1).and_then(|c| c.to_digit(10)).map(|d| d as i32).unwrap_or(0)
}
