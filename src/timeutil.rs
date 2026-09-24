//! Civil-calendar <-> epoch-millis conversion (no external crates, no leap seconds).
//! GNSS raw epochs are kept as raw text elsewhere; this is the derived-layer
//! normalization used for ordering and dt computations.

/// Days from civil epoch (Howard Hinnant's algorithm), 1970-01-01 = 0.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0,11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Milliseconds since 1970-01-01T00:00:00 for a civil timestamp.
pub fn civil_to_ms(y: i64, mo: u32, d: u32, h: u32, mi: u32, sec: f64) -> i64 {
    let days = days_from_civil(y, mo, d);
    let whole = sec.trunc() as i64;
    let frac_ms = ((sec - whole as f64) * 1000.0).round() as i64;
    (((days * 24 + h as i64) * 60 + mi as i64) * 60 + whole) * 1000 + frac_ms
}

/// Parse a strict ISO-8601 UTC timestamp `YYYY-MM-DDTHH:MM:SS[.fff]Z` to epoch ms.
pub fn iso_to_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let mo: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    let h: u32 = s[11..13].parse().ok()?;
    let mi: u32 = s[14..16].parse().ok()?;
    let rest = &s[17..];
    let rest = rest.strip_suffix('Z')?;
    let sec: f64 = rest.parse().ok()?;
    Some(civil_to_ms(y, mo, d, h, mi, sec))
}

/// Format epoch ms as ISO-8601 UTC with millisecond precision.
pub fn ms_to_iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, mo, d) = civil_from_days(days);
    let h = rem / 3_600_000;
    let mi = (rem % 3_600_000) / 60_000;
    let s = (rem % 60_000) / 1000;
    let msec = rem % 1000;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, h, mi, s, msec)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let ms = civil_to_ms(2024, 3, 1, 12, 30, 15.250);
        assert_eq!(ms_to_iso(ms), "2024-03-01T12:30:15.250Z");
        assert_eq!(iso_to_ms("2024-03-01T12:30:15.250Z"), Some(ms));
        assert_eq!(ms_to_iso(0), "1970-01-01T00:00:00.000Z");
    }
}
