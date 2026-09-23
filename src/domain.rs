//! Domain model for the carrier-phase cycle-slip review station.
//!
//! All quantities keep their original textual representation and a unit or
//! coordinate datum. Numeric conversion happens in the derived layer only;
//! values that cannot be converted remain in place with a diagnostic.

use std::collections::BTreeMap;

/// Observation band index (1 based). Frequency metadata lives in [`Signal`].
pub type Band = i64;

/// Field codes used in the small RINEX-like case format.
pub const F_PSEUDORANGE: &str = "C1C"; // code pseudorange, metres
pub const F_CARRIER: &str = "L1C"; // accumulated carrier phase, cycles
pub const F_DOPPLER: &str = "D1C"; // Doppler, Hz
pub const F_SNR: &str = "S1C"; // signal-to-noise ratio, dB-Hz

#[derive(Debug, Clone)]
pub struct Station {
    pub code: String,
    pub name: String,
    pub marker: String,
    /// Receiver antenna phase centre, ECEF metres, WGS84 datum.
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
    pub coord_datum: String,
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub band: Band,
    /// RINEX frequency attribute as imported, e.g. `1`, `2`.
    pub freq_number: String,
    /// Observation codes kept exactly as imported, e.g. `C1C/L1C/D1C/S1C`.
    pub code_p: String,
    pub code_l: String,
    pub code_d: String,
    pub code_s: String,
    /// Carrier frequency, Hz.
    pub frequency_hz: f64,
    pub wavelength_m: f64,
}

/// A scalar imported value: original token preserved, numeric conversion best
/// effort. `value` is `None` when the token could not be converted; the
/// original token plus diagnostic stays in the observation.
#[derive(Debug, Clone)]
pub struct CellV {
    pub original: String,
    pub value: Option<f64>,
    pub unit: String,
    pub diagnostic: Option<String>,
}

impl CellV {
    pub fn numeric(token: &str, value: f64, unit: &str) -> Self {
        Self {
            original: token.to_string(),
            value: Some(value),
            unit: unit.to_string(),
            diagnostic: None,
        }
    }
    pub fn invalid(token: &str, unit: &str, diagnostic: String) -> Self {
        Self {
            original: token.to_string(),
            value: None,
            unit: unit.to_string(),
            diagnostic: Some(diagnostic),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObsField {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
}

impl ObsField {
    pub fn key(self) -> &'static str {
        match self {
            ObsField::Pseudorange => F_PSEUDORANGE,
            ObsField::Carrier => F_CARRIER,
            ObsField::Doppler => F_DOPPLER,
            ObsField::Snr => F_SNR,
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            ObsField::Pseudorange => "m",
            ObsField::Carrier => "cycle",
            ObsField::Doppler => "Hz",
            ObsField::Snr => "dB-Hz",
        }
    }
    pub fn all() -> [ObsField; 4] {
        [
            ObsField::Pseudorange,
            ObsField::Carrier,
            ObsField::Doppler,
            ObsField::Snr,
        ]
    }
}

/// Satellite state at an epoch (broadcast/ephemeris snapshot as imported).
#[derive(Debug, Clone)]
pub struct SatState {
    pub sat: String,
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
    pub coord_datum: String,
    /// Satellite clock bias as imported (seconds).
       pub sat_clock_bias_s: Option<f64>,
    pub health: String,
}

/// Receiver (whole-machine) clock event recorded against an epoch.
#[derive(Debug, Clone)]
pub struct ClockEvent {
    pub kind: String, // `clock_jump`
    pub magnitude_s: Option<f64>,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct SatObs {
    pub sat: String,
    /// Loss-of-lock indicator flags per band, stored exactly as imported.
    pub lli: BTreeMap<Band, i64>,
    /// band -> field -> imported cell (missing key means the band/field is
    /// absent at this epoch — a genuine frequency gap, not a zero).
    pub obs: BTreeMap<Band, BTreeMap<ObsField, CellV>>,
}

#[derive(Debug, Clone)]
pub struct EpochRecord {
    pub order: i64,
    pub id: String,
    /// Epoch time datum label, e.g. `GPS`.
    pub time_datum: String,
    pub gps_seconds: f64,
    pub ymdhms: String,
    pub receiver_clock_offset_s: CellV,
    pub clock_event: Option<ClockEvent>,
    pub sats: BTreeMap<String, SatObs>,
    pub sat_states: BTreeMap<String, SatState>,
}

#[derive(Debug, Clone)]
pub struct CaseBundle {
    pub case_id: String,
    pub title: String,
    pub station: Station,
    pub signals: BTreeMap<Band, Signal>,
    pub params: Params,
    pub epochs: Vec<EpochRecord>,
}

/// Detector parameters with explicit fingerprints.
#[derive(Debug, Clone)]
pub struct Params {
    pub gf_cycle_threshold: f64,
    pub gf_sigma_m: f64,
    pub mw_sigma_cycle: f64,
    pub td_sigma_cycle: f64,
    pub search_radius_cycle: i64,
    pub gap_factor: f64,
    pub lli_flag: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            gf_cycle_threshold: 0.15,
            gf_sigma_m: 0.02,
            mw_sigma_cycle: 0.25,
            td_sigma_cycle: 0.75,
            search_radius_cycle: 6,
            gap_factor: 1.5,
            lli_flag: 1,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        let s = format!(
            "gfct={}|gfs={}|mws={}|tds={}|rad={}|gap={}|lli={}",
            self.gf_cycle_threshold,
            self.gf_sigma_m,
            self.mw_sigma_cycle,
            self.td_sigma_cycle,
            self.search_radius_cycle,
            self.gap_factor,
            self.lli_flag
        );
        crate::store::fingerprint(s.as_bytes())
    }
}

/// Kind of a review candidate as classified by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// Single-satellite cycle-slip hypothesis set.
    Slip,
    /// Common-mode whole-machine clock jump; must be kept separate from slips.
    ClockJump,
}

impl CandidateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateKind::Slip => "slip",
            CandidateKind::ClockJump => "clock_jump",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "clock_jump" => CandidateKind::ClockJump,
            _ => CandidateKind::Slip,
        }
    }
}

/// Why an integer hypothesis was rejected; first-class data.
#[derive(Debug, Clone)]
pub struct Rejection {
    pub detector: String,
    pub reason: String,
}

/// Detector evidence for a candidate.
#[derive(Debug, Clone)]
pub struct DetectorEvidence {
    pub detector: String, // `gf`, `mw`, `td`
    pub residual: Option<f64>,
    pub unit: String,
    pub predicted: Option<f64>,
    pub observed: Option<f64>,
    pub threshold: f64,
    pub status: String, // `ok`, `alarm`, `unavailable`
    pub note: String,
}
