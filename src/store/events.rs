use super::project::PROJECT_ID;
use super::Store;
use crate::engine::DecisionRecord;
use crate::ids;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventBody {
    Import {
        batch_id: String,
        seq: i64,
        label: String,
        raw_sha: String,
        observations: i64,
        diagnostics: i64,
    },
    Params {
        params: crate::params::DetectorParams,
        fingerprint: String,
        reason: String,
    },
    Decision {
        action: String,
        candidate_id: Option<String>,
        sat: Option<String>,
        band: Option<String>,
        at_epoch: Option<String>,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub branch: String,
    pub seq: i64,
    pub kind: String,
    pub body: EventBody,
    pub created_at_ms: i64,
    pub batch_id: Option<String>,
    pub param_fp: Option<String>,
}

impl Store {
    pub fn append_event(
        &self,
        kind: &str,
        body: &EventBody,
        batch_id: Option<String>,
        param_fp: Option<String>,
    ) -> EventRecord {
        self.append_event_on_branch("main", kind, body, batch_id, param_fp, None)
    }

    /// Append on a branch. `parent_override` forks from an arbitrary old
    /// event; otherwise the branch's current tip (or project head for main)
    /// is the parent.
    pub fn append_event_on_branch(
        &self,
        branch: &str,
        kind: &str,
        body: &EventBody,
        batch_id: Option<String>,
        param_fp: Option<String>,
        parent_override: Option<&str>,
    ) -> EventRecord {
        self.with_conn_mut(|c| {
            let tx = c.transaction().unwrap();
            let ev = append_event_locked_on_parent(
                &tx,
                branch,
                kind,
                body,
                batch_id,
                param_fp,
                self.now_ms(),
                parent_override.map(|s| s.to_string()),
            );
            tx.execute(
                "UPDATE projects SET head_event_id=?1 WHERE id=?2",
                params![ev.id, PROJECT_ID],
            )
            .unwrap();
            tx.commit().unwrap();
            ev
        })
    }

    pub fn list_events(&self) -> Vec<EventRecord> {
        self.with_conn(|c| load_events(c, None))
    }

    /// Ancestor chain from the given event (inclusive) back to the root.
    pub fn ancestry(&self, event_id: &str) -> Vec<EventRecord> {
        self.with_conn(|c| {
            let all = load_events(c, None);
            let by_id: std::collections::HashMap<String, EventRecord> =
                all.into_iter().map(|e| (e.id.clone(), e)).collect();
            let mut chain = Vec::new();
            let mut cur = Some(event_id.to_string());
            while let Some(id) = cur {
                if let Some(ev) = by_id.get(&id) {
                    cur = ev.parent_id.clone();
                    chain.push(ev.clone());
                } else {
                    break;
                }
            }
            chain.reverse();
            chain
        })
    }

    pub fn set_head_to(&self, event_id: &str) {
        self.set_head(event_id);
    }

    /// Replay a chain: merged sources + winning params at its tip + decisions.
    pub fn decisions_on_chain(&self, event_id: &str) -> Vec<DecisionRecord> {
        self.ancestry(event_id)
            .into_iter()
            .filter_map(|e| match e.body {
                EventBody::Decision {
                    action,
                    candidate_id,
                    sat,
                    band,
                    at_epoch,
                    reason,
                } => {
                    let action = match action.as_str() {
                        "AcceptCandidate" => crate::engine::DecisionAction::AcceptCandidate,
                        "RejectCandidate" => crate::engine::DecisionAction::RejectCandidate,
                        "ForceBreak" => crate::engine::DecisionAction::ForceBreak,
                        "ParameterRevision" => crate::engine::DecisionAction::ParameterRevision,
                        _ => crate::engine::DecisionAction::RejectCandidate,
                    };
                    Some(DecisionRecord {
                        event_id: e.id,
                        parent_event_id: e.parent_id.unwrap_or_default(),
                        action,
                        candidate_id,
                        sat,
                        band,
                        at_epoch,
                        reason,
                        clock_ms: e.created_at_ms,
                    })
                }
                _ => None,
            })
            .collect()
    }

    pub fn params_on_chain(&self, event_id: &str) -> crate::params::DetectorParams {
        self.ancestry(event_id)
            .into_iter()
            .rev()
            .find_map(|e| match e.body {
                EventBody::Params { params, .. } => Some(params),
                _ => None,
            })
            .unwrap_or_default()
    }
}

pub(crate) fn append_event_locked(
    tx: &Connection,
    branch: &str,
    kind: &str,
    body: &EventBody,
    batch_id: Option<String>,
    param_fp: Option<String>,
    now_ms: i64,
) -> EventRecord {
    append_event_locked_on_parent(tx, branch, kind, body, batch_id, param_fp, now_ms, None)
}

pub(crate) fn append_event_locked_on_parent(
    tx: &Connection,
    branch: &str,
    kind: &str,
    body: &EventBody,
    batch_id: Option<String>,
    param_fp: Option<String>,
    now_ms: i64,
    parent_override: Option<String>,
) -> EventRecord {
    let body_json = serde_json::to_string(body).unwrap();
    let parent = parent_override
        .map(|s| s.to_string())
        .or_else(|| parent_tip(tx, branch));
    let seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE project_id=?1",
            params![PROJECT_ID],
            |r| r.get(0),
        )
        .unwrap();
    let id = ids::event_id(parent.as_deref().unwrap_or("ROOT"), kind, &body_json, now_ms);
    tx.execute(
        "INSERT INTO events(id,project_id,parent_id,branch,seq,kind,body_json,created_at_ms,batch_id,param_fp)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            id, PROJECT_ID, parent, branch, seq, kind, body_json, now_ms, batch_id, param_fp
        ],
    )
    .unwrap();
    EventRecord {
        id,
        parent_id: parent,
        branch: branch.to_string(),
        seq,
        kind: kind.to_string(),
        body: body.clone(),
        created_at_ms: now_ms,
        batch_id,
        param_fp,
    }
}

fn parent_tip(tx: &Connection, branch: &str) -> Option<String> {
    if branch == "main" {
        tx.query_row(
            "SELECT head_event_id FROM projects WHERE id=?1",
            params![PROJECT_ID],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .filter(|s| !s.is_empty())
    } else {
        tx.query_row(
            "SELECT id FROM events WHERE project_id=?1 AND branch=?2 ORDER BY seq DESC LIMIT 1",
            params![PROJECT_ID, branch],
            |r| r.get(0),
        )
        .ok()
    }
}


fn load_events(c: &Connection, _branch: Option<&str>) -> Vec<EventRecord> {
    let mut stmt = c
        .prepare(
            "SELECT id,parent_id,branch,seq,kind,body_json,created_at_ms,batch_id,param_fp
             FROM events WHERE project_id=?1 ORDER BY seq ASC",
        )
        .unwrap();
    let rows = stmt
        .query_map(params![PROJECT_ID], |r| {
            let body_json: String = r.get(5)?;
            Ok(EventRecord {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                branch: r.get(2)?,
                seq: r.get(3)?,
                kind: r.get(4)?,
                body: serde_json::from_str(&body_json).unwrap(),
                created_at_ms: r.get(6)?,
                batch_id: r.get(7)?,
                param_fp: r.get(8)?,
            })
        })
        .unwrap();
    rows.flatten().collect()
}
