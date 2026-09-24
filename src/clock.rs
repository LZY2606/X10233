use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 逻辑时钟：测试用虚拟时间，服务用系统时间，事件时间戳一律取自此。
pub trait Clock: Send + Sync {
    fn now(&self) -> f64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}

pub struct VirtualClock {
    t: Mutex<f64>,
}

impl VirtualClock {
    pub fn new(t: f64) -> Self {
        VirtualClock { t: Mutex::new(t) }
    }
    pub fn advance(&self, dt: f64) {
        let mut g = self.t.lock().unwrap();
        *g += dt;
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> f64 {
        *self.t.lock().unwrap()
    }
}
