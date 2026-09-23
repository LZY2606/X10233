//! Deterministic identity. FNV-1a 64-bit, rendered hex. IDs are pure
//! functions of content so export/delete/re-import keeps the same identity,
//! ordering and provenance without relying on autoincrement rowids.

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn id(prefix: &str, parts: &[&str]) -> String {
    let mut buf = String::new();
    for p in parts {
        buf.push_str(p);
        buf.push('\u{1f}');
    }
    format!("{}_{:016x}", prefix, fnv1a64(buf.as_bytes()))
}

pub fn id_bytes(prefix: &str, bytes: &[u8]) -> String {
    format!("{}_{:016x}", prefix, fnv1a64(bytes))
}

/// Monotonic virtual clock used for tests and the deterministic demo.
#[derive(Clone)]
pub struct VirtualClock {
    base_ns: i64,
    step_ns: i64,
    ticks: std::cell::Cell<i64>,
}

impl VirtualClock {
    pub fn new(base_ns: i64, step_ns: i64) -> Self {
        VirtualClock { base_ns, step_ns, ticks: std::cell::Cell::new(0) }
    }
    pub fn now_ns(&self) -> i64 {
        let t = self.ticks.get();
        self.ticks.set(t + 1);
        self.base_ns + t * self.step_ns
    }
    pub fn peek_ns(&self) -> i64 {
        self.base_ns + self.ticks.get() * self.step_ns
    }
}
