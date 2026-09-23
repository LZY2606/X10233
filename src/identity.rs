use serde::Serialize;
use sha2::{Digest, Sha256};

/// 稳定身份：对规范化 JSON 取 SHA-256 前 16 字节。
/// 时间戳、自增 rowid 一律不进入哈希，保证导出-删库-导入后身份不变。
pub fn stable_id<T: Serialize>(prefix: &str, material: &T) -> String {
    let canonical = serde_json::to_string(material).expect("stable id material is serializable");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(prefix.len() + 24);
    out.push_str(prefix);
    for byte in digest.iter().take(12) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 派生量数值在进入身份哈希前统一量化，消除平台浮点尾数差异。
pub fn q9(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        (x * 1e9).round() / 1e9
    }
}

pub fn q6(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        (x * 1e6).round() / 1e6
    }
}
