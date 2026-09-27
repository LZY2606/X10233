pub fn stable_hash(parts: &[String]) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    for part in parts {
        bytes.extend(part.as_bytes());
        bytes.push(0x1f);
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
        hash ^= hash >> 31;
    }
    format!("{hash:016x}")
}

pub fn fingerprint_json(value: &serde_json::Value) -> String {
    stable_hash(&[canonical_json(value)])
}

pub fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Array(values) => {
            let items: Vec<String> = values.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let items: Vec<String> = keys
                .iter()
                .map(|key| format!("{}={}", key, canonical_json(&map[*key])))
                .collect();
            format!("({})", items.join(","))
        }
    }
}

pub fn short_fingerprint(value: &serde_json::Value) -> String {
    fingerprint_json(value).chars().take(10).collect()
}
