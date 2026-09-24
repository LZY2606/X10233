//! 稳定身份：所有实体 id 均为内容寻址哈希，导出/删除/再导入后保持不变。
use sha2::{Digest, Sha256};

pub fn hash_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    for p in parts {
        h.update([0x1f]);
        h.update(p.as_bytes());
    }
    let digest = h.finalize();
    let mut s = String::with_capacity(prefix.len() + 17);
    s.push_str(prefix);
    s.push('_');
    for b in &digest[..8] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
