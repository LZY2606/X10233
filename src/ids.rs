use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    format!("{:x}", h.finalize())
}

pub fn id12(prefix: &str, parts: &[&str]) -> String {
    format!("{}_{}", prefix, &digest(parts)[..12])
}

pub fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", k, canonical(&map[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonical).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

pub fn fingerprint(value: &Value) -> String {
    let d = digest(&[&canonical(value)]);
    d[..16].to_string()
}
