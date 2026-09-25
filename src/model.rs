//! Domain model: signals, epochs, observations, clock events and candidates.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// GNSS constellation tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Constellation {
    G,
    R,
    E,
    C,
}

impl Constellation {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "G" => Some(Constellation::G),
            "R" => Some(Constellation::R),
            "E" => Some(Constellation::E),
            "C" | "BDS" => Some(Constellation::C),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Constellation::G => "G",
            Constellation::R => "R",
            Constellation::E => "E",
            Constellation::C => "C",
        }
    }
}

/// Frequency band. For GLONASS (`Constellation::R`) `channel` carries the
/// frequency number k (f = 1602 + k*9/16 MHz); it is stored verbatim for every
/// other constellation as well, where it is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Band {
    pub constellation: Constellation,
    /// RINEX band character, e.g. '1' for GPS L1, '2' for L2, '5' for L5.
    pub band: char,
    /// GLONASS frequency number (0 for CDMA constellations).
    pub channel: i32,
}

impl Band {
    /// Nominal carrier frequency in MHz.
    pub fn frequency_mhz(self) -> f64 {
        match self.constellation {
            Constellation::R => {
                // GLONASS FDMA: f_k = 1602 + k*9/16
                let base = match self.band {
                    '1' => 1602.0,
                    '2' => 1246.0,
                    _ => 1602.0,
                };
                let step = match self.band {
                    '1' => 9.0 / 16.0,
                    '2' => 7.0 / 16.0,
                    _ => 9.0 / 16.0,
                };
                base + step * self.channel as f64
            }
            Constellation::G => match self.band {
                '1' => 1575.42,
                '2' => 1227.60,
                '5' => 1176.45,
                _ => 1575.42,
            },
            Constellation::E => match self.band {
                '1' => 1575.42,
                '5' => 1176.45,
                '7' => 1207.14,
                '8' => 1191.795,
                '6' => 1278.75,
                _ => 1575.42,
            },
            Constellation::C => match self.band {
                '1' => 1561.098,
                '2' => 1207.140,
                '5' => 1176.45,
                '6' => 1268.52,
                '7' => 1207.14,
                '8' => 1191.795,
                _ => 1561.098,
            },
        }
    }

    /// Wavelength in metres per cycle.
    pub fn wavelength_m(self) -> f64 {
        crate::units::C_LIGHT_MPS / (self.frequency_mhz() * 1e6)
    }

    pub fn code(self) -> String {
        format!("{}{}", self.constellation.as_str(), self.band)
    }
}

/// A tracked signal: band + observation code, e.g. band G1 with code `1C`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Signal {
    pub band: Band,
    /// Full RINEX observation code as imported, e.g. "1C", "2W", "5X".
    pub obs_code: [char; 3],
}

impl Signal {
    pub fn band_label(&self) -> String {
        self.band.code()
    }
    pub fn obs_code_str(&self) -> String {
        self.obs_code.iter().collect()
    }
    pub fn key(&self) -> String {
        format!("{}:{}", self.band_label(), self.obs_code_str())
    }
    pub fn parse(constellation: Constellation, code: &str, channel: i32) -> Option<Signal> {
        let chars: Vec<char> = code.chars().collect();
        if chars.len() != 3 {
            return None;
        }
        let band_char = chars[0];
        if !band_char.is_ascii_digit() || band_char == '0' {
            return None;
        }
        Some(Signal {
            band: Band {
                constellation,
                band: band_char,
                channel,
            },
            obs_code: [chars[0], chars[1], chars[2]],
        })
    }
}

/// Satellite identifier such as G07, R11, E19, C30.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SatId {
    pub constellation: Constellation,
    pub prn: u32,
}

impl SatId {
    pub fn key(self) -> String {
        format!("{}{:02}", self.constellation.as_str(), self.prn)
    }
    pub fn parse(s: &str) -> Option<SatId> {
        let mut chars = s.chars();
        let c = Constellation::parse(chars.next()?.to_string().as_str())?;
        let prn: u32 = chars.as_str().parse().ok()?;
        if prn == 0 || prn > 99 {
            return None;
        }
        Some(SatId {
            constellation: c,
            prn,
        })
    }
}

/// Station metadata with coordinate datum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationMeta {
    pub marker_name: String,
    pub marker_number: String,
    pub receiver: String,
    pub antenna: String,
    /// Approximate marker position, metres, ITRF2020 (ECEF X/Y/Z).
    pub approx_position_m: [f64; 3],
}

/// Satellite state imported alongside observations (kept raw; derived layer
/// uses only the position fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatelliteState {
    pub sat: SatId,
    pub position_m: [f64; 3],
    pub clock_bias_s: f64,
    pub datum: String,
}

/// A receiver clock jump declared at the import boundary. RINEX expresses the
/// jump as a millisecond integer and a discontinuity in *all* satellites; it
/// must never be confused with a single-satellite cycle slip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    /// 1-based label of the epoch at which the jump first appears.
    pub at_epoch_label: String,
    pub jump_ms: f64,
    pub source: String,
    pub note: String,
}

/// One observation exactly as found in the imported epoch. `raw` is immutable;
/// `numeric` is `None` when conversion failed, and `diagnostics` says why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub signal: Signal,
    /// Raw token, e.g. "21045126.029", "" (missing) or "not-a-number".
    pub raw: String,
    pub numeric: Option<f64>,
    /// LLI flag characters kept verbatim ("" when absent).
    pub lli: String,
    /// Signal strength field kept verbatim ("" when absent).
    pub ss: String,
    /// Parsed numeric LLI bit flags (bit0=slip, bit3=half-cycle ...).
    pub lli_flags: u8,
    pub diagnostics: Vec<String>,
}

/// Satellite block inside an epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpochSat {
    pub sat: SatId,
    /// Keyed by `Signal::key()`.
    pub observations: BTreeMap<String, Observation>,
}

/// One imported epoch.
#[derive(Debug,Clone, PartialEq, Serialize, Deserialize)]
pub struct Epoch {
    /// 0-based position within the import batch (stable across replay).
    pub index: usize,
    /// Human label copied from input, e.g. "01".
    pub label: String,
    /// Raw timestamp token kept verbatim.
    pub timestamp_raw: String,
    /// GPS time of week in seconds (virtual clock; deterministic).
    pub tow_s: f64,
    /// Epoch interval flag: true when the gap to the previous epoch differs
    /// from the nominal sampling interval.
    pub irregular: bool,
    pub sats: Vec<EpochSat>,
}

/// A complete import payload. `source_version_id` is filled in by the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportPayload {
    pub case_id: String,
    pub station: StationMeta,
    pub nominal_interval_s: f64,
    /// Optional map "R": -7 giving GLONASS frequency numbers.
    #[serde(default)]
    pub glonass_channels: BTreeMap<String, i32>,
    pub epochs: Vec<EpochInput>,
    #[serde(default)]
    pub clock_events: Vec<ClockEvent>,
    #[serde(default)]
    pub satellite_states: Vec<SatelliteState>,
}

/// Loose epoch representation accepted at the import boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EpochInput {
    pub label: String,
    pub timestamp_raw: String,
    pub tow_s: f64,
    #[serde(default)]
    pub irregular: bool,
    #[serde(default)]
    pub sats: Vec<EpochSatInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EpochSatInput {
    pub sat: String,
    /// Each entry is [obsCode (e.g. "1C"), value token, LLI token, SS token].
    #[serde(default)]
    pub obs: Vec<[String; 4]>,
}

/// Detector families. They can legitimately disagree; none is ground truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Detector {
    /// Geometry-free / ionospheric combination (L1-L2 expressed in metres).
    IonosphereFree,
    /// Melbourne-Wubbena combination against pseudorange wide-lane.
    MelbourneWubbena,
    /// Time-differenced carrier phase vs Doppler prediction.
    TimeDifference,
}

impl Detector {
    pub fn as_str(self) -> &'static str {
        match self {
            Detector::IonosphereFree => "ionosphere_free",
            Detector::MelbourneWubbena => "melbourne_wubbena",
            Detector::TimeDifference => "time_difference",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ionosphere_free" => Some(Detector::IonosphereFree),
            "melbourne_wubbena" => Some(Detector::MelbourneWubbena),
            "time_difference" => Some(Detector::TimeDifference),
            _ => None,
        }
    }
}

/// Per-detector evidence attached to a candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorEvidence {
    pub detector: Detector,
    /// Residual in the combination's natural units (cycles of the wide lane
    /// for MW, metres for IF, seconds-equivalent cycles for TD).
    pub residual: f64,
    pub unit: String,
    pub expected: Option<f64>,
    pub threshold: f64,
    pub fired: bool,
    pub note: String,
}

/// An integer offset hypothesis for one band, in whole cycles.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BandOffset {
    pub band: String,
    pub cycles: i64,
}

/// A cycle-slip candidate produced at an arc boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable id `cand:<hash>`, content-addressed from its hypothesis.
    pub id: String,
    pub sat: String,
    pub epoch_label: String,
    /// Ordered band offsets this repair would apply (offset layer only).
    pub offsets: Vec<BandOffset>,
    pub evidence: Vec<DetectorEvidence>,
    /// 0..=1 score; tied candidates share the identical score by construction.
    pub score: f64,
    pub tied: bool,
    /// `accepted`, `rejected` or `pending`.
    pub disposition: String,
    /// Why the candidate is not uniquely admissible, if so.
    pub ambiguity: Option<String>,
    pub reject_reason: Option<String>,
    /// Algorithm version + parameter fingerprint under which it was made.
    pub algo_version: String,
    pub param_fingerprint: String,
    /// Direct source ids (event/arc ids) feeding this candidate.
    pub sources: Vec<String>,
}

/// A continuous carrier arc for one station/satellite/band.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub start_epoch_label: String,
    pub end_epoch_label: String,
    pub reason: String,
}
