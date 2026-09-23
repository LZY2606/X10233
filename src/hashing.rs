use serde_json::Value;
use sha2::{Digest, Sha256};

/// Recursively canonicalize a JSON value: object keys sorted, no whitespace.
/// Content-addressed identities therefore do not depend on field order or
/// formatting of the imported/exported representation.
pub fn canonical(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => out.extend_from_slice(serde_json::to_string(s).unwrap().as_bytes()),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(serde_json::to_string(key).unwrap().as_bytes());
                out.push(b':');
                write_canonical(&map[*key], out);
            }
            out.push(b'}');
        }
    }
}

/// Stable, content-addressed identifier: `<kind>:<16 hex chars of sha256>`.
pub fn content_id(kind: &str, value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(b"|");
    hasher.update(canonical(value));
    format!("{}:{}", kind, hex::encode(hasher.finalize()).split_at(16).0)
}
