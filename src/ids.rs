use sha2::{Digest, Sha256};

/// 内容寻址稳定身份：同一命名空间 + 同一内容 => 同一 ID。
/// 导出/删除数据库/再导入后身份保持不变。
pub fn cid(ns: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(ns.as_bytes());
    for p in parts {
        h.update([0x1f]);
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    let mut s = String::with_capacity(32);
    for b in &d[..12] {
        s.push_str(&format!("{:02x}", b));
    }
    format!("{}_{}", ns, s)
}
