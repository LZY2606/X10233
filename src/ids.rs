//! Stable, content-addressed identities.
//!
//! Every first-class object (import, observation, arc, run, candidate,
//! repair, event) gets an id derived from a cryptographic hash of its
//! canonical content.  Because ids depend only on content (never on database
//! row numbers or wall-clock time), they survive export -> delete -> import
//! round-trips unchanged.

use sha2::{Digest, Sha256};

/// Hash a canonical string into a prefixed, truncated hex id.
pub fn content_id(prefix: &str, canonical: &str) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b"\x00");
    h.update(canonical.as_bytes());
    let digest = h.finalize();
    let hex: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

/// Canonical fingerprint of a JSON value (keys sorted by serde_json's map).
pub fn fingerprint(value: &serde_json::Value) -> String {
    content_id("fp", &serde_json::to_string(value).unwrap_or_default())
}

/// Short display form of a stable id: `cand_a1b2c3d4` -> `a1b2c3d4`.
pub fn short(id: &str) -> &str {
    let body = id.rsplit('_').next().unwrap_or(id);
    &body[..body.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_prefixed() {
        let a = content_id("cand", "hello");
        let b = content_id("cand", "hello");
        let c = content_id("cand", "world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("cand_"));
        assert_eq!(short("cand_a1b2c3d4e5f6"), "a1b2c3d4");
    }
}
