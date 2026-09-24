//! 确定性稳定身份。
//!
//! 所有领域对象都从其自然键经 FNV-1a(64) 派生 `prefix-hex` 形式的身份，
//! 因此导出、删库再导入后身份不变（不依赖自增 rowid）。

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001b3;

pub fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h = FNV_OFFSET;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            h ^= 0x1f;
            h = h.wrapping_mul(FNV_PRIME);
        }
        for b in part.iter() {
            h ^= *b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

pub fn id(prefix: &str, parts: &[&[u8]]) -> String {
    format!("{}-{:016x}", prefix, fnv1a(parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_distinct() {
        let a = id("obs", &[b"p1", b"G07", b"2026-01-01T00:00:00Z", b"L1W"]);
        let b = id("obs", &[b"p1", b"G07", b"2026-01-01T00:00:00Z", b"L1W"]);
        let c = id("obs", &[b"p1", b"G07", b"2026-01-01T00:00:00Z", b"L2W"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("obs-"));
    }
}
