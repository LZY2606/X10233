//! Domain model: stable identities, units, virtual (logical) time, parameters.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Speed of light, m/s (SI, exact).
pub const C: f64 = 299_792_458.0;
/// GPS L1 carrier frequency, Hz.
pub const F1_HZ: f64 = 1575.42e6;
/// GPS L2 carrier frequency, Hz.
pub const F2_HZ: f64 = 1227.60e6;
/// Algorithm version stamped on every derived value.
pub const ALGO_VERSION: &str = "gsb-detect/1.0.0";

/// Stable, content-addressed identity: `prefix_hex20`.
pub fn content_id(prefix: &str, payload: &str) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b"|");
    h.update(payload.as_bytes());
    let hex = format!("{:x}", h.finalize());
    format!("{}_{}", prefix, &hex[..20])
}

/// Carrier wavelength in meters for a RINEX frequency number, if known.
pub fn wavelength_m(freq_number: i64) -> Option<f64> {
    freq_hz(freq_number).map(|f| C / f)
}

/// Carrier frequency in Hz for a RINEX frequency number, if known.
pub fn freq_hz(freq_number: i64) -> Option<f64> {
    match freq_number {
        1 => Some(F1_HZ),
        2 => Some(F2_HZ),
        _ => None,
    }
}

/// Wide-lane wavelength (m) for the L1/L2 pair.
pub fn wide_lane_wavelength_m() -> f64 {
    C / (F1_HZ - F2_HZ)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719_468
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` into seconds since Unix epoch.
/// Returns None when the label cannot be converted; callers must keep the
/// raw label in place and attach a diagnostic instead of dropping the value.
pub fn parse_epoch_secs(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    if s.get(4..5)? != "-" || s.get(7..8)? != "-" || s.get(10..11)? != "T" {
        return None;
    }
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    if s.get(13..14)? != ":" || s.get(16..17)? != ":" {
        return None;
    }
    let hh: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let sec_whole: i64 = s.get(17..19)?.parse().ok()?;
    let mut frac = 0.0f64;
    let rest = &s[19..];
    let rest = if let Some(r) = rest.strip_prefix('.') {
        let digits: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        frac = format!("0.{}", digits).parse().ok()?;
        &r[digits.len()..]
    } else {
        rest
    };
    if rest != "Z" {
        return None;
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || hh > 23 || mi > 59 || sec_whole > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    Some((days * 86_400 + hh * 3600 + mi * 60 + sec_whole) as f64 + frac)
}

/// Detector / repair parameters. The canonical JSON serialization is the
/// parameter fingerprint input, so every derived value can name the exact
/// rule version that produced it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// Epoch gap (s) above which an arc is broken.
    pub max_gap_s: f64,
    /// Time-difference residual (cycles) that triggers a candidate.
    pub td_threshold_cycles: f64,
    /// Geometry-free jump (m) that triggers a candidate.
    pub gf_threshold_m: f64,
    /// Melbourne-Wubbena jump (wide-lane cycles) that triggers a candidate.
    pub mw_threshold_cycles: f64,
    /// Common-mode P-minus-L jump (m) classified as a receiver clock jump.
    pub clock_common_threshold_m: f64,
    /// Distance from a half-integer residual that marks the slip ambiguous,
    /// emitting every equally-plausible integer offset as its own candidate.
    pub ambiguity_tolerance: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 90.0,
            td_threshold_cycles: 0.5,
            gf_threshold_m: 0.25,
            mw_threshold_cycles: 0.7,
            clock_common_threshold_m: 1000.0,
            ambiguity_tolerance: 0.08,
        }
    }
}

impl Params {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("params serialize")
    }
    pub fn fingerprint(&self) -> String {
        content_id("par", &self.canonical_json())
    }
}

/// A machine-readable diagnostic attached to a value that stayed in place.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

impl Diagnostic {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Diagnostic { code: code.to_string(), message: message.into(), line: None }
    }
    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}
