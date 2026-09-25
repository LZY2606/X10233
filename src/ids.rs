//! 稳定身份：对"自然键"取确定性 SHA-256 前缀。身份不依赖插入自增 id，
//! 因而导出、删除、重新导入后保持不变。

use sha2::{Digest, Sha256};

pub fn stable_id(prefix: &str, material: &str) -> String {
    let mut h = Sha256::new();
    h.update(material.as_bytes());
    let digest = h.finalize();
    let hex: String = digest.iter().take(10).map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

pub fn short_hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}
