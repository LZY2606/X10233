//! Deterministic, content-derived stable identities.
//!
//! Every entity id is a hash of its defining content plus the ids of its
//! direct sources, so export -> delete -> import reproduces identical ids,
//! ordering and provenance without any randomness or wall-clock input.

use sha2::{Digest, Sha256};

pub fn stable_id(kind: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"\0");
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\0");
    }
    let hex = format!("{:x}", h.finalize());
    format!("{}-{}", kind, &hex[..24])
}

/// Fingerprint of a parameter set (order-independent).
pub fn param_fingerprint(params: &[(String, String)]) -> String {
    let mut sorted: Vec<(String, String)> = params.to_vec();
    sorted.sort();
    let parts: Vec<String> = sorted.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
    let joined = parts.join(";");
    let mut h = Sha256::new();
    h.update(b"params\0");
    h.update(joined.as_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}
