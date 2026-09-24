//! 虚拟时间：生产用系统 UTC，测试用固定时钟，全部走同一 trait。
use std::sync::{Arc, Mutex};

pub trait Clock: Send + Sync {
    fn now(&self) -> String;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> String {
        // 不依赖 time/chrono：RFC3339 由 std 无法直接生成，这里用 epoch 秒拼 UTC。
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        epoch_to_rfc3339(secs, 0)
    }
}

/// 测试时钟：固定起点 + 每次调用步进固定秒数，保证可重放。
pub struct FixedClock {
    epoch: Mutex<i64>,
    step: i64,
}
impl FixedClock {
    pub fn new(start_unix: i64, step_seconds: i64) -> Arc<Self> {
        Arc::new(FixedClock { epoch: Mutex::new(start_unix), step: step_seconds })
    }
}
impl Clock for FixedClock {
    fn now(&self) -> String {
        let mut g = self.epoch.lock().unwrap();
        let s = epoch_to_rfc3339(*g, 0);
        *g += self.step;
        s
    }
}

/// Howard Hinnant days_from_civil，无外部依赖的公历换算（UTC 日期）。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_to_unix(y: i64, m: u32, d: u32, hh: u32, mm: u32, secs: f64) -> f64 {
    let days = days_from_civil(y, m, d);
    days as f64 * 86400.0 + hh as f64 * 3600.0 + mm as f64 * 60.0 + secs
}

pub fn epoch_to_rfc3339(unix: i64, nanos: u32) -> String {
    let days = unix.div_euclid(86400);
    let mut rem = unix.rem_euclid(86400);
    let hh = rem / 3600;
    rem %= 3600;
    let mm = rem / 60;
    let ss = rem % 60;
    // civil from days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    if nanos == 0 {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
    } else {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{nanos:09}Z")
    }
}
