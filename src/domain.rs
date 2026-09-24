//! Domain model: raw (immutable) layers, events, parameters, derived objects.

use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "cbs-detector-1.0.0";
pub const DEFAULT_SEED: u64 = 20260924;

// ---------------------------------------------------------------------------
// Units / datums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meter,
    Cycle,
    MetersPerSecond,
    Hertz,
    Second,
    Dimensionless,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cycle",
            Unit::MetersPerSecond => "m/s",
            Unit::Hertz => "Hz",
            Unit::Second => "s",
            Unit::Dimensionless => "1",
        }
    }
}

/// Position/time reference frame for satellite states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Datum {
    pub frame: String,
    pub timescale: String,
}

impl Default for Datum {
    fn default() -> Self {
        Self { frame: "ECEF/ITRF-like".into(), timescale: "GPS".into() }
    }
}

// ---------------------------------------------------------------------------
// Raw layer (kept exactly as imported)
// ---------------------------------------------------------------------------

/// A raw scalar: original textual representation plus, when convertible, the
/// normalized f64. Unconvertible values stay in place with a diagnostic and
/// are never dropped for plotting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawValue {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: Unit,
    pub diagnostic: Option<String>,
}

impl RawValue {
    pub fn parsed(raw: String, value: f64, unit: Unit) -> Self {
        Self { raw, value: Some(value), unit, diagnostic: None }
    }

    pub fn blank(unit: Unit) -> Self {
        Self { raw: String::new(), value: None, unit, diagnostic: None }
    }

    pub fn invalid(raw: String, unit: Unit, why: String) -> Self {
        Self { raw, value: None, unit, diagnostic: Some(why) }
    }
}

/// One (code/phase/doppler/SNR) measurement of one satellite-frequency at one
/// epoch, exactly as represented in the RINEX record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMeasurement {
    pub sat: String,
    pub obs_code: String,
    pub freq_no: u8,
    pub range: Option<RawValue>,
    pub phase: Option<RawValue>,
    pub doppler: Option<RawValue>,
    pub lli: Option<u8>,
    pub snr: Option<RawValue>,
}

/// Receiver clock jump reported by the import (e.g. derived from the RINEX
/// epoch header clock offset) - evidence kept verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEvent {
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub kind: String,
    pub jump_seconds_raw: RawValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub epoch_flag: u8,
    pub recv_clk_offset_raw: RawValue,
    pub measurements: Vec<RawMeasurement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateRow {
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub sat: String,
    pub x: RawValue,
    pub y: RawValue,
    pub z: RawValue,
    pub sat_clock_bias: RawValue,
    pub datum: Datum,
}

/// A complete, immutable raw submission. Once committed it is never
/// overwritten; corrections arrive as a new version with a parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBundle {
    pub station: String,
    pub marker: String,
    pub receiver: String,
    pub rinex_version: String,
    pub constellation: String,
    pub receiver_pos: Vec<f64>,
    pub receiver_pos_unit: Unit,
    pub system_types: Vec<String>,
    pub epochs: Vec<RawEpoch>,
    pub clock_events: Vec<RawClockEvent>,
    pub sat_states: Vec<SatStateRow>,
}

// ---------------------------------------------------------------------------
// Events (append-only audit log)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    ProjectCreated,
    RawCommitted,
    ParamsRevised,
    Adjudicated,
    ConflictResolved,
}

impl EventKind {
 pub fn as_str(self) -> &'static str {
        match self {
            EventKind::ProjectCreated => "project_created",
            EventKind::RawCommitted => "raw_committed",
            EventKind::ParamsRevised => "params_revised",
            EventKind::Adjudicated => "adjudicated",
            EventKind::ConflictResolved => "conflict_resolved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "project_created" => EventKind::ProjectCreated,
            "raw_committed" => EventKind::RawCommitted,
            "params_revised" => EventKind::ParamsRevised,
            "adjudicated" => EventKind::Adjudicated,
            "conflict_resolved" => EventKind::ConflictResolved,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub seq: i64,
    pub parent_event_id: Option<String>,
    pub branch: String,
    pub kind: EventKind,
    pub virtual_ms: i64,
    pub actor: String,
    pub summary: String,
    pub payload: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Parameters with fingerprints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub seed: u64,
    /// time-difference phase residual gate (cycles)
    pub td_gate_cycles: f64,
    /// ionosphere (geometry-free) jump gate (metres)
    pub gf_gate_m: f64,
    /// Melbourne-Wubbena widelane jump gate (cycles of widelane)
    pub mw_gate_wl_cycles: f64,
    /// gap-time above which iono extrapolation tolerance widens (seconds)
    pub gap_seconds: f64,
    pub gap_gf_tol_m: f64,
    /// satellites needed to call a common (receiver clock) jump
    pub clock_min_sats: usize,
    /// fraction of sats whose phase step must agree for a clock jump
    pub clock_consensus: f64,
    pub clock_phase_step_cycles: f64,
    /// integer search radius when enumerating candidates
    pub search_radius: i64,
    /// MW window length (epochs)
    pub mw_window: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            td_gate_cycles: 0.25,
            gf_gate_m: 0.08,
            mw_gate_wl_cycles: 0.25,
            gap_seconds: 30.0,
            gap_gf_tol_m: 0.12,
            clock_min_sats: 3,
            clock_consensus: 0.75,
            clock_phase_step_cycles: 50.0,
            search_radius: 6,
            mw_window: 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Derived: boundaries, detector results, candidates, repairs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detector {
    TimeDifference,
    IonosphereFree,
    MelbourneWubbena,
    ClockCommonMode,
    Lli,
}

impl Detector {
    pub fn as_str(self) -> &'static str {
        match self {
            Detector::TimeDifference => "time_difference",
            Detector::IonosphereFree => "ionosphere_gf",
            Detector::MelbourneWubbena => "melbourne_wubbena",
            Detector::ClockCommonMode => "clock_common_mode",
            Detector::Lli => "lli",
        }
    }

    pub fn all() -> [Detector; 5] {
        [
            Detector::TimeDifference,
            Detector::IonosphereFree,
            Detector::MelbourneWubbena,
            Detector::ClockCommonMode,
            Detector::Lli,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Unique,
    Underdetermined,
    Rejected,
}

impl CandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateStatus::Unique => "unique",
            CandidateStatus::Underdetermined => "underdetermined",
            CandidateStatus::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Accepted,
    Rejected,
    Unresolved,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Accepted => "accepted",
            Decision::Rejected => "rejected",
            Decision::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub detector: String,
    /// integer offset per frequency band, e.g. {"1": -3, "2": 0}
    pub offset_cycles: serde_json::Map<String, serde_json::Value>,
    pub residual: f64,
    pub residual_unit: Unit,
    pub score: f64,
    pub tie_rand: f64,
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorResult {
    pub id: String,
    pub detector: String,
    pub at_epoch_no: u32,
    pub residual: f64,
    pub residual_unit: Unit,
    pub fired: bool,
    pub unknown: bool,
    pub reason: String,
    pub candidates: Vec<Candidate>,
}

/// A station-satellite-frequency arc boundary sits between two consecutive
/// observed epochs of that band.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    pub id: String,
    pub sat: String,
    pub band: u8,
    pub from_epoch_no: u32,
    pub to_epoch_no: u32,
    pub dt_seconds: f64,
    pub missing: bool,
    pub lli: bool,
    pub clock_suspect: bool,
    pub results: Vec<DetectorResult>,
    /// adopted integer offset (offset layer only); None = not repaired
    pub repair_offset_cycles: Option<i64>,
    pub decision: String,
    pub decided_by_event: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcInfo {
    pub id: String,
    pub sat: String,
    pub band: u8,
    pub start_epoch_no: u32,
    pub end_epoch_no: u32,
    pub epoch_count: usize,
    pub ended_by: String,
}

// ---------------------------------------------------------------------------
// Rejected reasons / diagnostics are first-class
// ---------------------------------------------------------------------------

pub mod reasons {
    pub const CLOCK_JUMP_SUPPRESSED: &str = "suppressed:common_receiver_clock_jump";
    pub const LLI_FALSE_ALARM: &str = "lli_flag_without_phase_break";
    pub const FREQUENCY_MISSING: &str = "frequency_missing_at_epoch";
    pub const UNEVEN_EPOCHS: &str = "uneven_epoch_spacing";
    pub const MULTIWAY_TIE: &str = "multiple_equal_integer_splits";
    pub const NO_BREAK: &str = "residual_within_gate";
    pub const GAP_IONO_DRIFT: &str = "gap_ionosphere_drift_ambiguous";
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

pub fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn short_hash(s: &str) -> String {
    format!("{:016x}", fnv1a64(s))
}
