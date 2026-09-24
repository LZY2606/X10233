//! Observation codes, frequency numbers and the synthetic frequency table.
//!
//! Frequencies are explicit import parameters (RINEX header semantics), but
//! the bundled offline case uses a documented synthetic frequency table so
//! the package is self-contained. Codes that cannot be resolved stay recorded
//! with a diagnostic instead of being silently dropped.

use serde::{Deserialize, Serialize};

/// Frequency band (frequency number) for an observation code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Band {
    pub constellation: String,
    /// RINEX frequency number, 1-based.
    pub freq_number: u8,
    pub code: String,
}

/// Known frequency for a band, in Hz.
pub struct FreqEntry {
    pub band: Band,
    pub hz: f64,
}

/// Synthetic GNSS frequency table used by the bundled case (Hz).
///
/// Chosen as simple, documented values so MW/GF arithmetic is exact and
/// inspectable; these are *not* claim to match the broadcast constellations.
pub fn synthetic_table() -> Vec<FreqEntry> {
    let g = |n: u8, code: &str, hz: f64| FreqEntry {
        band: Band { constellation: "G".into(), freq_number: n, code: code.into() },
        hz,
    };
    vec![
        g(1, "C1C", 1.500_000_000.0),
        g(1, "L1C", 1.500_000_000.0),
        g(1, "D1C", 1.500_000_000.0),
        g(1, "S1C", 1.500_000_000.0),
        g(2, "C2W", 1.200_000_000.0),
        g(2, "L2W", 1.200_000_000.0),
        g(2, "D2W", 1.200_000_000.0),
        g(2, "S2W", 1.200_000_000.0),
        g(3, "C5X", 1.100_000_000.0),
        g(3, "L5X", 1.100_000_000.0),
        g(3, "D5X", 1.100_000_000.0),
        g(3, "S5X", 1.100_000_000.0),
    ]
}

/// Observation kind implied by a RINEX-like three-character code prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
    Unknown,
}

pub fn code_kind(code: &str) -> ObsKind {
    match code.chars().next() {
        Some('C') => ObsKind::Pseudorange,
        Some('L') => ObsKind::Carrier,
        Some('D') => ObsKind::Doppler,
        Some('S') => ObsKind::Snr,
        _ => ObsKind::Unknown,
    }
}

/// Frequency (Hz) for a code via the synthetic table, with explicit
/// frequencies supplied at import taking precedence.
pub fn lookup_hz(code: &str, explicit: &[(String, f64)]) -> Result<f64, String> {
    for (c, hz) in explicit {
        if c == code {
            return Ok(*hz);
        }
    }
    for e in synthetic_table() {
        if e.band.code == code {
            return Ok(e.hz);
        }
    }
    Err(format!("observation code {} has no resolvable frequency", code))
}

/// Wavelength (m/cycle) for a carrier code.
pub fn wavelength(code: &str, explicit: &[(String, f64)]) -> Result<f64, String> {
    Ok(299_792_458.0 / lookup_hz(code, explicit)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_resolve_and_unknown_codes_diagnose() {
        assert_eq!(lookup_hz("L1C", &[]).unwrap(), 1.5e9);
        assert!(lookup_hz("L9Z", &[]).is_err());
        assert_eq!(code_kind("C2W"), ObsKind::Pseudorange);
        assert_eq!(code_kind("L5X"), ObsKind::Carrier);
        let w1 = wavelength("L1C", &[]).unwrap();
        let w2 = wavelength("L2W", &[]).unwrap();
        assert!((w1 - 0.1998616386666667).abs() < 1e-9);
        assert!((w2 - 0.2498270483333333).abs() < 1e-9);
    }
}
