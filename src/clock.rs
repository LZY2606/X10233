use std::sync::Mutex;

pub trait Clock: Send + Sync {
    fn now_iso(&self) -> String;
    fn now_ms(&self) -> i64;
}

pub struct WallClock;

impl Clock for WallClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
    fn now_iso(&self) -> String {
        format_iso(self.now_ms())
    }
}

/// Deterministic clock used by every test: advance explicitly, never reads the
/// host wall clock.
pub struct VirtualClock {
    ms: Mutex<i64>,
}

impl VirtualClock {
    pub fn new(epoch_ms: i64) -> Self {
        VirtualClock { ms: Mutex::new(epoch_ms) }
    }
    pub fn advance(&self, ms: i64) {
        *self.ms.lock().unwrap() += ms;
    }
    pub fn set(&self, ms: i64) {
        *self.ms.lock().unwrap() = ms;
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> i64 {
        *self.ms.lock().unwrap()
    }
    fn now_iso(&self) -> String {
        format_iso(self.now_ms())
    }
}

/// Days in a civil calendar month, Howard Hinnant's civil-from-days inverse.
pub fn format_iso(ms_since_unix_epoch: i64) -> String {
    let total_seconds = ms_since_unix_epoch.div_euclid(1000);
    let millis = ms_since_unix_epoch.rem_euclid(1000);
    let days = total_seconds.div_euclid(86_400);
    let secs_of_day = total_seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_and_known_dates() {
        assert_eq!(format_iso(0), "1970-01-01T00:00:00.000Z");
        // 2000-01-01T00:00:00Z = 10957 days after epoch
        assert_eq!(format_iso(10957 * 86_400_000), "2000-01-01T00:00:00.000Z");
    }
}
