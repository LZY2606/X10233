//! Append-only event log. Raw inputs are never overwritten: corrections are
//! new batches with a parent batch; revisions are parameter events with a
//! parent revision; decisions fork a new head.

use crate::model::{ClockEvent, DecisionRecord, DetectorParams, Id, Observation, SatState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    ProjectCreated,
    RawBatch,
    ParamRevision,
    Decision,
    Conflict,
    ConflictResolved,
    Undo,
}

/// A submitted raw batch. A later correction keeps the same logical
/// observation id only when the raw key is unchanged; a corrected value is a
/// new observation id whose `supersedes` names the prior one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBatch {
    pub batch_id: Id,
    pub parent_batch_id: Option<Id>,
    pub source_name: String,
    /// RINEX / sheet fragment text actually submitted (audit).
    pub input_fragment: String,
    pub observations: Vec<Observation>,
    pub sat_states: Vec<SatState>,
    pub declared_clock_events: Vec<ClockEvent>,
    /// Format-level errors that prevented some rows from converting. The
    /// rejected fragment is retained.
    pub format_errors: Vec<FormatError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatError {
    pub line_no: Option<usize>,
    pub raw_line: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventPayload {
    ProjectCreated { name: String },
    RawBatch(RawBatch),
    ParamRevision { parent_fp: String, params: DetectorParams, note: String },
    Decision(DecisionRecord),
    /// A conflict between two human decisions on the same candidate.
    Conflict { candidate_id: Id, prior_event: Id, detail: String },
    /// Resolution of a conflict, selecting one event's position.
    ConflictResolved { candidate_id: Id, winning_event: Id, note: String },
    /// Logical undo of an earlier event (the event stays in the log).
    Undo { target_event: Id, note: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Id,
    pub seq: i64,
    pub kind: EventKind,
    pub ts_ms: i64,
    pub project_id: Id,
    pub parent_event_id: Option<Id>,
    pub payload: EventPayload,
}

impl Event {
    pub fn new_id(seq: i64, project_id: &Id, ts_ms: i64, payload_key: &str) -> Id {
        Id(format!(
            "ev{}",
            crate::model::stable_id(
                "event",
                &format!("{}|{}|{}|{}", project_id, seq, ts_ms, payload_key)
            )
        ))
    }
}
