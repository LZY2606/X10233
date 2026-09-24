//! 稳定身份与参数指纹：所有实体 ID 由内容哈希 + 命名空间派生，
//! 导出/删除/再导入后保持不变；同一内容重复导入得到同一身份。

use sha2::{Digest, Sha256};

pub fn content_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// 命名空间哈希：ns 区分实体类型，parts 为确定性拼接的字段。
pub fn stable_id(ns: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(ns.as_bytes());
    for p in parts {
        h.update([0u8]);
        h.update(p.as_bytes());
    }
    let digest = hex::encode(h.finalize());
    format!("{}-{}", ns, &digest[..16])
}

/// 参数指纹：JSON 规范化（键排序由调用方保证用 BTreeMap/Value 序列化）后哈希。
pub fn params_fingerprint(params: &serde_json::Value) -> String {
    let canonical = canonicalize(params);
    let bytes = serde_json::to_vec(&canonical).expect("json serialize");
    content_hash(&bytes)[..16].to_string()
}

fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value::*;
    match v {
        Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonicalize(&m[k]));
            }
            Object(out)
        }
        Array(a) => Array(a.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}
