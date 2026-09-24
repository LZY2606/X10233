//! Carrier-breakpoint analysis engine.
//!
//! Design constraints (see project brief):
//! * TD (time difference), GF (ionosphere/geometry-free combination) and MW
//!   (Melbourne-Wubbena) each propose their own candidates; proposals may
//!   disagree and **no detector is treated as truth**.
//! * A receiver clock jump and a single-satellite cycle slip are represented
//!   by separate objects.
//! * Repair lives only in an offset layer; raw phase observations are never
//!   rewritten.
//! * Everything is deterministic: stable identities and fixed ordering.

pub mod compute;
pub mod reconcile;

pub use compute::{compute, ClockPoint, DecisionAction, DecisionRecord, PhasePoint, SourceView, StatePoint};

pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;
