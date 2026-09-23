//! Deterministic, stable identifiers.
//!
//! Identities are content-addressed from the *stable* parts of an entity
//! (project, type tag, natural keys). They therefore survive export -> delete ->
//! re-import and do not depend on insertion order or an SQLite rowid.

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn id(prefix: &str, parts: &[&str]) -> String {
    let mut material = String::new();
    material.push_str(prefix);
    for p in parts {
        material.push('\u{1}');
        material.push_str(p);
    }
    format!("{}-{:016x}", prefix, fnv1a64(material.as_bytes()))
}

pub fn content_fp(text: &str) -> String {
    format!("{:016x}", fnv1a64(text.as_bytes()))
}
