//! Deterministic virtual clock.
//!
//! Tests and the bundled case use a fixed seed so that event timestamps and any
//! pseudo-random tie-breaking are reproducible. No wall clock or public network
//! is consulted in the domain logic.

/// A clock that hands out monotonically increasing virtual seconds.
pub trait Clock {
    fn now(&mut self) -> f64;
}

/// Fixed-seed deterministic clock. Starts at an arbitrary but fixed epoch and
/// advances by a fixed step on every call.
#[derive(Debug, Clone)]
pub struct VirtualClock {
    t: f64,
    step: f64,
}

impl VirtualClock {
    pub fn seeded(seed: u64) -> Self {
        // Map the seed to a stable start offset (GPST-like virtual seconds).
        let start = 1_000.0 + (seed % 100_000) as f64;
        Self { t: start, step: 1.0 }
    }

    pub fn with_step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }
}

impl Clock for VirtualClock {
    fn now(&mut self) -> f64 {
        let v = self.t;
        self.t += self.step;
        v
    }
}

/// Fixed-seed, deterministic PRNG (xorshift64*) used only for stable tie-break
/// ordering so that "equal split" candidates never depend on HashMap order.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn seeded(seed: u64) -> Self {
        Self { state: if seed == 0 { 0x9e3779b97f4a7c15 } else { seed } }
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform-ish float in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_deterministic_for_seed() {
        let a: Vec<f64> = (0..3).map(|_| VirtualClock::seeded(42).now()).collect();
        // Fresh clock same seed -> same first value; advancing monotonic.
        let mut c = VirtualClock::seeded(42);
        let v0 = c.now();
        assert_eq!(a[0], v0);
        assert!(c.now() > v0);
    }

    #[test]
    fn rng_is_stable() {
        let mut r = Rng::seeded(7);
        let x = (r.next_u64(), r.next_u64());
        let mut r2 = Rng::seeded(7);
        assert_eq!(x, (r2.next_u64(), r2.next_u64()));
    }
}
