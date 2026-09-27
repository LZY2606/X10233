//! Append-only event log.
//!
//! Raw inputs are never overwritten. Every change is a new event with an
//! explicit parent (seq-1 on the same branch), an algorithm version and a
//! parameter fingerprint. Branches fork at an event and carry their own linear
//! history, so returning to any old event restores that dependency state.

use serde::{Deserialize, Serialize};

use crate::raw_model::{EpochRecord, ImportReport};

/// Algorithm version of the derived layer. Bump when detector math changes.
pub const ALGO_VERSION: &str = "mw-ion-td-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ProjectCreated,
    RawCommitted,
    ParamsCommitted,
    Decision,
    BranchCreated,
}

/// Tunable detector parameters. Fingerprinted so derived results record the
/// exact parameter set that produced them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// Expected sampling interval in seconds. Gaps larger than this start a gap.
    pub nominal_interval_s: f64,
    /// Multiple of nominal interval tolerated before cutting a gap boundary.
    pub gap_tolerance_factor: f64,
    /// TD-phase residual threshold in cycles.
    pub td_threshold_cycles: f64,
    /// Ionosphere (geometry-free) residual threshold in metres.
    pub ion_threshold_m: f64,
    /// Melbourne-Wubbena residual threshold in wide-lane cycles.
    pub mw_threshold_wl_cycles: f64,
    /// Minimum samples before MW running mean is trusted.
    pub mw_min_history: usize,
    /// Half of this value bounds the integer search; equal ties are all kept.
    pub integer_search_half_width: i64,
    /// Threshold above which a consistent across-all-sat jump is a clock jump.
    pub clock_consensus_fraction: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            nominal_interval_s: 30.0,
            gap_tolerance_factor: 1.5,
            td_threshold_cycles: 0.25,
            ion_threshold_m: 0.05,
            mw_threshold_wl_cycles: 0.5,
            mw_min_history: 2,
            integer_search_half_width: 20,
            clock_consensus_fraction: 0.8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    ProjectCreated {
        project_id: String,
        name: String,
        case_id: Option<String>,
    },
    RawCommitted {
        batch_id: String,
        parent_batch_id: Option<String>,
        label: String,
        /// Human-readable narrative step for the counter-example archive.
        note: String,
        epochs: Vec<EpochRecord>,
        report: ImportReport,
    },
    ParamsCommitted {
        params: DetectorParams,
        reason: String,
    },
    Decision {
        /// Arc/boundary this decision adjudicates, e.g. "arc:STA1|G07|L1".
        subject_id: String,
        /// Boundary id when the decision is about a specific boundary.
        boundary_id: Option<String>,
        action: DecisionAction,
        /// Chosen integer offset per band (repair lives only on the offset layer).
        chosen_offsets: Vec<(String, i64)>,
        rationale: String,
    },
    BranchCreated {
        branch_id: String,
        forked_from_branch: String,
        forked_at_seq: i64,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    Accept,
    Reject,
    /// Conflict resolution: supersede a prior decision.
    Supersede,
    /// Parameter adjustment request is recorded via ParamsCommitted instead;
    /// this marks that a boundary needs recomputation after a param change.
    Recompute,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    /// Globally stable identifier, preserved across export/import.
    pub event_id: String,
    pub seq: i64,
    pub branch: String,
    /// seq of the parent event on this branch (-1 for branch root / create).
    pub parent_seq: i64,
    /// Virtual clock timestamp (fixed seed), seconds.
    pub vtime: f64,
    pub kind: EventKind,
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    pub fn params_of(&self) -> Option<&DetectorParams> {
        match &self.payload {
            EventPayload::ParamsCommitted { params, .. } => Some(params),
            _ => None,
        }
    }
}

/// FNV-1a 64-bit fingerprint of the canonical JSON of a value. Stable across
/// processes and platforms (operates on UTF-8 bytes, not in-memory layout).
pub fn fingerprint<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("fingerprint serialize");
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fp-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_order_sensitive() {
        let a = fingerprint(&vec![1, 2, 3]);
        let b = fingerprint(&vec![1, 2, 3]);
        let c = fingerprint(&vec![3, 2, 1]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("fp-"));
    }
}
