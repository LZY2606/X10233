//! Stable, content-derived identities.
//!
//! Every displayed value carries an identity derived from its *natural key*
//! (domain parentage + ordinal), so export -> delete DB -> import preserves
//! identity, order and source relations without random UUIDs.

use sha2::{Digest, Sha256};

/// 12-char hex fingerprint of a deterministic string key.
pub fn fid(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b"\u{1f}");
        }
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    hex::encode(&d[..6])
}

/// 16-char hex fingerprint used for parameter hashes.
pub fn fp(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b"\u{1f}");
        }
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    hex::encode(&d[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_distinct() {
        assert_eq!(fid(&["a", "b"]), fid(&["a", "b"]));
        assert_ne!(fid(&["a", "b"]), fid(&["a", "c"]));
        assert_ne!(fid(&["a,b"]), fid(&["a", "b"]));
    }
}
