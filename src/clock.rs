//! 虚拟时间：测试与重放都使用确定性时间，绝不读取系统墙钟。

#[derive(Debug, Clone)]
pub struct VirtualClock {
    now_s: f64,
    step_s: f64,
}

impl VirtualClock {
    pub const EPOCH: f64 = 1_780_000_000.0;

    pub fn start_at(now_s: f64, step_s: f64) -> Self {
        Self { now_s, step_s }
    }

    pub fn fixed(now_s: f64) -> Self {
        Self { now_s, step_s: 0.0 }
    }

    pub fn now(&self) -> f64 {
        self.now_s
    }

    pub fn advance(&mut self) -> f64 {
        self.now_s += self.step_s;
        self.now_s
    }

    pub fn seek(&mut self, now_s: f64) {
        self.now_s = now_s;
    }
}
