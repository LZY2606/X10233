//! 稳定身份、固定 seed、虚拟时间。
//!
//! 身份只由“直接来源内容”决定，不依赖墙钟，因此导出/删除本地库/再导入后保持不变，
//! 且局部输入改动只改变依赖它的派生身份。

use sha2::{Digest, Sha256};

/// 计算 `sha256(prefix | bincode-ish(json))` 的前 16 字节十六进制。
pub fn content_id(prefix: &str, material: &str) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update([0u8; 1]);
    h.update(material.as_bytes());
    format!("{:.16x}", format!("{:x}", hex_digest(&mut h)))
}

fn hex_digest(h: &mut Sha256) -> Vec<u8> {
    h.clone().finalize().to_vec()
}

/// 对字符串规范化后求内容 id（用于“直接来源”与参数指纹）。
pub fn json_content_id(prefix: &str, value: &serde_json::Value) -> String {
    let s = canonical_json(value);
    content_id(prefix, &s)
}

/// 保证稳定顺序的 JSON 序列化（键排序、无空格）。
pub fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", json_string(k), canonical_json(&map[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(a) => {
            format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(","))
        }
        other => other.to_string(),
    }
}

fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// 确定性 ID 生成器：固定 seed（测试）或系统熵（生产）。
pub struct IdGen {
    lcg: u64,
    counter: u64,
    tag: String,
}

impl IdGen {
    /// 固定 seed：所有测试用它，得到可复现的事件/版本身份。
    pub fn seeded(seed: u64, tag: &str) -> Self {
        Self { lcg: seed.wrapping_add(0x9E37_79B9_7F4A_7C15), counter: 0, tag: tag.to_string() }
    }

    fn next(&mut self) -> u64 {
        // LCG 常量（Knuth MMIX），确定性。
        self.lcg = self
            .lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.counter += 1;
        self.lcg ^ (self.counter << 32)
    }

    pub fn new_id(&mut self) -> String {
        let n = self.next();
        format!("{}-{:016x}", self.tag, n)
    }
}

/// 虚拟时间：测试固定，生产取墙钟。时间戳绝不参与身份。
pub trait Clock: Send + Sync {
    fn now_rfc3339(&self) -> String;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_rfc3339(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format_gps_epoch(secs)
    }
}

/// 固定时钟：从某个 epoch 秒起步，可在测试中手动推进。
pub struct FixedClock {
    secs: std::sync::Mutex<u64>,
}

impl FixedClock {
    pub fn new(epoch_secs: u64) -> Self {
        Self { secs: std::sync::Mutex::new(epoch_secs) }
    }
    pub fn advance(&self, secs: u64) {
        *self.secs.lock().unwrap() += secs;
    }
}

impl Clock for FixedClock {
    fn now_rfc3339(&self) -> String {
        format_gps_epoch(*self.secs.lock().unwrap())
    }
}

fn format_gps_epoch(secs: u64) -> String {
    // 简化的 UTC RFC3339（1970 epoch），仅用于展示。
    let (y, mo, d, hh, mm, ss) = civil_from_unix(secs as i64);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, hh, mm, ss)
}

fn civil_from_unix(mut z: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs_per_day = 86400i64;
    let days = z.div_euclid(secs_per_day);
    z -= days * secs_per_day;
    let hh = (z / 3600) as u32;
    let mm = ((z % 3600) / 60) as u32;
    let ss = (z % 60) as u32;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_ids_are_deterministic() {
        let mut a = IdGen::seeded(42, "evt");
        let mut b = IdGen::seeded(42, "evt");
        assert_eq!(a.new_id(), b.new_id());
        assert_ne!(a.new_id(), b.new_id());
    }

    #[test]
    fn content_ids_stable_and_order_independent() {
        let v1 = serde_json::json!({"b": 1, "a": [1, 2]});
        let v2 = serde_json::json!({"a": [1, 2], "b": 1});
        assert_eq!(json_content_id("x", &v1), json_content_id("x", &v2));
        let v3 = serde_json::json!({"b": 1, "a": [1, 3]});
        assert_ne!(json_content_id("x", &v1), json_content_id("x", &v3));
    }
}
