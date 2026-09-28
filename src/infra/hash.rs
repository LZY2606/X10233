//! 稳定哈希：身份（候选、弧段、提交、事件）全部由内容决定，导入导出后保持不变。

use sha2::{Digest, Sha256};

pub fn short_hash(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let d = h.finalize();
    hex::encode(&d[..10])
}
