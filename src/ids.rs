//! Stable, content-derived identifiers. No wall clock, no randomness, so that
//! export -> delete database -> import preserves every identity.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn stable_id(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0x1f]);
        }
        h.update(p.as_bytes());
    }
    hex::encode(&h.finalize()[..8])
}

/// Canonical JSON encoding (object keys sorted, no whitespace) so fingerprints
/// do not depend on field insertion order.
pub fn canonical_json(value: &Value) -> String {
    canonical(value)
}

fn canonical(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap(),
        Value::Array(arr) => {
            let mut out = String::from("[");
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&canonical(v));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = String::from("{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                out.push_str(&canonical(&map[*k]));
            }
            out.push('}');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn id_is_stable_and_distinct() {
        assert_eq!(stable_id(&["a", "b"]), stable_id(&["a", "b"]));
        assert_ne!(stable_id(&["a", "b"]), stable_id(&["a", "c"]));
        assert_eq!(stable_id(&["a", "b"]).len(), 16);
    }

    #[test]
    fn canonical_ignores_key_order() {
        let a = json!({"b": 1, "a": [1, 2]});
        let b = json!({"a": [1, 2], "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }
}
