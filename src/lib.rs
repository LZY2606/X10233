//! Local-first GNSS carrier-phase cycle-slip review bench.
//!
//! Crate layout:
//! - [`parser`]: ingest the RINEX-OBS-derived epoch text, preserving raw tokens
//! - [`model`]: typed measurement / version data
//! - [`detector`]: geometry-free, Melbourne-Wubbena, time-difference detectors
//! - [`engine`]: arcs, findings, candidates, partial invalidation, event replay
//! - [`store`]: SQLite persistence + export/import
//! - [`api`]: Axum HTTP surface
pub mod api;
pub mod case;
pub mod detector;
pub mod engine;
pub mod hash;
pub mod model;
pub mod parser;
pub mod store;
pub mod vtime;
pub mod webassets;
