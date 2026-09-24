//! Deterministic virtual time.
//!
//! All timestamps emitted by the engine are taken from a `Clock`, never the
//! host wall clock. Tests use `FixedClock` with a fixed origin so runs are
//! bit-for-bit reproducible; the server uses a monotonic virtual clock seeded
//! from the database (still advancing only through explicit ticks).
//!
//! Time scale: GPS-like continuous seconds since 2000-01-01T00:00:00, leap
//! seconds intentionally ignored (documented on every time value as
//! `gps_seconds_since_2000-01-01`).

/// Epoch instant in the data domain: GPS-like continuous seconds since
/// 2000-01-01T00:00:00.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct GpsTime(pub f64);

impl GpsTime {
    pub fn secs(self) -> f64 {
        self.0
    }
}

/// Virtual clock used for event/ledger timestamps (milliseconds since origin).
pub trait VirtualClock: Send {
    fn now_ms(&self) -> i64;
    fn advance_ms(&mut self, ms: i64);
}

/// Fixed deterministic clock for tests and replay.
#[derive(Debug, Clone)]
pub struct FixedClock {
    pub t_ms: i64,
}

impl FixedClock {
    /// Fixed seed origin: 2024-01-01T00:00:00 expressed as ms since 2000.
    pub fn seeded() -> Self {
        FixedClock {
            t_ms: 1_704_067_200_000 - 946_684_800_000,
        }
    }
    pub fn at(t_ms: i64) -> Self {
        FixedClock { t_ms }
    }
}

impl VirtualClock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.t_ms
    }
    fn advance_ms(&mut self, ms: i64) {
        self.t_ms += ms;
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]` (also accepts a space separator) into
/// GPS-like seconds since 2000-01-01T00:00:00.
pub fn parse_iso(s: &str) -> Result<f64, String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return Err(format!("timestamp too short: {s:?}"));
    }
    let num = |r: std::ops::Range<usize>| -> Result<i64, String> {
        std::str::from_utf8(&b[r])
            .map_err(|e| e.to_string())?
            .parse::<i64>()
            .map_err(|e| format!("bad number in timestamp {s:?}: {e}"))
    };
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    if b[4] != b'-' || b[7] != b'-' {
        return Err(format!("expected dashes in timestamp: {s:?}"));
    }
    if b[10] != b'T' && b[10] != b' ' {
        return Err(format!("expected T/space separator: {s:?}"));
    }
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    if b[13] != b':' || b[16] != b':' {
        return Err(format!("expected colons in timestamp: {s:?}"));
    }
    let frac = if b.len() > 19 {
        if b[19] != b'.' {
            return Err(format!("expected fractional dot: {s:?}"));
        }
        let rest = &s[20..];
        if !rest.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("bad fractional seconds: {s:?}"));
        }
        let pad = format!("{rest:0<9}");
        pad[..9].parse::<i64>().map_err(|e| e.to_string())? as f64 / 1e9
    } else {
        0.0
    };
    let days = days_from_civil(year, month, day) - days_from_civil(2000, 1, 1);
    Ok(days as f64 * 86400.0 + hour as f64 * 3600.0 + minute as f64 * 60.0 + second as f64 + frac)
}

/// Render seconds-since-2000 back to `YYYY-MM-DDTHH:MM:SS.fff`.
pub fn format_iso(t: f64) -> String {
    let total_ms = (t * 1000.0).round() as i64;
    let day0 = days_from_civil(2000, 1, 1);
    let days = total_ms.div_euclid(86_400_000);
    let mut ms_of_day = total_ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(day0 + days);
    let h = ms_of_day / 3_600_000;
    ms_of_day %= 3_600_000;
    let min = ms_of_day / 60_000;
    ms_of_day %= 60_000;
    let sec = ms_of_day / 1000;
    let milli = ms_of_day % 1000;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{sec:02}.{milli:03}")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_epoch() {
        for iso in [
            "2000-01-01T00:00:00",
            "2024-03-15T11:22:33.500",
            "1999-12-31T23:59:59",
            "2026-09-25T08:30:00.125",
        ] {
            let t = parse_iso(iso).unwrap();
            let back = format_iso(t);
            assert_eq!(back, iso, "roundtrip {iso}");
        }
    }

    #[test]
    fn unequal_gap_arithmetic() {
        let a = parse_iso("2024-01-01T00:00:00").unwrap();
        let b = parse_iso("2024-01-01T00:00:07").unwrap();
        assert!((b - a - 7.0).abs() < 1e-9);
    }
}
