use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Stable, content-addressed identity: `prefix_hex20`.
/// Same content -> same id, across export / delete / re-import.
pub fn id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b"\x1e");
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    let digest = h.finalize();
    format!("{}_{}", prefix, &hex(&digest)[..20])
}
