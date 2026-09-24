//! Axum web UI + JSON API.

use crate::domain::*;
use crate::ingest::{merge_sat_states, parse_rinex3};
use crate::state::{self, Engine, RawVersionRec};
use crate::store::Store;
use crate::vclock::VClock;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub clock: Arc<Mutex<VClock>>,
}

pub fn app(store: Arc<Store>, clock: VClock) -> Router {
    let state = AppState { store, clock: Arc::new(Mutex::new(clock)) };
    Router::new()
        .route("/", get(index))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/:pid", get(project_view).delete(delete_project))
        .route("/api/projects/:pid/events", get(events))
        .route("/api/projects/:pid/view/:event_id", get(view_at))
        .route("/api/projects/:pid/import-rinex", post(import_rinex))
        .route("/api/projects/:pid/import-state", post(import_state))
        .route("/api/projects/:pid/params", post(revise_params))
        .route("/api/projects/:pid/adjudicate", post(adjudicate))
        .route("/api/projects/:pid/stale", get(stale_rows))
        .route("/api/export", get(export_all))
        .route("/api/import", post(import_all))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

#[derive(Serialize)]
struct ErrBody {
    error: String,
}

fn err(msg: impl Into<String>) -> (StatusCode, Json<ErrBody>) {
    (StatusCode::BAD_REQUEST, Json(ErrBody { error: msg.into() }))
}

fn not_found(msg: impl Into<String>) -> (StatusCode, Json<ErrBody>) {
    (StatusCode::NOT_FOUND, Json(ErrBody { error: msg.into() }))
}

// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    #[serde(default)]
    id: Option<String>,
}

async fn create_project(
    State(st): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let id = body.id.unwrap_or_else(|| format!("prj-{}", short_hash(&body.name)[..10].to_string()));
    if st.store.project_exists(&id) {
        return Err(err("project id already exists"));
    }
    let mut clk = st.clock.lock().unwrap();
    let ms = clk.now_ms() as i64;
    let seq = 1i64;
    let ev = Event {
        id: format!("ev:{}:0001", id),
        seq,
        parent_event_id: None,
        branch: "main".into(),
        kind: EventKind::ProjectCreated,
        virtual_ms: ms,
        actor: "reviewer".into(),
        summary: format!("project '{}' created", body.name),
        payload: serde_json::json!({"name": body.name}),
    };
    st.store.create_project(&id, &body.name, ms, &ev.id).map_err(|e| err(e.to_string()))?;
    st.store.append_event(&id, &ev).map_err(|e| err(e.to_string()))?;
    Ok(Json(serde_json::json!({"project_id": id, "event": ev})))
}

async fn list_projects(State(st): State<AppState>) -> Json<serde_json::Value> {
    let projs = st.store.list_projects().unwrap_or_default();
    Json(serde_json::json!({"projects": projs.iter().map(|(id,name)| serde_json::json!({"id":id,"name":name})).collect::<Vec<_>>()}))
}

async fn delete_project(
    State(st): State<AppState>,
    Path(pid): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    st.store.delete_project(&pid).map_err(|e| err(e.to_string()))?;
    Ok(Json(serde_json::json!({"deleted": pid})))
}

async fn project_view(
    State(st): State<AppState>,
    Path(pid): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    if events.is_empty() {
        return Err(not_found("unknown project"));
    }
    let snap = Engine::new(events.clone()).materialize(&pid, None);
    let chains = state::branch_chains(&events)
        .iter()
        .map(|c| serde_json::json!({"tip": c.last().map(|e| e.id.clone()), "branch": c.last().map(|e| e.branch.clone()), "length": c.len()}))
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({"snapshot": snap, "branches": chains})))
}

async fn events(
    State(st): State<AppState>,
    Path(pid): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    Ok(Json(serde_json::json!({"events": events})))
}

async fn view_at(
    State(st): State<AppState>,
    Path((pid, event_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    let chain = state::chain_until(&events, &event_id);
    if chain.is_empty() || chain.last().map(|e| e.id != event_id).unwrap_or(true) {
        return Err(not_found("event not found"));
    }
    let engine = Engine::new(chain);
    let seq = events.iter().position(|e| e.id == event_id).map(|i| i as i64 + 1).unwrap_or(0);
    let _ = seq;
    let snap = engine.materialize(&pid, None);
    Ok(Json(serde_json::json!({"snapshot": snap})))
}

// ---------------------------------------------------------------------------
// import handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RinexBody {
    filename: String,
    content: String,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    parent_event_id: Option<String>,
}

fn next_event(st: &AppState, pid: &str, parent: &str, kind: EventKind, actor: &str, summary: &str, payload: serde_json::Value) -> Result<Event, String> {
    let events = st.store.list_events(pid).map_err(|e| e.to_string())?;
    let seq = events.len() as i64 + 1;
    let parent_ev = events
        .iter()
        .find(|e| e.id == parent)
        .ok_or_else(|| format!("parent event {parent} not found"))?;
    let mut clk = st.clock.lock().unwrap();
    let ev = Event {
        id: format!("ev:{pid}:{seq:04}"),
        seq,
        parent_event_id: Some(parent.to_string()),
        branch: parent_ev.branch.clone(),
        kind,
        virtual_ms: clk.now_ms() as i64,
        actor: actor.into(),
        summary: summary.into(),
        payload,
    };
    st.store.append_event(pid, &ev).map_err(|e| e.to_string())?;
    st.store.set_head(pid, &ev.id);
    Ok(ev)
}

async fn import_rinex(
    State(st): State<AppState>,
    Path(pid): Path<String>,
    Json(body): Json<RinexBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    if events.is_empty() {
        return Err(not_found("unknown project"));
    }
    let parent = body.parent_event_id.clone().or_else(|| state::head_event(&events).map(|e| e.id.clone())).unwrap();
    let (mut bundle, report) = match parse_rinex3(&body.content) {
        Ok(v) => v,
        Err(e) => return Err(err(format!("{body_filename}: {e}", body_filename = body.filename))),
    };
    if report.has_errors() {
        return Err(err(report.errors.join("; ")));
    }
    bundle.station = bundle.marker.clone();

    // merge with existing raw version if this chains on from one
    let snap = Engine::new(events.clone()).materialize(&pid, None);
    let mut diagnostics = report.warnings.clone();
    if let Some(rv) = latest_raw(&st, &pid).map_err(err)? {
        let (merged, d) = state::merge_patch(&rv.bundle, &bundle, diagnostics);
        bundle = merged;
        diagnostics = d;
    }
    let payload = serde_json::json!({
        "kind": "rinex",
        "sources": [body.filename],
        "parent_version": snap.raw_version,
        "diagnostics": diagnostics,
        "bundle": bundle,
    });
    let ev = next_event(&st, &pid, &parent, EventKind::RawCommitted, body.actor.as_deref().unwrap_or("reviewer"), &format!("imported RINEX '{}'", body.filename), payload)
        .map_err(err)?;
    archive_superseded(&st, &pid, &events, &ev.id);
    Ok(Json(serde_json::json!({"event": ev, "warnings": diagnostics})))
}

#[derive(Deserialize)]
struct StateBody {
    filename: String,
    content: String,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    parent_event_id: Option<String>,
}

async fn import_state(
    State(st): State<AppState>,
    Path(pid): Path<String>,
    Json(body): Json<StateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    if events.is_empty() {
        return Err(not_found("unknown project"));
    }
    let rv = latest_raw(&st, &pid).map_err(err)?.ok_or_else(|| err("import RINEX observations before satellite states"))?;
    let parent = body.parent_event_id.clone().unwrap_or_else(|| state::head_event(&events).unwrap().id.clone());
    let mut bundle = rv.bundle.clone();
    let report = merge_sat_states(&mut bundle, &body.content).map_err(err)?;
    if report.has_errors() {
        return Err(err(report.errors.join("; ")));
    }
    let payload = serde_json::json!({
        "kind": "satstate",
        "sources": [body.filename],
        "parent_version": rv.version,
        "diagnostics": report.warnings,
        "bundle": bundle,
    });
    let ev = next_event(&st, &pid, &parent, EventKind::RawCommitted, body.actor.as_deref().unwrap_or("reviewer"), &format!("imported satellite states '{}'", body.filename), payload)
        .map_err(err)?;
    archive_superseded(&st, &pid, &events, &ev.id);
    Ok(Json(serde_json::json!({"event": ev, "warnings": report.warnings})))
}

fn latest_raw(st: &AppState, pid: &str) -> Result<Option<RawVersionRec>, String> {
    let events = st.store.list_events(pid).map_err(|e| e.to_string())?;
    let mut rec: Option<RawVersionRec> = None;
    for ev in &events {
        if ev.kind == EventKind::RawCommitted {
            let bundle: RawBundle = serde_json::from_value(ev.payload["bundle"].clone()).map_err(|e| e.to_string())?;
            let parent_version = ev.payload["parent_version"].as_u64().map(|v| v as u32);
            let kind = ev.payload["kind"].as_str().unwrap_or("full").to_string();
            let sources = ev.payload["sources"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
            rec = Some(RawVersionRec {
                version: rec.as_ref().map(|r| r.version + 1).unwrap_or(1),
                parent_version,
                fingerprint: crate::ingest::fingerprint_bundle(&bundle),
                kind,
                sources,
                committed_event_id: ev.id.clone(),
                bundle,
            });
        }
    }
    Ok(rec)
}

#[derive(Deserialize)]
struct ParamsBody {
    params: Params,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    parent_event_id: Option<String>,
}

async fn revise_params(
    State(st): State<AppState>,
    Path(pid): Path<String>,
    Json(body): Json<ParamsBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    if events.is_empty() {
        return Err(not_found("unknown project"));
    }
    let parent = body.parent_event_id.clone().unwrap_or_else(|| state::head_event(&events).unwrap().id.clone());
    let old_fp = state::params_fp(&Params::default());
    let snap = Engine::new(events.clone()).materialize(&pid, None);
    let payload = serde_json::json!({
        "params": body.params,
        "previous_params_fp": snap.params_fp,
    });
    let _ = old_fp;
    let ev = next_event(
        &st,
        &pid,
        &parent,
        EventKind::ParamsRevised,
        body.actor.as_deref().unwrap_or("reviewer"),
        body.summary.as_deref().unwrap_or("detector parameters revised"),
        payload,
    )
    .map_err(err)?;
    archive_superseded(&st, &pid, &events, &ev.id);
    Ok(Json(serde_json::json!({"event": ev})))
}

#[derive(Deserialize)]
struct AdjudicateBody {
    boundary_id: String,
    decision: String,
    #[serde(default)]
    candidate_id: Option<String>,
    #[serde(default)]
    offsets: Option<std::collections::BTreeMap<String, i64>>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    parent_event_id: Option<String>,
    /// conflicting with a prior decision -> recorded as conflict resolution
    #[serde(default)]
    supersedes_decision_event: Option<String>,
}

async fn adjudicate(
    State(st): State<AppState>,
    Path(pid): Path<String>,
    Json(body): Json<AdjudicateBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let events = st.store.list_events(&pid).map_err(|e| err(e.to_string()))?;
    if events.is_empty() {
        return Err(not_found("unknown project"));
    }
    let decision = match body.decision.as_str() {
        "accepted" => Decision::Accepted,
        "rejected" => Decision::Rejected,
        "unresolved" => Decision::Unresolved,
        other => return Err(err(format!("unknown decision '{other}'"))),
    };
    let snap = Engine::new(events.clone()).materialize(&pid, None);
    let bnd = snap
        .boundaries
        .iter()
        .find(|b| b.id == body.boundary_id)
        .ok_or_else(|| err("boundary id not found in current state"))?;

    // Resolve offsets from chosen candidate or explicit payload.
    let offsets = if let Some(cid) = &body.candidate_id {
        let cand = bnd
            .combos
            .iter()
            .flat_map(|r| r.candidates.iter())
            .chain(bnd.per_band.values().flat_map(|r| r.td.candidates.iter()))
            .find(|c| c.id == *cid)
            .ok_or_else(|| err("candidate id not found on boundary"))?;
        cand.offset_cycles.clone()
    } else {
        body.offsets.clone().unwrap_or_default()
    };

    let parent = body
        .parent_event_id
        .clone()
        .unwrap_or_else(|| state::head_event(&events).unwrap().id.clone());
    let conflicts = bnd.decided_by_event.is_some();
    let kind = if conflicts || body.supersedes_decision_event.is_some() {
        EventKind::ConflictResolved
    } else {
        EventKind::Adjudicated
    };
    let summary = format!(
        "{} on {} -> {} (offsets {:?})",
        if kind == EventKind::ConflictResolved { "conflict resolution" } else { "adjudication" },
        body.boundary_id,
        decision.as_str(),
        offsets
    );
    let payload = serde_json::json!({
        "boundary_id": body.boundary_id,
        "candidate_id": body.candidate_id,
        "decision": decision.as_str(),
        "offsets": offsets,
        "note": body.note,
        "supersedes": body.supersedes_decision_event.or_else(|| bnd.decided_by_event.clone()),
    });
    let ev = next_event(
        &st,
        &pid,
        &parent,
        kind,
        body.actor.as_deref().unwrap_or("reviewer"),
        &summary,
        payload,
    )
    .map_err(err)?;
    archive_superseded(&st, &pid, &events, &ev.id);
    Ok(Json(serde_json::json!({"event": ev})))
}

async fn stale_rows(
    State(st): State<AppState>,
    Path(pid): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"stale_rows": st.store.list_stale(&pid)}))
}

async fn export_all(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(st.store.export_all())
}

#[derive(Deserialize)]
struct ImportBody {
    doc: serde_json::Value,
    #[serde(default)]
    replace: bool,
}

async fn import_all(
    State(st): State<AppState>,
    Json(body): Json<ImportBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrBody>)> {
    let n = st.store.import_all(&body.doc, body.replace).map_err(err)?;
    Ok(Json(serde_json::json!({"imported_projects": n})))
}

/// Archive only derived rows whose direct source content changed (or whose
/// params fingerprint changed). Local input changes expire local results.
fn archive_superseded(st: &AppState, pid: &str, old_events: &[Event], new_event_id: &str) {
    let old = Engine::new(old_events.to_vec()).materialize(pid, None);
    let mut new_events = old_events.to_vec();
    if let Some(ev) = st.store.list_events(pid).ok().and_then(|evs| evs.into_iter().find(|e| e.id == new_event_id)) {
        new_events.push(ev);
    } else {
        return;
    }
    let new = Engine::new(new_events).materialize(pid, None);

    let old_key = |r: &state::ViewResult| {
        (
            r.id.clone(),
            r.residual.to_bits(),
            r.fired,
            r.unknown,
            r.reason.clone(),
            r.candidates.iter().map(|c| (c.id.clone(), c.status.clone(), c.score.to_bits())).collect::<Vec<_>>(),
        )
    };
    let collect = |snap: &state::Snapshot| {
        let mut rows = Vec::new();
        for b in &snap.boundaries {
            for (_, br) in &b.per_band {
                rows.push((br.td.id.clone(), br.td.clone()));
                rows.push((br.lli.id.clone(), br.lli.clone()));
            }
            for c in &b.combos {
                rows.push((c.id.clone(), c.clone()));
            }
        }
        rows
    };
    let old_rows = collect(&old);
    let new_rows = collect(&new);
    let changed_ids: std::collections::BTreeSet<String> = {
        let new_by_id: std::collections::BTreeMap<_, _> =
            new_rows.iter().map(|(id, r)| (id.clone(), old_key(r))).collect();
        let mut s = std::collections::BTreeSet::new();
        for (id, r) in &old_rows {
            match new_by_id.get(id) {
                None => {
                    s.insert(id.clone());
                }
                Some(k) if *k != old_key(r) => {
                    s.insert(id.clone());
                }
                _ => {}
            }
        }
        s
    };

    let archived: Vec<serde_json::Value> = old_rows
        .into_iter()
        .filter(|(id, _)| changed_ids.contains(id))
        .map(|(id, r)| {
            serde_json::json!({
                "kind": "detector_result",
                "id": id,
                "superseded_at_event": new_event_id,
                "algo_version": r.prov.algo_version,
                "params_fp": r.prov.params_fp,
                "raw_version": r.prov.raw_version,
                "basis_event_id": r.prov.basis_event_id,
                "state": "stale",
                "result": r,
            })
        })
        .collect();
    if !archived.is_empty() {
        st.store.archive_stale(pid, new_event_id, archived);
    }
}
