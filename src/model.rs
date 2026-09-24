//! Core shared types: virtual clock, stable identities, time & hash helpers.

use sha2::{Digest, Sha256};

/// Algorithm version stamped on every derived value.
pub const ALGO_VERSION: &str = "gsb-detect-1.0.0";

/// Virtual clock so tests can run on fixed time.
#[derive(Clone, Debug)]
pub enum Clock {
    System,
    Fixed(i64),
}

impl Clock {
    pub fn now_ms(&self) -> i64 {
        match self {
            Clock::System => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            Clock::Fixed(t) => *t,
        }
    }
}

pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Stable identity: derived purely from a kind prefix and a seed string.
/// Survives export / delete / import because it is content addressed.
pub fn make_id(kind: &str, seed: &str) -> String {
    let h = sha256_hex(&format!("{}|{}", kind, seed));
    format!("{}_{}", kind, &h[..16])
}

// ---- civil calendar <-> unix ms (no external deps, deterministic) ----

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` into unix milliseconds.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z').trim_end_matches('z');
    let (date, time) = s.split_once('T')?;
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next()?.parse().ok()?;
    let sec_s = tp.next()?;
    let (ss, frac_ms) = match sec_s.split_once('.') {
        Some((a, b)) => {
            let ss: i64 = a.parse().ok()?;
            let mut f: i64 = b.parse().ok()?;
            let mut w = b.len();
            while w < 3 { f *= 10; w += 1; }
            while w > 3 { f /= 10; w -= 1; }
            (ss, f)
        }
        None => (sec_s.parse().ok()?, 0),
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    Some((((days * 24 + hh) * 60 + mm) * 60 + ss) * 1000 + frac_ms)
}

pub fn format_iso_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, mo, d) = civil_from_days(days);
    let hh = rem / 3_600_000;
    let mm = (rem % 3_600_000) / 60_000;
    let ss = (rem % 60_000) / 1000;
    let mss = rem % 1000;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, hh, mm, ss, mss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_roundtrip() {
        let t = parse_iso_ms("2026-09-19T00:00:30.500Z").unwrap();
        assert_eq!(format_iso_ms(t), "2026-09-19T00:00:30.500Z");
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert!(parse_iso_ms("not-a-time").is_none());
        assert!(parse_iso_ms("2026-13-01T00:00:00Z").is_none());
    }

    #[test]
    fn ids_stable() {
        assert_eq!(make_id("arc", "a|b"), make_id("arc", "a|b"));
        assert_ne!(make_id("arc", "a|b"), make_id("arc", "a|c"));
    }
}
