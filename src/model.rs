//! Domain model for the GNSS carrier-phase cycle-slip review workbench.
//!
//! Design rules encoded here:
//! * Raw observations are preserved verbatim; normalization happens only in
//!   derived layers. Values that cannot be converted stay in place with a
//!   diagnostic attached.
//! * Candidates, rejection reasons, errors and unknown states are first-class
//!   data. No detector is treated as ground truth.
//! * Repairs live only in an offset layer; raw carrier phases are never
//!   modified.

use serde::{Deserialize, Serialize};

/// Algorithm version stamped on every derived row.
pub const ALGO_VERSION: &str = "slipdetect/1.2.0";

/// Fixed base of the virtual clock. Virtual time is deterministic:
/// `VTIME_BASE_UNIX + event_seq` seconds, so tests never touch wall time.
pub const VTIME_BASE_UNIX: i64 = 1_790_784_000; // 2026-10-01T00:00:00Z

pub fn virtual_time_iso(seq: i64) -> String {
    format_unix_utc(VTIME_BASE_UNIX + seq)
}

pub fn format_unix_utc(t: i64) -> String {
    // Civil-from-days algorithm (Howard Hinnant), no external deps.
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Detection / arc-building parameters. A parameter revision creates a new
/// fingerprint; derived rows keep the fingerprint they were computed with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// Max epoch gap (seconds) inside one arc; larger gaps split the arc.
    pub max_gap_s: f64,
    /// Time-difference residual threshold (cycles).
    pub td_threshold_cycles: f64,
    /// Geometry-free iono combination jump threshold (cycles).
    pub gf_threshold_cycles: f64,
    /// Melbourne-Wubbena wide-lane jump threshold (cycles).
    pub mw_threshold_cycles: f64,
    /// Clock-jump detection: fraction of satellites that must jump together.
    pub clock_jump_ratio: f64,
    /// Half-width (cycles) within which several integer offsets stay legal.
    pub integer_ambiguity_tolerance: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 60.0,
            td_threshold_cycles: 0.6,
            gf_threshold_cycles: 0.8,
            mw_threshold_cycles: 1.0,
            clock_jump_ratio: 0.6,
            integer_ambiguity_tolerance: 0.35,
        }
    }
}

impl Params {
    /// Stable fingerprint: FNV-1a over the canonical JSON encoding.
    pub fn fingerprint(&self) -> String {
        let s = serde_json::to_string(self).unwrap_or_default();
        fnv1a_hex(s.as_bytes())
    }
}

pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", h)
}

/// GPS carrier frequency (Hz) for a frequency number, used only when
/// normalizing into the derived layer. Unknown numbers stay raw + diagnostic.
pub fn carrier_hz(freq: i64) -> Option<f64> {
    match freq {
        1 => Some(1_575_420_000.0),
        2 => Some(1_227_600_000.0),
        5 => Some(1_176_450_000.0),
        _ => None,
    }
}

/// One parsed observation line, raw fields kept as written.
#[derive(Debug, Clone, Default)]
pub struct ObsLine {
    pub line_no: i64,
    pub kind: String, // "obs" | "clk"
    pub epoch_raw: String,
    pub epoch_s: Option<f64>, // seconds since first epoch of the file
    pub station: String,
    pub sat: String,
    pub freq: Option<i64>,
    pub code: String,
    pub c_raw: String,
    pub c_val: Option<f64>, // pseudorange, m
    pub l_raw: String,
    pub l_val: Option<f64>, // carrier phase, cycles
    pub d_raw: String,
    pub d_val: Option<f64>, // Doppler, Hz
    pub lli: Option<i64>,
    pub s_raw: String,
    pub s_val: Option<f64>, // SNR, dB-Hz
    pub clk_raw: String,
    pub clk_val: Option<f64>, // receiver clock jump, s
    pub diags: Vec<String>,
}

/// Scope of a derived object, used for dependency-local invalidation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Scope {
    pub station: String,
    pub sat: String,    // "" = whole station (e.g. clock events)
    pub freq: i64,      // 0 = frequency-independent
}

impl Scope {
    pub fn intersects(&self, other: &Scope) -> bool {
        if self.station != other.station {
            return false;
        }
        let sat_ok = self.sat.is_empty() || other.sat.is_empty() || self.sat == other.sat;
        let freq_ok = self.freq == 0 || other.freq == 0 || self.freq == other.freq;
        sat_ok && freq_ok
    }
}

/// Machine-readable judgement event payload kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    InputImported {
        input_id: String,
        version: i64,
        parent_id: Option<String>,
        label: String,
        content_hash: String,
        changed_scopes: Vec<Scope>,
        n_lines: i64,
        n_diagnostics: i64,
    },
    DetectionRun {
        params_fp: String,
        algo_version: String,
        input_id: String,
        n_arcs: i64,
        n_candidates: i64,
        superseded: i64,
    },
    CandidateDecided {
        decision_id: String,
        candidate_id: String,
        action: String,
        branch: String,
        chosen_offset_cycles: Option<f64>,
        note: String,
    },
    DecisionUndone {
        decision_id: String,
        undone_decision_id: String,
    },
    RepairWritten {
        repair_id: String,
        arc_id: String,
        from_epoch_s: f64,
        offset_cycles: f64,
        candidate_id: Option<String>,
        decision_id: Option<String>,
    },
    ResultsStaled {
        reason: String,
        n_arcs: i64,
        n_candidates: i64,
        n_combos: i64,
    },
    ArchiveExported {
        n_rows: i64,
    },
    ArchiveImported {
        n_rows: i64,
    },
    CasePackStep {
        step: String,
        detail: String,
    },
}
