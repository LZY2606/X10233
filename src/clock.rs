//! Pluggable clock so tests use fixed virtual time and the server uses wall time.

use std::sync::{Arc, Mutex};

pub trait Clock: Send + Sync {
    /// Monotonic-ish virtual timestamp: milliseconds since a fixed origin.
    fn now_ms(&self) -> i64;
}

#[derive(Default)]
pub struct VirtualClock {
    ms: Mutex<i64>,
}

impl VirtualClock {
    pub fn new(start_ms: i64) -> Arc<Self> {
        Arc::new(VirtualClock { ms: Mutex::new(start_ms) })
    }

    /// Advance the clock deterministically; every committed event gets a
    /// distinct, ordered timestamp.
    pub fn advance(&self, delta_ms: i64) {
        *self.ms.lock().unwrap() += delta_ms;
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> i64 {
        *self.ms.lock().unwrap()
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

/// Deterministic pseudo RNG seed used anywhere randomness appears (kept fixed).
pub const FIXED_SEED: u64 = 20260924;
