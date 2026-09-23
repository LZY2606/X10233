// Virtual, deterministic clock. The domain never reads the wall clock; project
// time is a fixed virtual origin plus a monotonic counter. Tests set a seed and
// a virtual origin, so runs are fully reproducible.

pub struct VirtualClock {
    origin_iso: String,
    counter: u64,
}

impl VirtualClock {
    pub fn new(origin_iso: &str) -> Self {
        Self { origin_iso: origin_iso.to_string(), counter: 0 }
    }

    // Deterministic, monotonically increasing ISO-ish stamp derived from the
    // virtual origin and an internal counter (no SystemTime, no randomness).
    pub fn tick(&mut self) -> String {
        self.counter += 1;
        let micros = self.counter * 1000;
        format!("{}#v{:06}.{:06}", self.origin_iso, self.counter, micros)
    }

    pub fn origin(&self) -> &str {
        &self.origin_iso
    }
}
