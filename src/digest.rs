//! 确定性身份：FNV-1a 64bit 内容指纹（无随机种子，无外部依赖）。
//!
//! 所有显示值（源版本、规则版本、运行、候选、决定…）的稳定身份均由其
//! 规范化内容派生；导出删除数据库后再导入，身份保持不变。

pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[derive(Default)]
pub struct Hasher {
    h: u64,
}

impl Hasher {
    pub fn new() -> Self {
        Hasher { h: 0xcbf2_9ce4_8422_2325 }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            self.h ^= b as u64;
            self.h = self.h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.feed(&(s.len() as u64).to_le_bytes());
        self.feed(s.as_bytes());
        self.feed(&[0x1f]);
        self
    }
    pub fn int(&mut self, i: i64) -> &mut Self {
        self.feed(&i.to_le_bytes())
    }
    pub fn opt_f64(&mut self, v: Option<f64>) -> &mut Self {
        match v {
            None => self.feed(&[0x00]),
            Some(x) => {
                self.feed(&[0x01]);
                self.feed(&x.to_bits().to_le_bytes());
            }
        }
    }
    pub fn finish(&self) -> String {
        format!("{:016x}", self.h)
    }
}

pub fn id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Hasher::new();
    h.str(prefix);
    for p in parts {
        h.str(p);
    }
    format!("{prefix}_{}", h.finish())
}
