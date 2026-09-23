//! 稳定身份：FNV-1a 内容哈希。所有展示值的身份由内容决定，
//! 因此导出 / 删除数据库 / 再导入后身份保持不变。

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn hash_hex(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

/// 由若干部分拼接出稳定身份：`prefix_` + 16 位十六进制哈希。
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut buf = String::new();
    for p in parts {
        buf.push_str(p);
        buf.push('\u{1f}');
    }
    format!("{}_{}", prefix, hash_hex(buf.as_bytes()))
}
