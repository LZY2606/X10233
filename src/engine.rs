//! Event-sourced replay engine. Events are append-only and immutable; raw
//! inputs are never overwritten. Derived nodes (steps, arcs, clock jumps,
//! repair offsets) are content-addressed, so a local source change only
//! invalidates nodes whose identity changed. Superseded nodes remain stored
//! and viewable but never masquerade as current results.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::detect::{self, ArcNode, ClockJump, ComputeInput, StepNode};
use crate::model::*;
use crate::util::hid;

pub const EVT_PROJECT_CREATED: &str = "project-created";
pub const EVT_SOURCES_REVISED: &str = "sources-revised";
pub const EVT_PARAMS_REVISED: &str = "params-revised";
pub const EVT_CANDIDATE_REVIEWED: &str = "candidate-reviewed";
pub const EVT_DECISION_UNDONE: &str = "decision-undone";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectCreated {
    pub name: String,
    pub source_id: String,
    pub param_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffsetNode {
    pub id: String,
    pub candidate_id: String,
    pub sat: SatId,
    pub epoch_index: u64,
    pub step_id: String,
    pub offsets: Vec<OffsetPair>,
    pub decision_event_id: String,
    pub param_fingerprint: String,
    pub active: bool,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UnknownMark {
    pub candidate_id: String,
    pub sat: SatId,
    pub epoch_index: u64,
    pub step_id: String,
    pub decision_event_id: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub event_id: String,
    pub seq: i64,
    pub candidate_id: String,
    pub sat: SatId,
    pub epoch_index: u64,
    pub decision: String,
    pub comment: String,
    pub reviewer: String,
    pub supersedes_event_id: Option<String>,
    pub undone: bool,
    pub created_ms: i64,
    pub offsets: Vec<OffsetPair>,
    pub candidate_snapshot: Option<Candidate>,
}

#[derive(Debug, Clone)]
pub struct ReplayState {
    pub project_id: String,
    pub branch_id: String,
    pub source: Option<SourceBundle>,
    pub source_id: Option<String>,
    pub params: DetectorParams,
    pub steps: Vec<StepNode>,
    pub arcs: Vec<ArcNode>,
    pub clock_jumps: Vec<ClockJump>,
    pub wavelengths: BTreeMap<String, f64>,
    pub offsets: Vec<OffsetNode>,
    pub unknowns: Vec<UnknownMark>,
    pub decisions: Vec<DecisionRecord>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub slot_ids: BTreeMap<String, String>,
    pub sat_state_ids: BTreeMap<String, String>,
    pub clock_source_ids: BTreeMap<u64, String>,
    pub events_seen: Vec<StoredEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: String,
    pub seq: i64,
    pub event_type: String,
    pub source_id: Option<String>,
    pub param_fingerprint: Option<String>,
    pub decision_key: Option<String>,
    pub supersedes_event_id: Option<String>,
    pub undoes_event_id: Option<String>,
    pub undone: bool,
    pub payload_json: String,
    pub author: String,
    pub created_ms: i64,
}

impl ReplayState {
    pub fn empty(project_id: String, branch_id: String) -> Self {
        Self {
            project_id,
            branch_id,
            source: None,
            source_id: None,
            params: DetectorParams::default(),
            steps: Vec::new(),
            arcs: Vec::new(),
            clock_jumps: Vec::new(),
            wavelengths: BTreeMap::new(),
            offsets: Vec::new(),
            unknowns: Vec::UnknownMark>::new(),
            decisions: Vec::new(),
            diagnostics: Vec::new(),
            slot_ids: BTreeMap::new(),
            sat_state_ids: BTreeMap::new(),
            clock_source_ids: BTreeMap::new(),
            events_seen: Vec::new(),
        }
    }

    fn recompute(&mut self, source_id: &str) {
        let scope = self.project_id.clone();
        let _ = source_id;
        let Some(bundle) = self.source.clone() else {
            self.steps.clear();
            self.arcs.clear();
            self.clock_jumps.clear();
            return;
        };
        build_source_identities(
            &bundle,
            &scope,
            &mut self.slot_ids,
            &mut self.sat_state_ids,
            &mut self.clock_source_ids,
        );
        let input = ComputeInput {
            bundle,
            params: self.params.clone(),
            slot_ids: self.slot_ids.clone(),
            sat_state_ids: self.sat_state_ids.clone(),
            clock_source_ids: self.clock_source_ids.clone(),
            scope_id: source_id.to_string(),
            wavelengths: BTreeMap::new(),
        };
        let result = detect::compute(&input);
        self.steps = result.steps;
        self.arcs = result.arcs;
        self.clock_jumps = result.clock_jumps;
        self.wavelengths = result.wavelengths;
    }

    fn find_candidate(&self, candidate_id: &str) -> Option<(StepNode, Candidate)> {
        for st in &self.steps {
            for c in &st.candidates {
                if c.id == candidate_id {
                    return Some((st.clone(), c.clone()));
                }
            }
        }
        None
    }

    fn active_offset_for(
        &self,
        sat: &SatId,
        epoch_index: u64,
    ) -> Option<&OffsetNode> {
        self.offsets
            .iter()
            .filter(|o| o.active && o.sat == *sat && o.epoch_index == epoch_index)
            .last()
    }
}

pub fn stable_slot_id(source_id: &str, epoch: u64, sat: &SatId, code: &str) -> String {
    hid("slot:", &format!("{source_id}:{epoch}:{}:{code}", sat.raw()))
}

fn build_source_identities(
    bundle: &SourceBundle,
    source_id: &str,
    slot_ids: &mut BTreeMap<String, String>,
    sat_state_ids: &mut BTreeMap<String, String>,
    clock_ids: &mut BTreeMap<u64, String>,
) {
    for epoch in &bundle.obs.epochs {
        for rec in &epoch.records {
            for slot in &rec.slots {
                let k = format!(
                    "{}:{}:{}",
                    epoch.epoch_index,
                    rec.sat.raw(),
                    slot.code.raw
                );
                slot_ids.insert(k, stable_slot_id(source_id, epoch.epoch_index, &rec.sat, &slot.code.raw));
            }
        }
    }
    for st in &bundle.sat_states {
        let k = format!("{}:{}:{}", st.sat.raw(), st.band, st.code);
        sat_state_ids.insert(k, hid("satst:", &format!("{source_id}:{k}")));
    }
    for c in &bundle.clock_events {
        clock_ids.insert(
            c.epoch_index,
            hid("clksrc:", &format!("{source_id}:{}:{}", c.event_index, c.epoch_index)),
        );
    }
}

#[derive(Debug, Clone)]
pub enum ApplyEffect {
    Recomputed,
    Decision,
    None,
}

/// Apply one stored event to a replay state. This is used both for full
/// replays and for as-of-event historical views.
pub fn apply_event(state: &mut ReplayState, ev: &StoredEvent, ctx: &ReplayCtx) {
    state.events_seen.push(ev.clone());
    match ev.event_type.as_str() {
        EVT_PROJECT_CREATED => {
            let p: ProjectCreated = serde_json::from_str(&ev.payload_json).unwrap();
            if let Some((bundle, diags)) = (ctx.sources)(&p.source_id) {
                state.source = Some(bundle);
                state.source_id = Some(p.source_id);
                state.diagnostics = diags;
                state.params = (ctx.params)(&p.param_fingerprint);
                state.recompute(&p.source_id);
            }
        }
        EVT_SOURCES_REVISED => {
            let r: SourceRevision = serde_json::from_str(&ev.payload_json).unwrap();
            if let Some((bundle, diags)) = (ctx.sources)(ev.source_id.as_deref().unwrap_or(&r.parent_source_id)) {
                state.source = Some(bundle);
                state.source_id = ev.source_id.clone();
                state.diagnostics = diags;
                state.recompute(ev.source_id.as_deref().unwrap_or(&r.parent_source_id));
            }
        }
        EVT_PARAMS_REVISED => {
            let r: ParamRevision = serde_json::from_str(&ev.payload_json).unwrap();
            state.params = r.new_params;
            if let Some(sid) = &state.source_id {
                state.recompute(sid);
            }
        }
        EVT_CANDIDATE_REVIEWED => {
            apply_review(state, ev);
        }
        EVT_DECISION_UNDONE => {
            let target = ev.undoes_event_id.clone().unwrap();
            if let Some(off) = state
                .offsets
                .iter_mut()
                .find(|o| o.decision_event_id == target)
            {
                off.active = false;
                off.superseded_by = Some(ev.id.clone());
            }
            for u in state.unknowns.iter_mut().filter(|u| u.decision_event_id == target) {
                u.note = format!("{}\n[undone by {}]", u.note, ev.id);
            }
            for d in state.decisions.iter_mut().filter(|d| d.event_id == target) {
                d.undone = true;
            }
        }
        _ => {}
    }
}

fn apply_review(state: &mut ReplayState, ev: &StoredEvent) {
    let payload: ReviewPayload = serde_json::from_str(&ev.payload_json).unwrap();
    let key = ev.decision_key.clone().unwrap_or_default();
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    let sat = parts.first().and_then(|s| SatId::parse(s)).unwrap_or(SatId {
        system: "G".into(),
        prn: 0,
    });
    let epoch_index: u64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let candidate_id = parts.get(2).cloned().unwrap_or_default();
    let (step, cand) = state
        .find_candidate(&candidate_id)
        .unwrap_or_else(|| {
            let st = StepNode {
                id: "step:missing".into(),
                sat: sat.clone(),
                epoch_index,
                prev_epoch_index: epoch_index.saturating_sub(1),
                delta_seconds: 0.0,
                irregular_gap: false,
                clock_jump_id: None,
                lli_flags: vec![],
                missing_bands: vec![],
                boundary_reasons: vec![],
                candidates: vec![],
                residuals: Default::default(),
                status: "unknown".into(),
            };
            (st, Candidate {
                id: candidate_id.clone(),
                sat: sat.clone(),
                epoch_index,
                location: String::new(),
                offsets: vec![],
                score: 0,
                ambiguous: false,
                tie_group_size: 0,
                lli_evidence: false,
                clock_event_id: None,
                status: "stale".into(),
                proposals: vec![],
                reject_reasons: vec!["candidate not present at this event".into()],
            })
        });

    // Conflict resolution: a later decision on the same key supersedes earlier
    // active offsets. Everything is kept as history.
    if payload.decision == "accept" {
        if let Some(existing) = state.active_offset_for(&sat, epoch_index) {
            let id = existing.id.clone();
            if let Some(o) = state.offsets.iter_mut().find(|o| o.id == id) {
                o.active = false;
                o.superseded_by = Some(ev.id.clone());
            }
        }
        let off = OffsetNode {
            id: hid("offset:", &format!("{}:{}:{}", candidate_id, ev.id, ev.seq)),
            candidate_id: candidate_id.clone(),
            sat: sat.clone(),
            epoch_index,
            step_id: step.id.clone(),
            offsets: cand.offsets.clone(),
            decision_event_id: ev.id.clone(),
            param_fingerprint: ev.param_fingerprint.clone().unwrap_or_default(),
            active: true,
            superseded_by: payload.supersedes_event_id.clone(),
        };
        state.offsets.push(off);
        state.unknowns.retain(|u| !(u.sat == sat && u.epoch_index == epoch_index));
    } else if payload.decision == "mark-unknown" {
        state.unknowns.push(UnknownMark {
            candidate_id: candidate_id.clone(),
            sat: sat.clone(),
            epoch_index,
            step_id: step.id.clone(),
            decision_event_id: ev.id.clone(),
            note: payload.comment.clone(),
        });
    }
    state.decisions.push(DecisionRecord {
        event_id: ev.id.clone(),
        seq: ev.seq,
        candidate_id,
        sat,
        epoch_index,
        decision: payload.decision.clone(),
        comment: payload.comment,
        reviewer: payload.reviewer,
        supersedes_event_id: payload.supersedes_event_id,
        undone: false,
        created_ms: ev.created_ms,
        offsets: cand.offsets.clone(),
        candidate_snapshot: Some(cand),
    });
}

pub fn decision_key(sat: &SatId, epoch_index: u64, candidate_id: &str) -> String {
    format!("{}:{}:{}", sat.raw(), epoch_index, candidate_id)
}

/// Node identity sets derived from a replay state. The materializer compares
/// these against stored nodes to find locally-changed dependencies.
#[derive(Debug, Clone, Default)]
pub struct NodeSets {
    pub steps: BTreeSet<String>,
    pub arcs: BTreeSet<String>,
    pub clocks: BTreeSet<String>,
    pub steps_json: BTreeMap<String, String>,
    pub arcs_json: BTreeMap<String, String>,
    pub clocks_json: BTreeMap<String, String>,
}

pub fn node_sets(state: &ReplayState) -> NodeSets {
    let mut ns = NodeSets::default();
    for s in &state.steps {
        ns.steps.insert(s.id.clone());
        ns.steps_json.insert(s.id.clone(), serde_json::to_string(s).unwrap());
    }
    for a in &state.arcs {
        ns.arcs.insert(a.id.clone());
        ns.arcs_json.insert(a.id.clone(), serde_json::to_string(a).unwrap());
    }
    for c in &state.clock_jumps {
        ns.clocks.insert(c.id.clone());
        ns.clocks_json.insert(c.id.clone(), serde_json::to_string(c).unwrap());
    }
    ns
}

pub struct ReplayCtx<'a> {
    pub sources: &'a dyn Fn(&str) -> Option<(SourceBundle, Vec<ParseDiagnostic>)>,
    pub params: &'a dyn Fn(&str) -> DetectorParams,
}
