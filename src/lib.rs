//! Local-first GNSS carrier-phase cycle-slip review workbench.
//!
//! Layers:
//! - raw layer: imported observations are stored verbatim (value as text + unit),
//!   never overwritten; revisions are new versions linked by parent ids.
//! - derived layer: arcs, combination residuals, detector candidates. Every
//!   derived value carries algorithm version, parameter fingerprint and direct
//!   source ids. Local input changes only stale the dependent derived results.
//! - adjudication layer: human decisions are events on branches; repairs are
//!   stored only as an offset layer, never by mutating observations.

pub mod archive;
pub mod casepack;
pub mod detect;
pub mod engine;
pub mod ids;
pub mod model;
pub mod rinex;
pub mod store;

pub const ALGO_VERSION: &str = "1.0.0";
pub const ARCHIVE_FORMAT: &str = "gnss-slip-archive/1";
