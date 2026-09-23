//! Virtual clock and stable identifier helpers.
//!
//! Every timestamp in the system comes from a [`Clock`] so that tests can run
//! with fully virtual time. Stable identifiers are content/position based so
//! that export -> delete -> import keeps identity, order and provenance.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Deterministic clock advanced explicitly (fixed seed runs only).
#[derive(Clone)]
pub struct VirtualClock {
    inner: Arc<Mutex<i64>>,
}

impl VirtualClock {
    pub fn new(start_ms: i64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(start_ms)),
        }
    }
    pub fn advance(&self, ms: i64) {
        *self.inner.lock().unwrap() += ms;
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> i64 {
        *self.inner.lock().unwrap()
    }
}

/// `EP07`, `BR02`, ... zero padded stable ids from a sequence number.
pub fn seq_id(prefix: &str, n: i64, width: usize) -> String {
    format!("{}{:0width$}", prefix, n, width = width)
}

pub fn epoch_id(idx: i64) -> String {
    seq_id("EP", idx, 2)
}

/// GPS calendar conversion (synthetic offline case uses a fixed GPS epoch).
/// Returns seconds since 1980-01-06 00:00:00 UTC, *without* leap seconds; the
/// case header records the datum explicitly.
pub fn ymdhms_to_gps_seconds(year: i64, month: u32, day: u32, h: u32, m: u32, s: u32) -> f64 {
    let days = days_from_civil(year as i64, month, day) - days_from_civil(1980, 1, 6);
    days as f64 * 86_400.0 + (h as f64) * 3600.0 + (m as f64) * 60.0 + s as f64
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
