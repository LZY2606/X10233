//! Domain model.
//!
//! Every derived value carries:
//! * a stable identity (`Id`) that survives recompute, export/import and fork,
//! * the algorithm version,
//! * a parameter fingerprint,
//! * its direct sources,
//! * a unit/coordinate reference.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 12 hex chars of SHA-256 over a canonical key string. Deterministic across
/// machines, runs and import/export cycles.
pub fn stable_id(kind: &str, key: &str) -> Id {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"\x1f");
    h.update(key.as_bytes());
    let digest = h.finalize();
    Id(hex::encode(&digest[..6]))
}

/// Stable, content-derived identity of a domain object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Id(pub String);

impl Id {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Coordinate/time reference carried by a domain quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reference {
    /// Seconds since GPS epoch 1980-01-06, no leap seconds.
    GpsSeconds,
    /// ISO-8601 wall time as observed in the RINEX header.
    WallIso,
    /// SI metre.
    Metre,
    /// Carrier cycle (unitless count, explicitly named).
    Cycle,
    /// SI metre / second.
    MetrePerSecond,
    /// SI second.
    Second,
    /// dB-Hz (SNR).
    DbHz,
    /// Dimensionless.
    One,
    /// WGS-84 (ECEF, metres).
    Wgs84,
}

impl Reference {
    pub fn unit(self) -> &'static str {
        match self {
            Reference::GpsSeconds | Reference::Second => "s",
            Reference::WallIso => "ISO-8601",
            Reference::Metre => "m",
            Reference::Cycle => "cycle",
            Reference::MetrePerSecond => "m/s",
            Reference::DbHz => "dB-Hz",
            Reference::One => "1",
            Reference::Wgs84 => "WGS-84 ECEF m",
        }
    }
}

/// A value with an explicit reference. The raw layer keeps [`Value::raw`];
/// conversion failures stay at the origin instead of being silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Value {
    /// Original textual representation exactly as imported.
    pub raw: String,
    /// Parsed value in [`Value::reference`] units; `None` when unconvertible.
    pub value: Option<f64>,
    pub reference: Reference,
    /// Diagnostics attached when `value` could not be produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

impl Value {
    pub fn parsed(raw: impl Into<String>, value: f64, reference: Reference) -> Self {
        Value { raw: raw.into(), value: Some(value), reference, diagnostics: vec![] }
    }

    pub fn unparseable(raw: impl Into<String>, reference: Reference, why: impl Into<String>) -> Self {
        Value { raw: raw.into(), value: None, reference, diagnostics: vec![why.into()] }
    }

    pub fn is_ok(&self) -> bool {
        self.value.is_some()
    }
}

pub const ALGO_VERSION: &str = "gsb-cs-algo-1.0.0";

/// RINEX observation type / code, preserved verbatim plus the frequency band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange,
    CarrierPhase,
    Doppler,
    /// RINEX SSI mapped to dB-Hz.
    Snr,
}

/// One imported observation cell for a station / epoch / satellite / code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Stable id: hash of (station, epoch raw, sv, code).
    pub id: Id,
    pub station: String,
    pub epoch_raw: String,
    pub gps_seconds: Option<f64>,
    pub sv: String,
    /// RINEX 3 code, e.g. `C1C`, `L1W`, `D1W`, `S1C`.
    pub code: String,
    pub kind: ObsKind,
    /// Frequency number as RINEX band digit (1,2,5,...).
    pub band: u8,
    /// Frequency in Hz derived from band; `None` for unknown bands.
    pub frequency_hz: Option<f64>,
    /// Loss-of-lock indicator raw digit for this code (0 when none).
    pub lli: u8,
    /// Signal strength indicator raw digit (0..9, 0 = unknown).
    pub ssi: u8,
    pub value: Value,
}

/// Receiver/satellite state for one epoch (az/el/ECEF), units explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub id: Id,
    pub station: String,
    pub epoch_raw: String,
    pub gps_seconds: Option<f64>,
    pub sv: String,
    pub azimuth_deg: Option<f64>,
    pub elevation_deg: Option<f64>,
    /// ECEF x/y/z in WGS-84 metres.
    pub x_m: Option<f64>,
    pub y_m: Option<f64>,
    pub z_m: Option<f64>,
    pub diagnostics: Vec<String>,
}

/// A receiver (whole-instrument) clock event, always separated from slips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockEventType {
    /// Declared in the imported sheet (explicit source of truth).
    Declared,
    /// Detected by the common-mode detector.
    Detected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: Id,
    pub station: String,
    pub epoch_raw: String,
    pub gps_seconds: Option<f64>,
    pub event_type: ClockEventType,
    /// Jump in receiver time, seconds (phase & code common mode).
    pub jump_seconds: Value,
    pub sources: Vec<Id>,
}

/// Tunable detector parameters. Each revision is immutable and fingerprinted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// GF ionosphere-combination residual gate, cycles.
    pub gf_sigma_cycles: f64,
    /// Melbourne-Wubbena residual gate, wide-lane cycles.
    pub mw_sigma_cycles: f64,
    /// Time-difference single-frequency gate, cycles.
    pub td_sigma_cycles: f64,
    /// Common-mode agreement required to call a receiver clock event, cycles.
    pub clock_common_sigma_cycles: f64,
    /// Fraction-of-cycle window considered an integer tie, cycles.
    pub tie_band_cycles: f64,
    /// Epoch gap (s) above which a frequency arc ends.
    pub arc_gap_seconds: f64,
    /// Maximum integer shift searched when proposing repairs.
    pub search_window_cycles: i64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            gf_sigma_cycles: 1.0,
            mw_sigma_cycles: 0.75,
            td_sigma_cycles: 3.0,
            clock_common_sigma_cycles: 0.5,
            tie_band_cycles: 0.2,
            arc_gap_seconds: 60.0,
            search_window_cycles: 20,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let json = serde_json::to_string(self).expect("params serialize");
        stable_id("params", &json).0
    }
}

/// Provenance attached to every derived object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub algo_version: String,
    pub params_fp: String,
    /// Direct source object ids (raw observations / clock events / decisions).
    pub sources: Vec<Id>,
    /// Event that produced this revision.
    pub produced_by_event: Id,
}

/// Why a candidate or branch was rejected. Rejections are first-class.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rejection {
    pub reason_code: String,
    pub detail: String,
    pub sources: Vec<Id>,
}

/// A per-detector proposal. Different detectors may propose different shifts;
/// none of them is treated as ground truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorVote {
    pub detector: String,
    /// Observed residual before rounding (cycles of that combination).
    pub residual: Option<f64>,
    pub residual_unit: String,
    /// Proposed integer shift of (band1, band2) in cycles; None = no proposal.
    pub shift_cycles_b1: Option<i64>,
    pub shift_cycles_b2: Option<i64>,
    /// Score contribution 0..1.
    pub score: f64,
    pub note: String,
}

/// Lifecycle of a cycle-slip candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Proposed,
    Accepted,
    Rejected,
    /// Two valid offsets remain indistinguishable.
    Ambiguous,
    /// Human decisions disagree.
    Conflict,
    /// Cannot form the combination (missing frequency etc.).
    Unknown,
}

/// One legal integer offset hypothesis at an arc boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetHypothesis {
    /// Slip size on band 1 (cycles); `None` when band 1 is unavailable.
    pub shift_b1_cycles: Option<i64>,
    /// Slip size on band 2 (cycles); `None` when band 2 is unavailable.
    pub shift_b2_cycles: Option<i64>,
    pub total_score: f64,
    /// True when the hypothesis shares the top score (equally divided).
    pub tied: bool,
}

/// The central review object: a slip candidate at one station/sv/epoch boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: Id,
    pub station: String,
    pub sv: String,
    pub band1: u8,
    pub band2: u8,
    pub epoch_raw: String,
    pub gps_seconds: Option<f64>,
    pub prev_epoch_raw: String,
    pub arc_id_before: Id,
    pub arc_id_after: Id,
    pub status: CandidateStatus,
    pub votes: Vec<DetectorVote>,
    pub hypotheses: Vec<OffsetHypothesis>,
    /// Raw LLI flags observed at the boundary (`(code, digit)`).
    pub lli_flags: Vec<(String, u8)>,
    /// Rejection/error reasons — kept even when accepted.
    pub rejections: Vec<Rejection>,
    /// Free-form error/unknown diagnostic, if any.
    pub error: Option<String>,
    pub provenance: Provenance,
}

/// A station/sv/band continuous carrier-phase track version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcRecord {
    pub id: Id,
    pub station: String,
    pub sv: String,
    pub band: u8,
    pub code: String,
    pub start_epoch_raw: String,
    pub end_epoch_raw: String,
    pub start_gps: Option<f64>,
    pub end_gps: Option<f64>,
    /// How this boundary started/ended.
    pub boundary_reason: String,
    /// Revision lineage: the arc version this was revised from.
    pub revised_from: Option<Id>,
    pub provenance: Provenance,
}

/// A combination residual row feeding the multi-frequency plot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualPoint {
    pub id: Id,
    pub station: String,
    pub sv: String,
    pub epoch_raw: String,
    pub gps_seconds: Option<f64>,
    pub combination: String,
    /// Residual value in `unit`.
    pub value: Option<f64>,
    pub unit: String,
    pub diagnostics: Vec<String>,
    pub sources: Vec<Id>,
}

/// Why an arc ended / started.
pub mod boundary {
    pub const START: &str = "track-start";
    pub const GAP: &str = "epoch-gap";
    pub const MISSING_FREQ: &str = "frequency-missing";
    pub const SLIP: &str = "cycle-slip";
    pub const CLOCK: &str = "receiver-clock-event";
    pub const END: &str = "track-end";
}

/// Decision outcomes from the review UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionKind {
    Accept,
    Reject,
    /// Parameter adjustment recorded alongside the decision (params revision).
    ParameterRevision,
    ConflictResolved,
    Undo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub kind: DecisionKind,
    pub candidate_id: Option<Id>,
    pub shift_b1_cycles: Option<i64>,
    pub shift_b2_cycles: Option<i64>,
    pub note: String,
    /// Event being undone, for [`DecisionKind::Undo`].
    pub undoes_event: Option<Id>,
}
