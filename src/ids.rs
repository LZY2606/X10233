//! Stable, content-addressed identifiers. They survive export/import and a
//! deleted local database because they depend only on domain content, never
//! on SQLite rowids or creation time.

use sha2::{Digest, Sha256};

pub fn hash_bytes(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        h.update((i as u64).to_le_bytes());
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    hex::encode(h.finalize())
}

/// 16-hex-char fingerprint prefixed to give stable, readable ids.
pub fn short_hash(data: &[u8]) -> String {
    let full = hash_bytes(&[data]);
    full[..16].to_string()
}

pub fn id_for(prefix: &str, content: &str) -> String {
    format!("{prefix}:{}", short_hash(content.as_bytes()))
}

/// Deterministic RNG (xorshift64*) used by generators; fixed seed everywhere.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform f64 in [-1, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}
