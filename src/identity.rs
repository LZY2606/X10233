//! Stable identities. Derived values get content-addressed ids built from
//! algorithm version, parameter fingerprint and *direct* source ids, so the
//! same replayed graph yields byte-identical ids (and survives export/delete/
//! import). Event ids are uuid v4 strings and are preserved inside archives.

use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn hash16(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        h.update((i as u64).to_le_bytes());
        let bytes = p.as_bytes();
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    let digest = h.finalize();
    let mut out = String::new();
    for b in digest.iter().take(8) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Canonical JSON encoding: keys sorted, no whitespace. Deterministic across
/// machines (f64 serialized by ryu inside serde_json is fixed for a given
/// f64 bit pattern, which is all replay needs).
pub fn canonical<T: Serialize>(v: &T) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, serde_json::ser::CompactFormatter);
    serde_json::ser::SerializeStruct::serialize; // keep import surface obvious
    let value = serde_json::to_value(v).expect("serializable");
    let sorted = sort_value(value);
    serde_json::to_writer(&mut ser, &sorted).expect("serialize");
    String::from_utf8(buf).expect("utf8")
}

fn sort_value(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(mut m) => {
            let mut entries: Vec<_> = m.drain().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut o = serde_json::Map::new();
            for (k, val) in entries {
                o.insert(k, sort_value(val));
            }
            serde_json::Value::Object(o)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(sort_value).collect())
        }
        other => other,
    }
}

pub fn content_id(kind: &str, payload: &str) -> String {
    format!("{kind}_{}", hash16(&[kind, payload]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[test]
    fn ids_are_stable_and_short() {
        let a = hash16(&["run", "p1", "raw|3"]);
        let b = hash16(&["run", "p1", "raw|3"]);
        let c = hash16(&["run", "p1", "raw|4"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn canonical_ignores_key_order() {
        #[derive(Serialize)]
        struct A {
            b: i32,
            a: i32,
        }
        #[derive(Serialize)]
        struct B {
            a: i32,
            b: i32,
        }
        assert_eq!(canonical(&A { a: 1, b: 2 }), canonical(&B { a: 1, b: 2 }));
        assert_eq!(canonical(&A { a: 1, b: 2 }), r#"{"a":1,"b":2}"#);
    }
}
