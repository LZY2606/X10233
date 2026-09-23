//! Stable content identities. Every identity is a UUIDv5 derived from a fixed
//! namespace and stable semantic parts, so export -> delete -> import preserves
//! every id, ordering and provenance relation.

use uuid::Uuid;

pub const NS: Uuid = Uuid::from_u128(0x6e6e7373_2d63_7963_b0c8_a17e55ed0001);

pub fn tag(prefix: &str, u: Uuid) -> String {
    let s = u.simple().to_string();
    format!("{}_{}", prefix, &s[..10])
}

pub fn id(prefix: &str, parts: &[&str]) -> String {
    let mut name = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            name.push('\u{001f}');
        }
        name.push_str(p);
    }
    tag(prefix, Uuid::new_v5(&NS, name.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic() {
        assert_eq!(id("x", &["a", "b"]), id("x", &["a", "b"]));
        assert_ne!(id("x", &["a", "b"]), id("x", &["a", "c"]));
    }
}
