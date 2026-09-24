//! 虚拟时间：所有时间戳由固定起点 + 单调计数器派生，保证可重放。
//!
//! 测试与服务器都使用同一时钟，固定 seed：
//! 起点固定为 2024-06-01T00:00:00Z，每次取号前进 1 秒。

use std::sync::Mutex;

/// 固定种子起点（毫秒，UNIX 纪元）。2024-06-01T00:00:00Z。
pub const SEED_EPOCH_MS: i64 = 1_717_200_000_000;

#[derive(Debug)]
pub struct VClock {
    next_ms: Mutex<i64>,
}

impl Default for VClock {
    fn default() -> Self {
        Self::new()
    }
}

impl VClock {
    pub fn new() -> Self {
        VClock { next_ms: Mutex::new(SEED_EPOCH_MS) }
    }

    /// 取下一个时间戳（RFC3339, UTC），每次前进固定 1 秒。
    pub fn now(&self) -> String {
        let mut g = self.next_ms.lock().unwrap();
        let ms = *g;
        *g += 1000;
        rfc3339_utc(ms)
    }

    pub fn now_ms(&self) -> i64 {
        let mut g = self.next_ms.lock().unwrap();
        let ms = *g;
        *g += 1000;
        ms
    }

    pub fn peek_ms(&self) -> i64 {
        *self.next_ms.lock().unwrap()
    }
}

pub fn rfc3339_utc(unix_ms: i64) -> String {
    let (y, mo, d, hh, mm, ss, milli) = crate::domain::calendar::from_unix_ms(unix_ms);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{milli:03}Z")
}
