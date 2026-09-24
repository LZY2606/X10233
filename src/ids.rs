//! Deterministic, content-derived stable identities.
//! Ids never depend on wall clock, process, or machine state, so an
//! export -> delete -> import round trip reproduces identical ids.

/// FNV-1a 64-bit, hex-encoded. Deterministic across runs and platforms.
pub fn fnv_hex(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0x1f;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

pub fn id(prefix: &str, parts: &[&str]) -> String {
    format!("{}:{}", prefix, fnv_hex(parts))
}
