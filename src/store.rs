//! Application service: commits events, replays projections and caches
//! derived views. This is the single transactional entry point used by both
//! the HTTP layer and the tests.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};

use crate::clock::Clock;
use crate::db;
use crate::domain::state::ProjectState;
use crate::event::{Event, EventKind, EventPayload, RawBatch};
use crate::ingest::IngestOutcome;
use crate::model::{
    Candidate, CandidateStatus, DecisionKind, DecisionRecord, DetectorParams, Id,
};

pub struct Store {
    conn: Mutex<Connection>,
    clock: std::sync::Arc<dyn Clock>,
}

pub struct HeadView {
    pub state: ProjectState,
    pub view: crate::domain::state::DerivedView,
    pub at_seq: i64,
    pub event_id: Id,
}

pub struct HistoryView {
    pub state: ProjectState,
    pub view: crate::domain::state::DerivedView,
    pub at_seq: i64,
    pub event_id: Id,
}

impl Store {
    pub fn open(path: &Path, clock: std::sync::Arc<dyn Clock>) -> rusqlite::Result<Self> {
        let conn = db::open(path)?;
        Ok(Store { conn: Mutex::new(conn), clock })
    }

    pub fn open_memory(clock: std::sync::Arc<dyn Clock>) -> rusqlite::Result<Self> {
        let conn = db::open_in_memory()?;
        Ok(Store { conn: Mutex::new(conn), clock })
    }

    pub fn create_project(&self, name: &str) -> rusqlite::Result<Id> {
        let conn = self.conn.lock().unwrap();
        let id = crate::model::stable_id("project", &format!("{}|{}", name, self.clock.now_ms()));
        let ts = self.clock.now_ms();
        db::create_project(&conn, &id, name, ts)?;
        let seq = db::next_seq(&conn, &id)?;
        let payload_key = format!("create|{name}");
        let ev_id = Event::new_id(seq, &id, ts, &payload_key);
        let ev = Event {
            id: ev_id.clone(),
            seq,
            kind: EventKind::ProjectCreated,
            ts_ms: ts,
            project_id: id.clone(),
            parent_event_id: None,
            payload: EventPayload::ProjectCreated { name: name.to_string() },
        };
        db::append_event(&conn, &ev)?;
        Ok(id)
    }

    pub fn list_projects(&self) -> rusqlite::Result<Vec<(Id, String)>> {
        let conn = self.conn.lock().unwrap();
        db::list_projects(&conn)
    }

    /// Commit a parsed raw batch as a new immutable event.
    pub fn commit_batch(
        &self,
        project_id: &Id,
        outcome: IngestOutcome,
    ) -> rusqlite::Result<(Id, RawBatch)> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let parent = db::head_seq(&tx, project_id).ok();
        let _ = parent;
        let seq = db::next_seq(&tx, project_id)?;
        let ts = self.clock.now_ms();
        let batch = outcome.batch;
        let key = format!("rawbatch|{}|{}", batch.source_name, batch.batch_id);
        let ev_id = Event::new_id(seq, project_id, ts, &key);
        let parent_event = head_event_id(&tx, project_id)?;
        let ev = Event {
            id: ev_id.clone(),
            seq,
            kind: EventKind::RawBatch,
            ts_ms: ts,
            project_id: project_id.clone(),
            parent_event_id: parent_event,
            payload: EventPayload::RawBatch(batch.clone()),
        };
        db::append_event(&tx, &ev)?;
        tx.commit()?;
        Ok((ev_id, batch))
    }

    /// Append a parameter revision (new immutable params version).
    pub fn revise_params(
        &self,
        project_id: &Id,
        params: DetectorParams,
        note: &str,
    ) -> rusqlite::Result<Id> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let state = replay(&tx, project_id, None)?;
        let parent_fp = state.params.fingerprint();
        if params == state.params {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let seq = db::next_seq(&tx, project_id)?;
        let ts = self.clock.now_ms();
        let key = format!("params|{}", params.fingerprint());
        let ev_id = Event::new_id(seq, project_id, ts, &key);
        let parent_event = head_event_id(&tx, project_id)?;
        let ev = Event {
            id: ev_id.clone(),
            seq,
            kind: EventKind::ParamRevision,
            ts_ms: ts,
            project_id: project_id.clone(),
            parent_event_id: parent_event,
            payload: EventPayload::ParamRevision {
                parent_fp,
                params,
                note: note.to_string(),
            },
        };
        db::append_event(&tx, &ev)?;
        tx.commit()?;
        Ok(ev_id)
    }

    /// Record a human decision. Conflicting decisions on the same candidate
    /// produce an explicit Conflict event; neither silently overwrites the
    /// other.
    pub fn decide(
        &self,
        project_id: &Id,
        candidate_id: &Id,
        kind: DecisionKind,
        shift_b1: Option<i64>,
        shift_b2: Option<i64>,
        note: &str,
    ) -> rusqlite::Result<Id> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let state = replay(&tx, project_id, None)?;
        let ckey = candidate_id.0.clone();
        let existing = state
            .decisions
            .iter()
            .find(|(d, _)| d.candidate_id.as_ref().map(|i| i.0 == ckey).unwrap_or(false))
            .map(|(_, eid)| eid.clone());
        let seq = db::next_seq(&tx, project_id)?;
        let ts = self.clock.now_ms();
        let parent_event = head_event_id(&tx, project_id)?;
        let rec = DecisionRecord {
            kind,
            candidate_id: Some(candidate_id.clone()),
            shift_b1_cycles: shift_b1,
            shift_b2_cycles: shift_b2,
            note: note.to_string(),
            undoes_event: None,
        };
        let key = format!("decision|{ckey}|{:?}", kind);
        let ev_id = Event::new_id(seq, project_id, ts, &key);

        // conflict if a prior decision disagrees and is not itself undone
        let conflicting = match kind {
            DecisionKind::Accept | DecisionKind::Reject => {
                if let Some(prior) = &existing {
                    if !state.undone.contains(prior) {
                        let prior_accepted =
                            state.accepted.contains_key(&ckey) && kind == DecisionKind::Reject;
                        let prior_rejected =
                            state.rejected.contains(&ckey) && kind == DecisionKind::Accept;
                        let prior_shift = state.accepted.get(&ckey);
                        let shifted = matches!(kind, DecisionKind::Accept)
                            && prior_shift.map(|p| {
                                p.shift_b1 != shift_b1 || p.shift_b2 != shift_b2
                            })
                            .unwrap_or(false);
                        Some((prior.clone(), prior_accepted || prior_rejected || shifted))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        db::append_event(
            &tx,
            &Event {
                id: ev_id.clone(),
                seq,
                kind: EventKind::Decision,
                ts_ms: ts,
                project_id: project_id.clone(),
                parent_event_id: parent_event.clone(),
                payload: EventPayload::Decision(rec),
            },
        )?;
        if let Some((prior, true)) = conflicting {
            let cseq = db::next_seq(&tx, project_id)?;
            let cts = self.clock.now_ms();
            let cid = Event::new_id(
                cseq,
                project_id,
                cts,
                &format!("conflict|{ckey}|{}", ev_id.0),
            );
            db::append_event(
                &tx,
                &Event {
                    id: cid,
                    seq: cseq,
                    kind: EventKind::Conflict,
                    ts_ms: cts,
                    project_id: project_id.clone(),
                    parent_event_id: Some(ev_id.clone()),
                    payload: EventPayload::Conflict {
                        candidate_id: candidate_id.clone(),
                        prior_event: prior,
                        detail: format!("human decisions disagree ({note})"),
                    },
                },
            )?;
        }
        tx.commit()?;
        Ok(ev_id)
    }

    /// Resolve a conflict by selecting the winning decision.
    pub fn resolve_conflict(
        &self,
        project_id: &Id,
        candidate_id: &Id,
        winning_event: &Id,
        note: &str,
    ) -> rusqlite::Result<Id> {
        self.append_simple(
            project_id,
            EventKind::ConflictResolved,
            EventPayload::ConflictResolved {
                candidate_id: candidate_id.clone(),
                winning_event: winning_event.clone(),
                note: note.to_string(),
            },
        )
    }

    /// Logical undo: the target event remains in the log but is rendered
    /// ineffective; the undo itself is a new event.
    pub fn undo(&self, project_id: &Id, target_event: &Id, note: &str) -> rusqlite::Result<Id> {
        self.append_simple(
            project_id,
            EventKind::Undo,
            EventPayload::Undo {
                target_event: target_event.clone(),
                note: note.to_string(),
            },
        )
    }

    fn append_simple(
        &self,
        project_id: &Id,
        kind: EventKind,
        payload: EventPayload,
    ) -> rusqlite::Result<Id> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let seq = db::next_seq(&tx, project_id)?;
        let ts = self.clock.now_ms();
        let key = serde_json::to_string(&payload).unwrap();
        let ev_id = Event::new_id(seq, project_id, ts, &key);
        let parent_event = head_event_id(&tx, project_id)?;
        db::append_event(
            &tx,
            &Event {
                id: ev_id.clone(),
                seq,
                kind,
                ts_ms: ts,
                project_id: project_id.clone(),
                parent_event_id: parent_event,
                payload,
            },
        )?;
        tx.commit()?;
        Ok(ev_id)
    }

    /// Current head projection + derived view.
    pub fn head(&self, project_id: &Id) -> rusqlite::Result<HeadView> {
        let conn = self.conn.lock().unwrap();
        let at_seq = db::head_seq(&conn, project_id)?;
        let state = replay(&conn, project_id, Some(at_seq))?;
        let event_id =
            head_event_id(&conn, project_id)?.unwrap_or_else(|| Id("project".into()));
        let view = state.derive_view();
        Ok(HeadView { state, view, at_seq, event_id })
    }

    /// Replay to (and including) any old event. Historical conclusions are
    /// computed from the then-current dependency versions, never overwritten
    /// by the newest results.
    pub fn at_event(&self, project_id: &Id, event_seq: i64) -> rusqlite::Result<HistoryView> {
        let conn = self.conn.lock().unwrap();
        let state = replay(&conn, project_id, Some(event_seq))?;
        let event_id = Id(format!("seq-{event_seq}"));
        let view = state.derive_view();
        Ok(HistoryView { state, view, at_seq: event_seq, event_id })
    }

    pub fn events(&self, project_id: &Id) -> rusqlite::Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        db::load_events(&conn, project_id)
    }

    pub fn export(&self, project_id: &Id) -> rusqlite::Result<String> {
        let conn = self.conn.lock().unwrap();
        db::export_project(&conn, project_id)
    }

    pub fn import_bundle(&self, bundle: &str) -> rusqlite::Result<Id> {
        let conn = self.conn.lock().unwrap();
        db::import_bundle(&conn, bundle)
    }
}

fn head_event_id(conn: &Connection, project_id: &Id) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT id FROM events WHERE project_id = ?1 ORDER BY seq DESC LIMIT 1",
        rusqlite::params![project_id.0],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map(|o| o.map(Id))
}

/// Replay events into a fresh projection. `up_to` truncates for history.
pub fn replay(
    conn: &Connection,
    project_id: &Id,
    up_to: Option<i64>,
) -> rusqlite::Result<ProjectState> {
    let events = db::load_events_until(conn, project_id, up_to)?;
    let mut state = ProjectState::new(project_id.clone(), String::new());
    for ev in events {
        state.apply(&ev);
    }
    Ok(state)
}

/// Helper for tests/UI: current status of a candidate at head.
pub fn candidate_at_head<'a>(
    view: &'a crate::domain::state::DerivedView,
    cid: &Id,
) -> Option<&'a Candidate> {
    view.candidates.iter().find(|c| &c.id == cid)
}

pub fn status_label(s: CandidateStatus) -> &'static str {
    match s {
        CandidateStatus::Proposed => "proposed",
        CandidateStatus::Accepted => "accepted",
        CandidateStatus::Rejected => "rejected",
        CandidateStatus::Ambiguous => "ambiguous",
        CandidateStatus::Conflict => "conflict",
        CandidateStatus::Unknown => "unknown",
    }
}
