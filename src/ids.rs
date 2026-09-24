//! 稳定身份：所有领域对象的身份都是内容的确定性哈希，
//! 与数据库行号无关 —— 导出、删除本地库、再导入后身份保持不变。

/// FNV-1a 64 位哈希，十六进制输出。无外部依赖、跨平台确定。
pub fn stable_hash(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0x1f;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    format!("{}:{}", prefix, stable_hash(parts))
}
