//! Domain model and quantity conventions.
//!
//! Every displayed domain value carries a stable identity and a unit / datum.
//! Raw import representations are preserved verbatim; only the derived layer
//! normalizes units. Values that cannot be converted stay in place with a
//! diagnostic instead of being dropped for plotting.

use serde::{Deserialize, Serialize};

pub const L1_HZ: f64 = 1.57542e9;
pub const L2_HZ: f64 = 1.22760e9;
pub const C_LIGHT_M_S: f64 = 299_792_458.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Band {
    L1,
    L2,
}
impl Band {
    pub fn freq_hz(self) -> f64 {
        match self {
            Band::L1 => L1_HZ,
            Band::L2 => L2_HZ,
        }
    }
    pub fn wavelength_m(self) -> f64 {
        C_LIGHT_M_S / self.freq_hz()
    }
    pub fn code(self) -> &'static str {
        match self {
            Band::L1 => "L1",
            Band::L2 => "L2",
        }
    }
}

/// Parse RINEX-3-style observation code (e.g. `L1C`, `C2W`).
/// Frequency number is the band character; tracking attribute is preserved but
/// combinations here use the GPS L1/L2 bands.
pub fn parse_band(code: &str) -> Result<Band, String> {
    let bytes = code.as_bytes();
    if bytes.len() < 2 {
        return Err(format!("observation code too short: {code:?}"));
    }
    match bytes[1] {
        b'1' => Ok(Band::L1),
        b'2' => Ok(Band::L2),
        other => Err(format!(
            "frequency number {other} ({code:?}) is outside this case pack (GPS L1/L2 only)"
        )),
    }
}

/// A quantity carrying both its original textual representation and the
/// normalized value used by the derived layer.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Quantity {
    /// Original token exactly as supplied at import.
    pub raw: String,
    /// Normalized numeric value, or null when conversion failed.
    pub value: Option<f64>,
    /// Unit of `value` (raw text is kept untouched regardless).
    pub unit: String,
    /// Diagnostics attached when `value` is null; empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}
impl Quantity {
    pub fn raw_only(raw: &str, unit: &str, err: String) -> Self {
        Quantity {
            raw: raw.to_string(),
            value: None,
            unit: unit.to_string(),
            diagnostics: vec![err],
        }
    }
    pub fn ok(raw: &str, value: f64, unit: &str) -> Self {
        Quantity {
            raw: raw.to_string(),
            value: Some(value),
            unit: unit.to_string(),
            diagnostics: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Full RINEX observation code as imported, e.g. `L1C`.
    pub code: String,
    /// RINEX frequency number (band character), e.g. `1`, preserved separately.
    pub frequency_number: String,
    pub band: Option<String>,
    pub pseudorange_m: Quantity,
    pub carrier_cycles: Quantity,
    pub doppler_hz: Quantity,
    /// RINEX LLI flag as the original single character (or empty).
    pub lli: String,
    /// RINEX SSI/signal strength character (or empty).
    pub ssi: String,
    pub snr_db_hz: Quantity,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SatObs {
    pub sat: String,
    pub obs: Vec<Observation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    /// ECEF position components, datum labelled by `datum`.
    pub x_m: Quantity,
    pub y_m: Quantity,
    pub z_m: Quantity,
    pub datum: String,
    /// Satellite clock bias in metres, null/unknown allowed.
    pub clock_bias_m: Quantity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockJumpEvent {
    /// Epoch time label this event is attached to (matches EpochRecord.time).
    pub time: String,
    /// Receiver clock jump applied at this epoch (seconds, positive = jump
    /// forward). Stated by the import material, not inferred.
    pub jump_s: f64,
    pub source: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRecord {
    pub time: String,
    pub flag: String,
    pub sats: Vec<SatObs>,
    #[serde(default)]
    pub sat_states: Vec<SatState>,
    #[serde(default)]
    pub clock_events: Vec<ClockJumpEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPayload {
    pub station: String,
    pub receiver: String,
    pub antenna: String,
    /// Coordinate datum of the station coordinates.
    pub station_datum: String,
    pub station_x_m: f64,
    pub station_y_m: f64,
    pub station_z_m: f64,
    pub epochs: Vec<EpochRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub time: Option<String>,
    pub sat: Option<String>,
    pub obs_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DetectorName {
    IonosphereFreeGeometryFree,
    MelbourneWubbena,
    TimeDifference,
    Lli,
}
impl DetectorName {
    pub fn tag(self) -> &'static str {
        match self {
            DetectorName::IonosphereFreeGeometryFree => "GF",
            DetectorName::MelbourneWubbena => "MW",
            DetectorName::TimeDifference => "TD",
            DetectorName::Lli => "LLI",
        }
    }
    pub fn parse(s: &str) -> Option<DetectorName> {
        match s {
            "GF" => Some(DetectorName::IonosphereFreeGeometryFree),
            "MW" => Some(DetectorName::MelbourneWubbena),
            "TD" => Some(DetectorName::TimeDifference),
            "LLI" => Some(DetectorName::Lli),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CandidateStatus {
    Open,
    Accepted,
    Rejected,
}
impl CandidateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateStatus::Open => "open",
            CandidateStatus::Accepted => "accepted",
            CandidateStatus::Rejected => "rejected",
        }
    }
}

/// Calendar time `YYYY-MM-DDTHH:MM:SS` -> whole seconds since 2000-01-01T00:00.
pub fn parse_time(s: &str) -> Result<i64, String> {
    let bad = || format!("expected YYYY-MM-DDTHH:MM:SS, got {s:?}");
    if s.len() < 19 || s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return Err(bad());
    }
    let year: i64 = s[0..4].parse().map_err(|_| bad())?;
    let mon: i64 = s[5..7].parse().map_err(|_| bad())?;
    let day: i64 = s[8..10].parse().map_err(|_| bad())?;
    let hour: i64 = s[11..13].parse().map_err(|_| bad())?;
    let min: i64 = s[14..16].parse().map_err(|_| bad())?;
    let sec: i64 = s[17..19].parse().map_err(|_| bad())?;
    if !(2000..=2099).contains(&year) || !(1..=12).contains(&mon) || !(1..=31).contains(&day) {
        return Err(bad());
    }
    let days_before = |y: i64, m: i64| -> i64 {
        let leap = |yy: i64| (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0;
        let md = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        365 * (y - 2000) + (y - 1997) / 4 - (y - 1901) / 100 + (y - 1601) / 400
            + md[(m - 1) as usize]
            + if m > 2 && leap(y) { 1 } else { 0 }
    };
    let days = days_before(year, mon) + (day - 1);
    Ok(days * 86_400 + hour * 3_600 + min * 60 + sec)
}
