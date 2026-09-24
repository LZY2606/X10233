//! 虚拟时间基准：单调递增的逻辑时钟，保证事件序号稳定且可重放。
//! 不使用系统真实时间，测试与演示均通过注入的 Clock 推进。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Tick(pub u64);

impl Tick {
    pub fn as_iso(self) -> String {
        // 虚拟时间轴：以 2020-01-01T00:00:00Z 为原点的确定性秒偏移，仅用于展示。
        let secs = self.0;
        let days = secs / 86400;
        let rem = secs % 86400;
        let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
        // 简化的民用历换算（2020-01-01 起算，不考虑闰秒；确定性即可）。
        let (mut y, mut d) = (2020i64, days as i64);
        loop {
            let yl = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
            if d < yl { break; }
            d -= yl;
            y += 1;
        }
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut mo = 0usize;
        let mut dd = d;
        for (i, &md) in mdays.iter().enumerate() {
            if dd < md as i64 { mo = i; break; }
            dd -= md as i64;
        }
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo + 1, dd + 1, h, m, s)
    }
}

pub struct Clock {
    next: AtomicU64,
}

impl Clock {
    pub fn new(start: u64) -> Self {
        Self { next: AtomicU64::new(start) }
    }
    pub fn tick(&self) -> Tick {
        Tick(self.next.fetch_add(1, Ordering::SeqCst))
    }
    pub fn now(&self) -> u64 {
        self.next.load(Ordering::SeqCst)
    }
}

/// 每连接一个时钟（SQLite 连接不跨线程共享，故用 Mutex 包裹在 store 层）。
pub struct ClockedDb {
    pub clock: Mutex<Clock>,
}

impl ClockedDb {
    pub fn new(start: u64) -> Self {
        Self { clock: Mutex::new(Clock::new(start)) }
    }
    pub fn tick(&self) -> Tick {
        self.clock.lock().unwrap().tick()
    }
}
