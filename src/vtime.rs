//! Virtual clock: deterministic timestamps from a fixed seed, so replay never
//! depends on wall-clock time. Tests construct the app with a frozen origin.
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct VClock {
    inner: Arc<Mutex<State>>,
}

struct State {
    origin_ns: i64,
    step_ns: i64,
    counter: i64,
}

impl VClock {
    pub fn new(origin_ns: i64, step_ns: i64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State { origin_ns, step_ns, counter: 0 })),
        }
    }

    /// Default fixed seed: 2026-01-01T00:00:00Z as epoch nanoseconds, 1 ms
    /// deterministic increments.
    pub fn seeded() -> Self {
        let origin = 1_767_225_600_000_000_000i64;
        Self::new(origin, 1_000_000)
    }

    /// Monotonic, strictly increasing timestamp.
    pub fn now_ns(&self) -> i64 {
        let mut s = self.inner.lock().unwrap();
        s.counter += 1;
        s.origin_ns + s.step_ns * s.counter
    }

    pub fn now_iso(&self) -> String {
        iso_from_ns(self.now_ns())
    }
}

/// Minimal deterministic ns -> ISO8601 UTC (seconds precision is enough for
/// event ordering and keeps the format stable without a date dependency).
pub fn iso_from_ns(ns: i64) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    let (y, mo, d, hh, mi, ss) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mi:02}:{ss:02}Z")
}

fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let hh = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z / 146_097 } else { (z - 146_096) / 146_097 };
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y0 = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y0 + if month <= 2 { 1 } else { 0 };
    (year, month, day, hh, mi, ss)
}
