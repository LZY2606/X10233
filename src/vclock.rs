//! Deterministic virtual clock + seeded RNG.
//!
//! All derived timestamps come from this clock so replays are byte-stable.
//! "Virtual time" advances by a fixed tick per emitted event; real wall-clock
//! time is never stored in project data.

#[derive(Debug, Clone)]
pub struct VClock {
    epoch_ms: u64,
    seq: u64,
}

impl Default for VClock {
    fn default() -> Self {
        Self::new(1_700_000_000_000)
    }
}

impl VClock {
    pub const fn new(start_ms: u64) -> Self {
        Self { epoch_ms: start_ms, seq: 0 }
    }

    pub fn now_ms(&mut self) -> u64 {
        let t = self.epoch_ms;
        self.epoch_ms += 1;
        self.seq += 1;
        t
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// xorshift64* RNG with a fixed seed by default.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn seeded(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// deterministic float in [0, 1)
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
