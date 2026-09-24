//! 载波断点台 (carrier-breakdesk)
//!
//! Local-first GNSS carrier-phase cycle-slip review workbench.
//! Raw RINEX-like observation epochs and satellite states are imported
//! verbatim; detectors (geometry-free / Melbourne-Wubbena / time-difference)
//! propose cycle-slip candidates without any of them being ground truth;
//! receiver clock jumps are kept separate from single-satellite slips;
//! repairs are stored only as an offset layer. Everything is content
//! addressed so export / delete / re-import preserves identity, order and
//! provenance.

pub mod ids;
pub mod time;
pub mod freq;
pub mod model;
pub mod parse;
pub mod store;
pub mod detect;
pub mod casepack;
pub mod web;
