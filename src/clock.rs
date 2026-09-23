use std::sync::Mutex;

/// 逻辑时钟, 单位毫秒。测试用虚拟时间, 服务用系统时间。
pub trait Clock: Send + Sync {
    fn now_tick(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_tick(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// 虚拟时钟: 固定起点, 只能显式推进, 保证测试可复现。
pub struct VirtualClock {
    tick: Mutex<i64>,
}

impl VirtualClock {
    pub fn new(start: i64) -> Self {
        Self { tick: Mutex::new(start) }
    }
    pub fn advance(&self, delta_ms: i64) {
        let mut t = self.tick.lock().unwrap();
        *t += delta_ms;
    }
}

impl Clock for VirtualClock {
    fn now_tick(&self) -> i64 {
        *self.tick.lock().unwrap()
    }
}
