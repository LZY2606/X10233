//! SQLite persistence.
//!
//! Tables mirror the layering rules:
//! * `raw_*` rows are immutable source material (new batches append; the
//!   same natural key in a newer batch supersedes the old *view*, never the
//!   row).
//! * `events` form a hash-linked tree. Each event points at its parent, so
//!   checking out an old event replays exactly that branch; a human decision
//!   on an old event forks the tree.
//! * `derived_*` snapshot rows carry their viewpoint, algorithm version and
//!   parameter fingerprint; `materialized_view` states which rows are current
//!   at the tip and which are stale (viewable, never presented as current).

pub mod schema;
pub mod project;
pub mod import;
pub mod events;
pub mod derived;
pub mod bundle;

pub use project::Store;
pub use events::{EventBody, EventRecord};
