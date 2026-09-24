//! Clock abstraction: production wall clock vs fixed-seed virtual time.
//!
//! Tests pin a virtual clock so replay is fully deterministic and never
//! depends on the developer machine. Recorded event timestamps carry a
//! `clock` basis (`wall` or `virtual:<seed>`) for provenance.

use chrono::{DateTime, TimeZone, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn basis(&self) -> String;
}

pub struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn basis(&self) -> String {
        "wall".to_string()
    }
}

/// Mutable virtual clock, advanced explicitly. Start at a fixed instant.
pub struct VirtualClock {
    epoch_ms: i64,
    seed: u64,
}

impl VirtualClock {
    pub const START_MS: i64 = 1_700_000_000_000;
    pub fn new(seed: u64) -> Self {
        Self {
            epoch_ms: Self::START_MS + (seed as i64 % 1000),
            seed,
        }
    }
    pub fn advance_secs(&mut self, secs: i64) {
        self.epoch_ms += secs * 1000;
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(self.epoch_ms).unwrap()
    }
    fn basis(&self) -> String {
        format!("virtual:{}", self.seed)
    }
}
