use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A value exactly as typed in the source.  Raw text is always retained; when
/// it is a valid number we also carry the parsed number plus its physical unit
/// or coordinate datum.  Values that cannot be normalized stay in place with a
/// diagnostic instead of being dropped for plotting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measure {
    pub raw: String,
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}

/// Epoch timestamp.  The raw representation is preserved; derived code uses
/// GPS week / seconds-of-week with all differences stated in seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochTime {
    pub raw: String,
    pub gpst_week: i64,
    pub gpst_sow: f64,
    pub datum: String,
}

/// One frequency of one satellite at one epoch: pseudorange, carrier phase,
/// Doppler, LLI flag and SNR stored as imported (raw representation kept).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub site: String,
    pub sat: String,
    pub frequency_no: i64,
    pub obs_code: String,
    pub pseudorange: Measure,
    pub carrier: Measure,
    pub doppler: Measure,
    pub lli: Option<i64>,
    pub snr: Measure,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    pub seq: i64,
    pub time: EpochTime,
    pub receiver_clock_offset: Measure,
    pub observations: Vec<RawObservation>,
}

/// Receiver clock jump as stated in the source log.  Clock events are imported
/// evidence; the engine must keep them separate from single-satellite slips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockJumpEvent {
    pub time: EpochTime,
    pub magnitude: Measure,
    pub kind: String,
    pub note: String,
}

/// Satellite state as imported.  Coordinates carry a datum; unparseable values
/// are retained as measures with diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteState {
    pub sat: String,
    pub time: EpochTime,
    pub system: String,
    pub x: Measure,
    pub y: Measure,
    pub z: Measure,
    pub vx: Measure,
    pub vy: Measure,
    pub vz: Measure,
    pub datum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyMeta {
    pub system: String,
    pub obs_code: String,
    pub frequency_no: i64,
    pub frequency_hz: f64,
    pub wavelength_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteMeta {
    pub site: String,
    pub marker_name: String,
    pub receiver: String,
    pub antenna: String,
}

/// Full input bundle for one manifest version.  All domain quantities carry
/// units/datum at the raw layer; nothing is silently converted away.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputBundle {
    pub schema: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_manifest_id: Option<String>,
    pub site: SiteMeta,
    pub frequencies: Vec<FrequencyMeta>,
    pub epochs: Vec<RawEpoch>,
    pub clock_jumps: Vec<ClockJumpEvent>,
    pub satellite_states: Vec<SatelliteState>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// One item replaced by a revised manifest (only direct sources may change).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedItem {
    pub kind: String, // "epoch" | "clock_jump" | "satellite_state" | "frequency"
    pub key: String,  // epoch seq, clock jump raw time, sat@time ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    pub after: Value,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseBundle {
    pub schema: String,
    pub label: String,
    pub changes: Vec<ChangedItem>,
    pub notes: Vec<String>,
}

/// Tunable detector parameters.  Fingerprinted into every derived value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ruleset {
    pub name: String,
    pub gf_residual_cycles: f64,
    pub mw_residual_wl_cycles: f64,
    pub td_residual_cycles: f64,
    pub td_max_gap_seconds: f64,
    pub clock_min_common_sats: i64,
    pub clock_jump_meters_tolerance: f64,
    pub mw_code_noise_m: f64,
    pub iono_drift_per_second_cycles: f64,
    pub search_radius_cycles: i64,
}

impl Default for Ruleset {
    fn default() -> Self {
        Ruleset {
            name: "default v1".into(),
            gf_residual_cycles: 0.25,
            mw_residual_wl_cycles: 1.0,
            td_residual_cycles: 0.15,
            td_max_gap_seconds: 120.0,
            clock_min_common_sats: 2,
            clock_jump_meters_tolerance: 3.0,
            mw_code_noise_m: 2.5,
            iono_drift_per_second_cycles: 0.0005,
            search_radius_cycles: 30,
        }
    }
}

pub const ALGORITHM_VERSION: &str = "engine-v1.0";
pub const UNIT_RANGE: &str = "m";
pub const UNIT_PHASE: &str = "cycle";
pub const UNIT_DOPPLER: &str = "Hz";
pub const UNIT_SNR: &str = "dB-Hz";
pub const UNIT_SOW: &str = "s";
pub const UNIT_FREQ: &str = "Hz";
pub const UNIT_TIME_DIFF: &str = "s";
pub const COORD_DATUM: &str = "ITRF (ECEF, m)";
