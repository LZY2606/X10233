//! 虚拟时间：生产用系统时钟，测试用固定 seed 的虚拟时钟。
//! 所有历史事件时间戳均由此产生，测试不依赖机器当前时间。

use std::sync::{Arc, Mutex};

pub trait Clock: Send + Sync {
    /// RFC3339（UTC）时间戳。
    fn now_rfc3339(&self) -> String;
    /// 单调毫秒（测试中为确定性虚拟毫秒）。
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_rfc3339(&self) -> String {
        // 手写 RFC3339，避免额外时间库依赖；使用 UNIX epoch 转换。
        unix_to_rfc3339(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0))
    }

    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// 固定 seed 的虚拟时钟：从 seed 毫秒起步，每次取号前进 1ms，保证全流程确定。
#[derive(Clone)]
pub struct VirtualClock {
    ms: Arc<Mutex<i64>>,
}

impl VirtualClock {
    pub fn seeded(seed_ms: i64) -> Self {
        VirtualClock {
            ms: Arc::new(Mutex::new(seed_ms)),
        }
    }
}

impl Clock for VirtualClock {
    fn now_rfc3339(&self) -> String {
        unix_to_rfc3339(self.now_ms() / 1000)
    }

    fn now_ms(&self) -> i64 {
        let mut g = self.ms.lock().unwrap();
        let v = *g;
        *g += 1;
        v
    }
}

/// 将 `YYYY-MM-DDTHH:MM:SSZ` 转换为相对 UNIX epoch 的秒数（简化 Howard Hinnant 算法）。
pub fn rfc3339_to_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let mon: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;
    if !(1..=12).contains(&mon) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if mon <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if mon > 2 { mon - 3 } else { mon + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

/// UNIX 秒转 `YYYY-MM-DDTHH:MM:SSZ`。
pub fn unix_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        m,
        d,
        sod / 3600,
        sod / 60 % 60,
        sod % 60
    )
}

/// 历元令牌转“相对参考历元的秒偏移”。支持两种确定性格式：
/// - 绝对 RFC3339：`2026-01-01T00:00:30Z`
/// - 显式秒偏移：`+30s`
pub fn epoch_to_offset(tag: &str, reference: &str) -> Option<f64> {
    let t = tag.trim();
    if let Some(rest) = t.strip_prefix('+').or_else(|| t.strip_prefix('-')) {
        // 符号已在原值中，解析数值部分。
        let neg = t.starts_with('-');
        let num = rest.strip_suffix('s').unwrap_or(rest);
        let v: f64 = num.parse().ok()?;
        Some(if neg { -v } else { v })
    } else {
        let e = rfc3339_to_unix(t)? as f64;
        let r = rfc3339_to_unix(reference)? as f64;
        Some(e - r)
    }
}
