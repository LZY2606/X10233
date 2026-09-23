//! Injectable clock. Production uses system time; tests use a fixed virtual
//! clock so every replay is reproducible.
use std::sync::{Arc, Mutex};

pub trait Clock: Send + Sync {
    fn now(&self) -> String;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> String {
        // deterministic-free civil time label; stored as provenance, never used
        // in detection math
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        format!("unix+{}s", d.as_secs())
    }
}

#[derive(Clone)]
pub struct VirtualClock {
    t: Arc<Mutex<i64>>,
}
impl VirtualClock {
    pub fn new(start: i64) -> Self { VirtualClock { t: Arc::new(Mutex::new(start)) } }
    pub fn tick(&self, dt: i64) { let mut g = self.t.lock().unwrap(); *g += dt; }
}
impl Clock for VirtualClock {
    fn now(&self) -> String {
        format!("vt+{}s", *self.t.lock().unwrap())
    }
}
