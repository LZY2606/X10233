use std::sync::Mutex;

/// Virtual time. Tests and replays use [FixedClock] so every event timestamp is
/// reproducible; the packaged demo also pins the clock.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Deterministic monotonic clock: each call advances by `inc_ms`.
pub struct FixedClock {
    next: Mutex<i64>,
    inc_ms: i64,
}

impl FixedClock {
    pub fn new(start_ms: i64, inc_ms: i64) -> Self {
        Self { next: Mutex::new(start_ms), inc_ms }
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        let mut g = self.next.lock().unwrap();
        let v = *g;
        *g += self.inc_ms;
        v
    }
}
