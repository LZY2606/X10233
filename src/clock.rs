//! 虚拟时间：测试注入固定时钟，服务器使用系统时钟。

pub trait Clock: Send + Sync {
    /// 单调事件时间戳（秒）。
    fn now(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// 固定/步进时钟，测试专用。
pub struct FixedClock {
    pub t: std::sync::atomic::AtomicI64,
}

impl FixedClock {
    pub fn new(t: i64) -> Self {
        Self { t: std::sync::atomic::AtomicI64::new(t) }
    }
    pub fn advance(&self, secs: i64) {
        self.t.fetch_add(secs, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.t.load(std::sync::atomic::Ordering::SeqCst)
    }
}
