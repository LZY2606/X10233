//! Stable, content-derived identities. Every displayed value carries one of
//! these ids; export/delete/re-import preserves them because they depend only
//! on content, never on database rowids or wall-clock time.
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Content-addressed id: `<prefix>_<first 24 hex of sha256>`.
pub fn content_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    for p in parts {
        h.update(b"\x1f");
        h.update(p.as_bytes());
    }
    let digest = h.finalize();
    format!("{}_{}", prefix, &hex(&digest)[..24])
}

pub fn sha256_hex(data: &str) -> String {
    let mut h = Sha256::new();
    h.update(data.as_bytes());
    hex(&h.finalize())
}
