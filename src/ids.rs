//! Stable content-addressed identifiers.
//!
//! Every derived object (residual point, candidate, hypothesis, arc, run) is
//! identified by a deterministic digest of its *content*, so exporting a
//! project, deleting the local database and importing it again reproduces the
//! exact same ids, ordering and provenance edges.

use sha2::{Digest, Sha256};

/// Compute a stable id: `prefix_` + first 16 hex chars of SHA-256 over the
/// canonicalised material.
pub fn stable_id(prefix: &str, material: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(b"\x00");
    for (idx, part) in material.iter().enumerate() {
        if idx > 0 {
            hasher.update(b"\x1f");
        }
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    format!("{}_{}", prefix, hex::encode(&digest[..8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_distinct() {
        let a = stable_id("cand", &["run1".into(), "G01".into(), "100".into()]);
        let b = stable_id("cand", &["run1".into(), "G01".into(), "100".into()]);
        let c = stable_id("cand", &["run1".into(), "G01".into(), "101".into()]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("cand_"));
        assert_eq!(a.len(), "cand_".len() + 16);
    }
}
