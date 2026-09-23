//! 稳定身份：所有逻辑身份都来自内容的 FNV-1a 64 位哈希，
//! 与插入顺序、自增 rowid、墙钟时间无关，因此导出删库再导入后身份不变。

pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn hid(prefix: &str, parts: &[&str]) -> String {
    let mut joined = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            joined.push('\u{1f}');
        }
        joined.push_str(p);
    }
    format!("{}_{:016x}", prefix, fnv1a64(joined.as_bytes()))
}

pub fn content_fp(content: &[u8]) -> String {
    format!("fhv_{:016x}", fnv1a64(content))
}
