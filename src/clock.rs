//! 可注入时钟：服务运行用系统时钟，测试用虚拟时钟（固定、可步进）。

use chrono::{DateTime, TimeZone, Utc};

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
    fn now_iso(&self) -> String {
        let ms = self.now_ms();
        let dt: DateTime<Utc> = Utc.timestamp_millis_opt(ms).single().unwrap_or_else(Utc::now);
        dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }
}

#[derive(Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
}

/// 虚拟时钟：固定起点，可显式步进，测试专用（不依赖真实时间）。
pub struct VirtualClock {
    cur: std::sync::Mutex<i64>,
}
impl VirtualClock {
    pub fn new(start_ms: i64) -> Self {
        Self { cur: std::sync::Mutex::new(start_ms) }
    }
    pub fn advance_ms(&self, delta: i64) {
        let mut g = self.cur.lock().unwrap();
        *g += delta;
    }
}
impl Clock for VirtualClock {
    fn now_ms(&self) -> i64 {
        *self.cur.lock().unwrap()
    }
}
