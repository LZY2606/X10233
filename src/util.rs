use sha2::{Digest, Sha256};

pub fn digest_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    let d = h.finalize();
    let mut s = String::new();
    for b in d {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Short, stable, content-addressed identity. Deterministic across machines,
/// databases, export/import round-trips and runs.
pub fn id(kind: &str, parts: &[&str]) -> String {
    format!("{}_{}", kind, &digest_hex(parts)[..16])
}

/// Deterministic virtual clock. Every project starts at the same fixed origin;
/// each recorded action advances the clock by a fixed step. Real wall-clock is
/// never used, so replay on any machine yields identical history.
#[derive(Clone)]
pub struct VirtualClock {
    base: i64,
    step_s: i64,
    n: i64,
}

impl VirtualClock {
    pub fn new() -> Self {
        // 2026-01-01T00:00:00Z measured in seconds since 1970-01-01.
        VirtualClock {
            base: 1_767_225_600,
            step_s: 1,
            n: 0,
        }
    }

    pub fn tick(&mut self) -> String {
        let t = self.base + self.n * self.step_s;
        self.n += 1;
        format_timestamp(t)
    }

    pub fn at(n: i64) -> String {
        format_timestamp(1_767_225_600 + n)
    }
}

pub fn format_timestamp(unix_s: i64) -> String {
    // Proleptic Gregorian conversion, UTC, second precision.
    let days = unix_s.div_euclid(86_400);
    let secs = unix_s.rem_euclid(86_400);
    let hour = secs / 3600;
    let minute = (secs % 3600) / 60;
    let sec = secs % 60;
    let (y, m, d) = ymd_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hour, minute, sec)
}

fn ymd_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m as u32, d as u32)
}

pub fn epoch_to_unix(e: &crate::domain::EpochKey) -> i64 {
    let days = days_from_ymd(e.year as i64, e.month as i64, e.day as i64);
    days * 86_400
        + (e.hour as i64) * 3600
        + (e.minute as i64) * 60
        + e.second.floor() as i64
}

fn days_from_ymd(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let m = month;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719_468
}

pub fn epoch_subseconds(e: &crate::domain::EpochKey) -> f64 {
    e.second.fract()
}
