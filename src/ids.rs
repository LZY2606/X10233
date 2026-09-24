// Stable identity + deterministic fingerprints.
//
// Content ids are FNV-1a over canonical text; they stay stable across export,
// database deletion and re-import. Process ids (runs/events/decisions) come
// from a fixed-seed, deterministic xorshift64 sequence inside a project so the
// whole application is replayable under virtual time in tests.

pub const ALGORITHM_VERSION: &str = "detector-core@1.0.0";

/// FNV-1a 64 bit, rendered as a 16-hex-char id.
pub fn fnv_id(prefix: &str, parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut eat = |b: u8| {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    eat(b'[');
    for p in parts {
        eat(b'|');
        for b in p.bytes() {
            eat(b);
        }
    }
    eat(b']');
    format!("{prefix}_{hash:016x}")
}

pub fn content_id(kind: &str, canonical: &str) -> String {
    fnv_id(kind, &[canonical])
}

/// Deterministic 64-bit PRNG (xorshift). Seeded identically for every project so
/// test replay is fixed; no OS randomness is used for persisted identity.
pub struct Rng64 {
    state: u64,
}

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        Rng64 { state: if seed == 0 { 0x9e3779b97f4a7c15 } else { seed } }
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    pub fn id(&mut self, prefix: &str) -> String {
        format!("{prefix}_{:016x}", self.next())
    }
}

/// Parse a hex-suffix id back (for ordering/inspection utilities).
pub fn id_suffix(id: &str) -> Option<u64> {
    let (_, hex) = id.split_once('_')?;
    u64::from_str_radix(hex, 16).ok()
}
