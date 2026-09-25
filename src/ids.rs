//! 稳定身份：所有身份都由内容派生（FNV-1a 64 位，截断显示）。
//! 这样导出 -> 删除本地库 -> 再导入后身份、顺序与来源关系保持不变。

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_b325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn content_id(prefix: &str, material: &str) -> String {
    format!("{prefix}_{:016x}", fnv1a64(material.as_bytes()))
}

/// 多段材料拼接的身份（用 0x1f/0x1e 作分隔，避免字段碰撞）。
pub fn joined_id(prefix: &str, parts: &[&str]) -> String {
    let mut m = String::new();
    for p in parts {
        m.push('\u{1f}');
        m.push_str(p);
        m.push('\u{1e}');
    }
    content_id(prefix, &m)
}
