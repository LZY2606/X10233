use sha2::{Digest, Sha256};

/// 稳定身份：对规范内容做 SHA-256，取前 16 个十六进制字符，加种类前缀。
/// 同一内容在任何机器、任何时刻得到同一身份；导出/导入后身份不变。
pub fn make_id(kind: &str, canonical_payload: &str) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"|");
    h.update(canonical_payload.as_bytes());
    let hex = format!("{:x}", h.finalize());
    format!("{}:{}", kind, &hex[..16])
}

/// 参数指纹：算法参数的规范 JSON 摘要。
pub fn fingerprint(payload: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"fp|");
    h.update(payload.as_bytes());
    let hex = format!("{:x}", h.finalize());
    hex[..12].to_string()
}
