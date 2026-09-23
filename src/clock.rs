//! 虚拟时间：测试使用固定 seed 的步进时钟，服务端使用系统时钟。
//! 事件身份不依赖时间，时间只用于展示与排序。

use std::sync::atomic::{AtomicI64, Ordering};

pub trait ClockClock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;
impl ClockClock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// 固定 seed 的虚拟时钟：起点 2026-09-23T00:00:00Z，每次取值步进 7 秒，
/// 保证跨进程重放的事件时间戳完全一致。
pub struct FixedClock {
    tick: AtomicI64,
}
impl FixedClock {
    pub fn seed0_ms() -> i64 {
        1_790_112_000_000 // 2026-09-23T00:00:00Z
    }
    pub fn new() -> Self {
        FixedClock { tick: AtomicI64::new(0) }
    }
}
impl ClockClock for FixedClock {
    fn now_ms(&self) -> i64 {
        let n = self.tick.fetch_add(1, Ordering::SeqCst);
        Self::seed0_ms() + n * 7_000
    }
}
