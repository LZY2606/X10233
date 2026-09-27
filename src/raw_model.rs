//! Raw observation layer.
//!
//! Everything here is preserved "as the epoch had it": pseudorange, carrier
//! phase, Doppler, LLI, signal-to-noise, receiver clock events, frequency
//! numbers and observation codes. Raw inputs are immutable once committed;
//! later corrections are expressed as new versions with a parent relationship.

use serde::{Deserialize, Serialize};

use crate::units::Qty;

/// A station (receiver) identifier, e.g. "STA1".
pub type StationId = String;
/// A satellite identifier, e.g. "G07".
pub type SatId = String;

/// Signal frequency band number. GNSS convention: e.g. 1 = L1, 2 = L2, 5 = L5.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FreqNum(pub u8);

/// Observation code (RINEX-style three-char code), e.g. "C1C", "L1W", "D5X", "S2W".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ObsCode(pub String);

/// One raw signal observation exactly as it appeared in the epoch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignalObs {
    /// Observation code, e.g. "C1C" (pseudorange), "L1W" (phase), "D1W" (Doppler), "S1W" (CN0).
    pub code: ObsCode,
    pub value: Qty,
    /// Loss-of-lock indicator raw flag (RINEX LLI bits), 0 if absent.
    pub lli: u8,
    /// Raw signal strength indicator, if present separately.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strength_raw: Option<String>,
}

/// The four canonical observation kinds we recognise from observation codes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ObsKind {
    /// Pseudorange (code starts with 'C' or 'P').
    Pseudorange,
    /// Accumulated carrier phase (code starts with 'L').
    Phase,
    /// Doppler (code starts with 'D').
    Doppler,
    /// Carrier-to-noise / signal strength (code starts with 'S').
    Cn0,
}

impl ObsCode {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Classify the observation kind from the leading letter.
    pub fn kind(&self) -> Option<ObsKind> {
        let first = self.0.chars().next()?;
        Some(match first {
            'C' | 'P' => ObsKind::Pseudorange,
            'L' => ObsKind::Phase,
            'D' => ObsKind::Doppler,
            'S' => ObsKind::Cn0,
            _ => return None,
        })
    }
    /// Frequency band number taken from the second character, e.g. "L1W" -> 1.
    pub fn freq_num(&self) -> Option<FreqNum> {
        let n = self.0.chars().nth(1)?.to_digit(10)?;
        Some(FreqNum(n as u8))
    }
}

/// Satellite state imported alongside observations (position + clock).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SatelliteState {
    pub sat: SatId,
    pub x: Qty,
    pub y: Qty,
    pub z: Qty,
    pub clock_bias: Qty,
}

/// Receiver clock jump event recorded exactly as supplied in the source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEvent {
    pub at_epoch: String,
    /// Raw marker token preserved from the import, e.g. "CLOCK_JUMP +1.0ms".
    pub marker: String,
    /// Declared jump size, if the source provides one (otherwise None).
    pub jump: Qty,
}

/// One imported epoch for one station: raw signals per satellite plus states.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpochRecord {
    pub epoch_id: String,
    /// Original timestamp token, preserved verbatim.
    pub time_raw: String,
    /// Virtual/parsed seconds in a fixed GPST-like timeline.
    pub t_seconds: f64,
    pub station: StationId,
    pub signals: Vec<(SatId, Vec<SignalObs>)>,
    pub clock_events: Vec<ClockEvent>,
    pub sat_states: Vec<SatelliteState>,
}

/// Result of importing a source batch: the accepted records plus per-line diagnostics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImportReport {
    pub accepted_epochs: usize,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportDiagnostic {
    pub line_no: usize,
    pub severity: DiagSeverity,
    pub code: String,
    pub message: String,
    /// The offending raw token is kept; the row is never silently dropped.
    pub raw_line: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagSeverity {
    Error,
    Warning,
}
