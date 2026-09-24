//! Carrier Break Desk — local-first GNSS carrier-phase cycle-slip review.
//!
//! Crate layout:
//! - [`domain`]   core data types (observations, runs, arcs, candidates, events)
//! - [`units`]    unit-tagged values with retained raw representation
//! - [`freq`]     observation-code / band frequency tables
//! - [`obx`]      small reproducible OBX text sample format + strict parser
//! - [`detect`]   ionosphere / Melbourne-Wubbena / time-difference detectors,
//!                clock jumps, arc segmentation
//! - [`store`]    SQLite event-sourced store with deterministic reducer
//! - [`api`]      axum HTTP application (JSON API + static web UI)

pub mod api;
pub mod detect;
pub mod domain;
pub mod freq;
pub mod obx;
pub mod store;
pub mod units;

pub use domain::*;
