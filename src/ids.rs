//! Stable, content-addressed identities.
//!
//! Every entity (import, arc, candidate, decision, derived result) carries an id
//! that is a hash of its canonical content plus explicit parent references.
//! Because ids depend only on content, export -> delete DB -> re-import preserves
//! identity, ordering and provenance relations.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Canonical JSON: serde_json with sorted map keys is not guaranteed for
/// arbitrary maps, so we serialize via a normalizing helper.
pub fn canonical_json<T: Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serializable");
    normalize(&v)
}

fn normalize(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{:?}:{}", k, normalize(&map[k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(normalize).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).expect("json"),
    }
}

/// Content-addressed id: hex sha256 of canonical content, truncated to 24 chars.
pub fn content_id<T: Serialize>(namespace: &str, value: &T) -> String {
    let mut h = Sha256::new();
    h.update(namespace.as_bytes());
    h.update(b"\x00");
    h.update(canonical_json(value).as_bytes());
    let digest = h.finalize();
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}-{}", namespace, &hex[..24])
}

/// Fingerprint for parameter sets (algorithm version + params).
pub fn param_fingerprint<T: Serialize>(algo_version: &str, params: &T) -> String {
    content_id("fp", &(algo_version, params))
}
