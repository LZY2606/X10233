use serde::{Deserialize, Serialize};

use crate::units::Quantity;

pub const KIND_CODE: &str = "code";
pub const KIND_PHASE: &str = "phase";
pub const KIND_DOPPLER: &str = "doppler";
pub const KIND_SNR: &str = "snr";

/// RINEX observation type code, kept verbatim from the header (`C1C`, `L1C`,
/// `D1C`, `S1C`, ...). `band` is the RINEX frequency attribute character.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObsCode {
    pub kind: String, // C | L | D | S
    pub band: String, // "1", "2", ...
    pub attribute: String,
    pub raw: String,
}

impl ObsCode {
    pub fn parse(raw: &str) -> Option<Self> {
        let chars: Vec<char> = raw.chars().collect();
        if chars.len() < 3 || !matches!(chars[0], 'C' | 'L' | 'D' | 'S') {
            return None;
        }
        Some(Self {
            kind: chars[0].to_string(),
            band: chars[1].to_string(),
            attribute: chars[2].to_string(),
            raw: raw.to_string(),
        })
    }
    pub fn kind_name(&self) -> &'static str {
        match self.kind.as_str() {
            "C" => KIND_CODE,
            "L" => KIND_PHASE,
            "D" => KIND_DOPPLER,
            "S" => KIND_SNR,
            _ => "unknown",
        }
    }
}

/// One RINEX observation field for one satellite at one epoch.
/// The textual representation is retained exactly as imported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObsSlot {
    pub code: ObsCode,
    pub quantity: Quantity,
    pub lli: Option<String>,
    pub signal_strength: Option<String>,
    /// `true` for a gap placeholder (all blank in RINEX).
    pub missing: bool,
    pub column: usize,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SatId {
    pub system: String, // G/R/E/C/J/I
    pub prn: u32,
}

impl SatId {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let mut c = raw.chars();
        let s = c.next()?;
        if !s.is_ascii_alphabetic() {
            return None;
        }
        let n: String = c.collect();
        let prn: u32 = n.trim().parse().ok()?;
        Some(Self { system: s.to_ascii_uppercase().to_string(), prn })
    }
    pub fn raw(&self) -> String {
        format!("{}{:02}", self.system, self.prn)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpochRecord {
    pub sat: SatId,
    pub slots: Vec<ObsSlot>,
    /// Receiver clock offset line suffix (seconds), as printed by RINEX.
    pub receiver_clock_offset_s: Option<String>,
}

/// A raw epoch, exactly as the RINEX lines described it. Nothing is
/// normalized here: epochs may be irregular, satellites/band may be missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEpoch {
    pub epoch_index: u64,
    /// 1-based source line(s) range "a-b".
    pub location: String,
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: f64,
    pub epoch_flag: String,
    pub records: Vec<EpochRecord>,
    pub receiver_clock_offset_s: Option<String>,
}

impl RawEpoch {
    /// Continuous seconds since 2000-01-01 (provisional, UTC calendar scale).
    pub fn gps_time_s(&self) -> f64 {
        let days = days_from_civil(self.year as i64, self.month as u64, self.day as u64);
        days as f64 * 86_400.0
            + self.hour as f64 * 3600.0
            + self.minute as f64 * 60.0
            + self.second
    }
}

fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i64;
    era * 146_097 + doe - days_from_civil_anchor()
}
fn days_from_civil_anchor() -> i64 {
    let era = 0;
    let yoe = 2000_i64;
    let doy = 0_i64; // Jan 1
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub location: String,
    pub code: String,
    pub message: String,
    pub raw: String,
    pub severity: String, // error | warning
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedObsFile {
    pub marker_name: String,
    pub receiver: String,
    pub antenna: String,
    pub header_raw: Vec<String>,
    pub obs_codes: Vec<ObsCode>,
    pub epochs: Vec<RawEpoch>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

/// Satellite state / frequency table row. Frequencies are in Hz, wavelengths
/// derived in the derived layer; raw strings are preserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatStateRow {
    pub sat: SatId,
    pub band: String,
    pub code: String,
    pub frequency_hz: Quantity,
    pub wavelength_m: Option<Quantity>,
    pub note: Option<String>,
    pub location: String,
}

/// Externally declared receiver clock jump (whole-machine clock event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    pub event_index: u64,
    pub location: String,
    /// Epoch index the jump first shows up at (0-based, in obs file order).
    pub epoch_index: u64,
    pub jump_ns: Quantity,
    pub kind: String, // declared | inferred
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceBundle {
    pub obs: ParsedObsFile,
    pub sat_states: Vec<SatStateRow>,
    pub clock_events: Vec<ClockEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorParams {
    pub iono_tol_m: f64,
    pub mw_tol_m: f64,
    pub td_tol_cycle: f64,
    pub clock_jump_min_ns: f64,
    pub irregular_gap_factor: f64,
    pub candidate_search_cycles: i64,
    pub max_equal_candidates: usize,
    pub enabled_detectors: Vec<String>,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            iono_tol_m: 0.05,
            mw_tol_m: 0.05,
            td_tol_cycle: 0.10,
            clock_jump_min_ns: 50.0,
            irregular_gap_factor: 1.7,
            candidate_search_cycles: 5,
            max_equal_candidates: 12,
            enabled_detectors: vec![
                "iono-gf".into(),
                "melbourne-wubbena".into(),
                "time-difference".into(),
            ],
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        crate::util::hid("pfp:", &serde_json::to_string(self).unwrap_or_default())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DetectorProposal {
    pub detector: String,
    pub status: String, // candidate | none | error | unknown
    pub residual: Option<f64>,
    pub residual_unit: Option<String>,
    pub tolerance: Option<f64>,
    pub delta_seconds: Option<f64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffsetPair {
    pub band: String,
    pub cycles: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub sat: SatId,
    pub epoch_index: u64,
    pub location: String,
    pub offsets: Vec<OffsetPair>,
    pub score: i64,
    pub ambiguous: bool,
    pub tie_group_size: usize,
    pub lli_evidence: bool,
    pub clock_event_id: Option<String>,
    pub status: String, // candidate | rejected-clock | error | unknown
    pub proposals: Vec<DetectorProposal>,
    pub reject_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReviewPayload {
    pub decision: String, // accept | reject | adjust-params | mark-unknown
    pub comment: String,
    pub supersedes_event_id: Option<String>,
    pub reviewer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamRevision {
    pub old_fingerprint: Option<String>,
    pub new_params: DetectorParams,
    pub reason: String,
    pub reviewer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRevision {
    pub parent_source_id: String,
    /// Raw text of the replacement obs file. The engine rebuilds and diffs
    /// slots; untouched slots keep their identities.
    pub obs_text: String,
    pub clock_events_text: Option<String>,
    pub sat_states_text: Option<String>,
    pub reason: String,
    pub reviewer: String,
}
