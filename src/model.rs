//! Domain model: raw observations keep their original representation; the
//! derived layer normalizes. Every domain quantity carries a unit or datum.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;
pub const GPS_L1_HZ: f64 = 1575.42e6;
pub const GPS_L2_HZ: f64 = 1227.60e6;

/// GLONASS carrier frequency for channel number k (-7..=13).
pub fn glo_frequency(band: &str, k: i32) -> Option<f64> {
    match band {
        "L1" => Some(1602.0e6 + k as f64 * 0.5625e6),
        "L2" => Some(1246.0e6 + k as f64 * 0.4375e6),
        _ => None,
    }
}

/// Carrier frequency for a satellite band. GPS ignores the channel number.
pub fn carrier_frequency(sat: &str, band: &str, freq_number: Option<i32>) -> Option<f64> {
    if sat.starts_with('R') {
        glo_frequency(band, freq_number.unwrap_or(0))
    } else {
        match band {
            "L1" => Some(GPS_L1_HZ),
            "L2" => Some(GPS_L2_HZ),
            _ => None,
        }
    }
}

pub fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Observation code, e.g. "L1C", "C2W", "D1C", "S1C".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObsCode {
    pub raw: String,
    pub field: ObsField,
    pub band: String, // "L1" | "L2"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsField {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
}

impl ObsField {
    pub fn unit(&self) -> &'static str {
        match self {
            ObsField::Pseudorange => "m",
            ObsField::Carrier => "cycles",
            ObsField::Doppler => "Hz",
            ObsField::Snr => "dB-Hz",
        }
    }
}

impl ObsCode {
    pub fn parse(raw: &str) -> Option<ObsCode> {
        let bytes = raw.as_bytes();
        if bytes.len() < 2 {
            return None;
        }
        let field = match bytes[0] {
            b'C' => ObsField::Pseudorange,
            b'L' => ObsField::Carrier,
            b'D' => ObsField::Doppler,
            b'S' => ObsField::Snr,
            _ => return None,
        };
        let band = match bytes[1] {
            b'1' => "L1".to_string(),
            b'2' => "L2".to_string(),
            _ => return None,
        };
        Some(ObsCode { raw: raw.to_string(), field, band })
    }
}

/// Detector / review parameters. A parameter fingerprint identifies the
/// exact rule version that produced a derived value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Params {
    pub max_gap_s: f64,
    pub td_threshold_cycles: f64,
    pub gf_threshold_cycles: f64,
    pub mw_threshold_cycles: f64,
    pub max_slip_search: i32,
    pub clock_jump_threshold_m: f64,
    #[serde(default)]
    pub note: String,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 120.0,
            td_threshold_cycles: 0.6,
            gf_threshold_cycles: 0.5,
            mw_threshold_cycles: 0.5,
            max_slip_search: 2,
            clock_jump_threshold_m: 100.0,
            note: String::new(),
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        let mut p = self.clone();
        p.note.clear();
        short_hash(&serde_json::to_string(&p).unwrap())
    }
}

/// A raw observation line, preserved verbatim plus its normalized value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormObs {
    pub epoch_id: String,
    pub t: f64,
    pub sat: String,
    pub code: String,
    pub field: ObsField,
    pub band: String,
    pub value: Option<f64>, // None => unconvertible, see diagnostics
    pub unit: String,
    pub lli: i64,
    pub raw: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SatState {
    pub epoch_id: String,
    pub sat: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub datum: String, // e.g. "WGS84"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockEventRaw {
    pub epoch_id: String,
    pub t: f64,
    pub ms: f64,
    pub raw: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub scope: String,
    pub message: String,
    pub raw: String,
}

/// Candidate lifecycle. `Unknown` is a first-class state: a detector could
/// not compute a value (e.g. frequency missing), not "no slip".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Pending,
    Accepted,
    Rejected,
    Unknown,
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateStatus::Pending => "pending",
            CandidateStatus::Accepted => "accepted",
            CandidateStatus::Rejected => "rejected",
            CandidateStatus::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "rejected" => Some(Self::Rejected),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}
