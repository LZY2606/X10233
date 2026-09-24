//! Typed domain model. Raw tokens always survive parsing (see
//! [`crate::parser`]); this layer holds the typed view plus the provenance.
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasKind {
    Code,
    Phase,
    Doppler,
    Cnr,
    Lli,
}

impl MeasKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MeasKind::Code => "code",
            MeasKind::Phase => "phase",
            MeasKind::Doppler => "doppler",
            MeasKind::Cnr => "cnr",
            MeasKind::Lli => "lli",
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            MeasKind::Code => "meter",
            MeasKind::Phase => "cycle",
            MeasKind::Doppler => "hertz",
            MeasKind::Cnr => "dB-Hz",
            MeasKind::Lli => "flag",
        }
    }
    pub fn from_code(code: &str) -> Option<MeasKind> {
        let first = code.chars().next()?;
        Some(match first {
            'C' | 'P' => MeasKind::Code,
            'L' => MeasKind::Phase,
            'D' => MeasKind::Doppler,
            'S' => MeasKind::Cnr,
            _ => return None,
        })
    }
}

/// A single observation value exactly as it appeared in the source line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurement {
    /// observation code, e.g. C1C / L2W; stored as-is from the source
    pub obs_code: String,
    pub kind: MeasKind,
    /// e.g. "L1" derived from the code band char; retained alongside raw code
    pub band: String,
    /// typed value if conversion succeeded (LLI is an integer flag)
    pub value_f64: Option<f64>,
    pub value_int: Option<i64>,
    /// true when token was MISSING / blank and could not be converted
    pub missing: bool,
    /// original token text ("46.1", "MISSING", "N/A")
    pub raw: String,
    pub unit: String,
    /// diagnostic attached when the token could not be converted; the value
    /// stays in place instead of being dropped for plotting.
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrequencyRecord {
    pub satellite: String,
    /// raw frequency labels -> raw numbers as written, e.g. L1=1575420000
    pub entries: Vec<(String, String)>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClockEvent {
    /// parsed jump in microseconds when available
    pub jump_us: Option<f64>,
    pub source: String,
    pub raw: String,
    pub unit: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SatState {
    pub satellite: String,
    /// raw key=value pairs from SATSTATE lines
    pub entries: Vec<(String, String)>,
    /// numeric helper view (values still also exist raw in `entries`)
    pub iono_l1_m: Option<f64>,
    pub clk_bias_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SatObs {
    pub satellite: String,
    pub measurements: Vec<Measurement>,
    pub lli_flags: Vec<(String, i64)>,
}

impl SatObs {
    pub fn find(&self, obs_code: &str) -> Option<&Measurement> {
        self.measurements.iter().find(|m| m.obs_code == obs_code)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EpochRecord {
    pub seq: i64,
    pub time_raw: String,
    pub time_s: Option<f64>,
    pub time_unit: String,
    pub time_diagnostic: Option<String>,
    pub sats: Vec<SatObs>,
    pub frequencies: Vec<FrequencyRecord>,
    pub clock_events: Vec<ClockEvent>,
    pub sat_states: Vec<SatState>,
}

impl EpochRecord {
    pub fn sat(&self, sat: &str) -> Option<&SatObs> {
        self.sats.iter().find(|s| s.satellite == sat)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StationMeta {
    pub id: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub height: Option<f64>,
    pub datum: String,
    pub receiver: String,
    pub antenna: String,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Units {
    pub code: String,
    pub phase: String,
    pub doppler: String,
    pub cnr: String,
    pub jump: String,
    pub range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedFile {
    pub station: StationMeta,
    pub units: Units,
    pub wavelengths: Vec<(String, f64)>,
    pub time_scale: String,
    pub constellation: String,
    pub epochs: Vec<EpochRecord>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub line_no: usize,
    pub severity: String, // error | warning
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

/// Params controlling detection. Serialized canonically to produce a
/// parameter fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub algo_version: String,
    pub gf_sigma_m: f64,
    pub mw_sigma_m: f64,
    pub td_sigma_cycles: f64,
    pub mw_envelope_m: f64,
    pub clock_sigma_m: f64,
    pub gap_factor: f64,
    pub max_lattice: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            algo_version: "csbench.core/1.0.0".to_string(),
            gf_sigma_m: 0.12,
            mw_sigma_m: 0.12,
            td_sigma_cycles: 0.5,
            mw_envelope_m: 0.25,
            clock_sigma_m: 500.0,
            gap_factor: 5.0,
            max_lattice: 3,
        }
    }
}

/// Parse an ISO8601 timestamp of the form 2026-01-04T00:01:45 into seconds
/// relative to the first epoch at compute time (the engine anchors t0).
pub fn iso_to_seconds(raw: &str) -> Option<f64> {
    let bytes = raw.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let n = |a: usize, b: usize| -> Option<i64> { raw.get(a..b)?.parse().ok() };
    let year: i64 = n(0, 4)?;
    let mon: i64 = n(5, 7)?;
    let day: i64 = n(8, 10)?;
    let hour: i64 = n(11, 13)?;
    let min: i64 = n(14, 16)?;
    let sec: i64 = n(17, 19)?;
    let leap = is_leap(year);
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..mon {
        days += mdays[(m - 1) as usize] as i64;
    }
    days += day - 1;
    Some((days * 86400 + hour * 3600 + min * 60 + sec) as f64)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
