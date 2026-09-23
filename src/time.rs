//! Virtual time (deterministic, test-friendly). Real wall clock is never used
//! for event ordering: tests inject an epoch and step the clock by fixed ms.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualClock {
    epoch_ms: i64,
    now_ms: i64,
}

impl VirtualClock {
    pub fn new(epoch_ms: i64) -> Self {
        Self { epoch_ms, now_ms: epoch_ms }
    }
    pub fn now_ms(&self) -> i64 {
        self.now_ms
    }
    pub fn advance(&mut self, delta_ms: i64) -> i64 {
        self.now_ms += delta_ms;
        self.now_ms
    }
    pub fn epoch_ms(&self) -> i64 {
        self.epoch_ms
    }
}

/// Convert a proleptic-Gregorian civil date to days since 1970-01-01
/// (Howard Hinnant's civil-from-days algorithm, inverse).
pub fn civil_to_days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Strict parser for the case format timestamps:
/// `YYYY-MM-DDTHH:MM:SS[.fff...]Z`. Preserves the raw text separately at the
/// call site; this function only performs the normalization.
pub fn parse_iso_utc(s: &str) -> Result<f64, String> {
    let b = s.as_bytes();
    let _ = b;
    let pick = |a: usize, b: usize| -> Result<i64, String> {
        s.get(a..b)
            .and_then(|x| x.parse().ok())
            .ok_or_else(|| format!("bad timestamp `{s}`"))
    };
    if s.len() < 20 || !s.ends_with('Z') {
        return Err(format!("bad timestamp `{s}` (expected YYYY-MM-DDTHH:MM:SSZ)"));
    }
    if &s[4..5] != "-" || &s[7..8] != "-" || &s[10..11] != "T" || &s[13..14] != ":" || &s[16..17] != ":" {
        return Err(format!("bad timestamp `{s}` (separators)"));
    }
    let year = pick(0, 4)?;
    let mon: u32 = pick(5, 7)? as u32;
    let day: u32 = pick(8, 10)? as u32;
    let hour: u32 = pick(11, 13)? as u32;
    let min: u32 = pick(14, 16)? as u32;
    let sec: u32 = pick(17, 19)? as u32;
    if !(1..=12).contains(&mon) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return Err(format!("bad timestamp `{s}` (range)"));
    }
    let mut frac = 0.0_f64;
    if s.len() > 20 {
        if &s[20..21] != "." {
            return Err(format!("bad timestamp `{s}` (fraction)"));
        }
        let rest = &s[21..s.len() - 1];
        if rest.is_empty() || !rest.bytes().all(|c| c.is_ascii_digit()) {
            return Err(format!("bad timestamp `{s}` (fraction digits)"));
        }
        frac = format!("0.{rest}").parse().map_err(|_| format!("bad timestamp `{s}`"))?;
    }
    let days = civil_to_days(year, mon, day);
    Ok(days as f64 * 86400.0 + hour as f64 * 3600.0 + min as f64 * 60.0 + sec as f64 + frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_instants() {
        assert_eq!(parse_iso_utc("1970-01-01T00:00:00Z").unwrap(), 0.0);
        assert!((parse_iso_utc("2024-03-01T00:00:00Z").unwrap() - 1709251200.0).abs() < 1e-6);
        assert!((parse_iso_utc("2024-03-01T00:00:00.500Z").unwrap() - 1709251200.5).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_timestamps() {
        assert!(parse_iso_utc("2024/03/01T00:00:00Z").is_err());
        assert!(parse_iso_utc("2024-13-01T00:00:00Z").is_err());
        assert!(parse_iso_utc("not-a-time").is_err());
    }

    #[test]
    fn clock_only_moves_when_stepped() {
        let mut c = VirtualClock::new(1_000);
        assert_eq!(c.now_ms(), 1_000);
        assert_eq!(c.advance(250), 1_250);
        assert_eq!(c.advance(0), 1_250);
    }
}
