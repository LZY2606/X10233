// Domain model for the cycle-slip review station.
//
// Rule of the house: raw input is kept exactly as imported (text, codes, LLI,
// SNR, frequency numbers, clock events). Normalisation happens only in the
// derived layer, and every derived value carries algorithm version, a
// parameter fingerprint and references to its direct sources.

use serde::Serialize;

/// A value that keeps its original textual representation. Parsing into the
/// normalised numeric form happens in the derived layer; a value that cannot be
/// converted stays in place with a diagnostic instead of being dropped.
#[derive(Debug, Clone, Serialize)]
pub struct RawValue {
    pub text: String,
    pub value: Option<f64>,
    pub unit: &'static str,
    pub diagnostic: Option<String>,
}

impl RawValue {
    pub fn parse(text: String, unit: &'static str) -> Self {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return RawValue { text, value: None, unit, diagnostic: Some("missing/blank".into()) };
        }
        match trimmed.parse::<f64>() {
            Ok(v) => RawValue { text, value: Some(v), unit, diagnostic: None },
            Err(_) => RawValue {
                text,
                value: None,
                unit,
                diagnostic: Some(format!("not convertible to {unit}: {trimmed}")),
            },
        }
    }
}

/// One RINEX epoch record, as written in the observation file.
#[derive(Debug, Clone)]
pub struct RawEpoch {
    pub epoch_id: String,
    pub order_index: i64,
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second_text: String,
    pub epoch_flag: i64,
    pub clock_offset_text: Option<String>,
    pub sat_ids: Vec<String>,
    pub raw_line: String,
    /// GPS seconds of week (derived, carries unit "s" in API payloads).
    pub gps_seconds: f64,
    pub time_text: String,
}

/// One observation item (a single code at one epoch/satellite), raw.
#[derive(Debug, Clone)]
pub struct RawObservation {
    pub epoch_id: String,
    pub order_index: i64,
    pub sat: String,
    /// RINEX observation code exactly as imported, e.g. "C1", "L1", "D1", "S1".
    pub obs_code: String,
    pub value_text: String,
    pub lli: Option<u8>,
    pub snr: Option<u8>,
}

/// Satellite state line (position/clock/health), raw with a coordinate datum.
#[derive(Debug, Clone)]
pub struct RawSatState {
    pub epoch_id: String,
    pub order_index: i64,
    pub sat: String,
    pub x: RawValue,
    pub y: RawValue,
    pub z: RawValue,
    pub clock_bias: RawValue,
    pub datum: String,
    pub health: Option<u8>,
}

/// Receiver-level event such as a whole-receiver clock jump, raw.
#[derive(Debug, Clone)]
pub struct RawReceiverEvent {
    pub epoch_id: String,
    pub order_index: i64,
    pub kind: String,
    pub amount_text: Option<String>,
    pub raw_line: String,
}

/// Result of importing one revision's file bundle.
pub struct ImportBundle {
    pub header: RinexHeader,
    pub epochs: Vec<RawEpoch>,
    pub observations: Vec<RawObservation>,
    pub sat_states: Vec<RawSatState>,
    pub receiver_events: Vec<RawReceiverEvent>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RinexHeader {
    pub marker_name: Option<String>,
    pub marker_number: Option<String>,
    pub receiver: Option<String>,
    pub antenna: Option<String>,
    pub approx_x: Option<String>,
    pub approx_y: Option<String>,
    pub approx_z: Option<String>,
    pub position_datum: Option<String>,
    pub obs_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParseDiagnostic {
    pub severity: String,
    pub code: String,
    pub line_no: Option<usize>,
    pub message: String,
}
