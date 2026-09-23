//! Injectable clock. Production uses wall time; tests pin a virtual clock with a
//! fixed seed so recorded events never depend on the developer's machine.

use std::sync::{Arc, Mutex};

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

/// Deterministic virtual clock. Seed is explicit; every call advances by a
/// fixed tick so two identical replays produce byte-identical ledgers.
pub struct FakeClock {
    state: Mutex<i64>,
    tick_ms: i64,
}
impl FakeClock {
    pub fn new(seed_ms: i64, tick_ms: i64) -> Arc<Self> {
        Arc::new(FakeClock {
            state: Mutex::new(seed_ms),
            tick_ms,
        })
    }
}
impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        let mut s = self.state.lock().unwrap();
        let v = *s;
        *s += self.tick_ms;
        v
    }
}

/// Fixed-seed deterministic LCG used by the offline case generator. The seed is
/// part of the case manifest, so the "noise" is part of the domain material.
pub struct SeededNoise {
    state: u64,
}
impl SeededNoise {
    pub fn new(seed: u64) -> Self {
        SeededNoise {
            state: if seed == 0 { 0x9e3779b97f4a7c15 } else { seed },
        }
    }
    /// Uniform in [-amp, amp] (mm for phase/code style generation).
    pub fn uniform(&mut self, amp: f64) -> f64 {
        // splitmix64
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        let u = (z >> 11) as f64 / (1u64 << 53) as f64;
        (u * 2.0 - 1.0) * amp
    }
}
