//! Carrier-break: a local-first GNSS cycle-slip candidate review workstation.
//!
//! The crate is layered so the domain engine (`engine`, `detect`, `physics`,
//! `parser`) has no dependency on SQLite or HTTP: it consumes committed events
//! and produces content-addressed, deterministic results. `store` persists the
//! event log to SQLite, `api` exposes it over Axum.

pub mod clock;
pub mod model;
pub mod parser;
pub mod physics;
pub mod detect;
pub mod engine;
pub mod case;
pub mod store;
pub mod api;

pub use engine::{App, Engine};
pub use model::*;
