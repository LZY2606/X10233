//! HTTP API + static Web UI.

use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::ingest;
use crate::model::{DecisionKind, DetectorParams, Id};
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
}

pub fn build_router(store: Arc<Store>) -> Router {
    let st = AppState { store };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{pid}/import", post(import_batch))
        .route("/api/projects/{pid}/head", get(head_view))
        .route("/api/projects/{pid}/events", get(events))
        .route("/api/projects/{pid}/events/{seq}", get(history_view))
        .route("/api/projects/{pid}/params", post(revise_params))
        .route("/api/projects/{pid}/decide", post(decide))
        .route("/api/projects/{pid}/resolve-conflict", post(resolve_conflict))
        .route("/api/projects/{pid}/undo", post(undo))
        .route("/api/projects/{pid}/export", get(export_bundle))
        .route("/api/import-bundle", post(import_bundle))
        .route("/api/sample/{name}", get(sample))
        .fallback(get(index))
        .with_state(st)
}

#[derive(Serialize)]
struct ProjectDto {
    id: String,
    name: String,
}

async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Vec<ProjectDto>>> {
    let ps = s.store.list_projects().map_err(ise)?;
    Ok(Json(
        ps.into_iter()
            .map(|(id, name)| ProjectDto { id: id.0, name })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
}

async fn create_project(
    State(s): State<AppState>,
    Json(b): Json<CreateBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = s.store.create_project(&b.name).map_err(ise)?;
    Ok(Json(serde_json::json!({ "id": id.0, "name": b.name })))
}

#[derive(Deserialize)]
struct ImportBody {
    source_name: String,
    content: String,
    #[serde(default)]
    parent_batch_id: Option<String>,
}

async fn import_batch(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(b): Json<ImportBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let project_id = Id(pid);
    let outcome = ingest::parse_any(
        &project_id,
        b.parent_batch_id.map(Id),
        &b.source_name,
        &b.content,
    )
    .map_err(bad)?;
    let n_obs = outcome.batch.observations.len();
    let n_errs = outcome.batch.format_errors.len();
    let n_clock = outcome.batch.declared_clock_events.len();
    let (event_id, batch) = s.store.commit_batch(&project_id, outcome).map_err(ise)?;
    Ok(Json(serde_json::json!({
        "event_id": event_id.0,
        "batch_id": batch.batch_id.0,
        "parent_batch_id": batch.parent_batch_id.map(|i| i.0),
        "observations": n_obs,
        "format_errors": n_errs,
        "declared_clock_events": n_clock,
        "format_error_detail": batch.format_errors,
    })))
}

#[derive(Serialize)]
struct HeadDto {
    at_seq: i64,
    event_id: String,
    station: String,
    params: DetectorParams,
    params_fp: String,
    params_history: Vec<ParamsHistoryDto>,
    batches: Vec<BatchSummaryDto>,
    events: Vec<EventDto>,
    arcs: Vec<crate::model::ArcRecord>,
    candidates: Vec<crate::model::Candidate>,
    residuals: Vec<crate::model::ResidualPoint>,
    clock_events: Vec<crate::model::ClockEvent>,
    decisions: Vec<DecisionDto>,
    conflicts: Vec<ConflictDto>,
    raw: RawDumpDto,
}

#[derive(Serialize)]
struct ParamsHistoryDto {
    parent_fp: String,
    fp: String,
    note: String,
}
#[derive(Serialize)]
struct BatchSummaryDto {
    batch_id: String,
    parent_batch_id: Option<String>,
    source_name: String,
    observations: usize,
    format_errors: usize,
}
#[derive(Serialize)]
struct EventDto {
    seq: i64,
    id: String,
    ts_ms: i64,
    kind: String,
    parent_event_id: Option<String>,
    payload: crate::event::EventPayload,
}
#[derive(Serialize)]
struct DecisionDto {
    event_id: String,
    record: crate::model::DecisionRecord,
}
#[derive(Serialize)]
struct ConflictDto {
    candidate_id: String,
    prior_event: String,
    later_event: String,
    detail: String,
    resolution: Option<String>,
}
#[derive(Serialize)]
struct RawDumpDto {
    observations: Vec<crate::model::Observation>,
    sat_states: Vec<crate::model::SatState>,
}

fn head_to_dto(events: Vec<Event>, hv: &crate::store::HeadView) -> HeadDto {
    let st = &hv.state;
    HeadDto {
        at_seq: hv.at_seq,
        event_id: hv.event_id.0.clone(),
        station: st.station.clone(),
        params: st.params.clone(),
        params_fp: st.params.fingerprint(),
        params_history: st
            .params_history
            .iter()
            .map(|(parent_fp, p, note)| ParamsHistoryDto {
                parent_fp: parent_fp.clone(),
                fp: p.fingerprint(),
                note: note.clone(),
            })
            .collect(),
        batches: st
            .batches
            .iter()
            .map(|b| BatchSummaryDto {
                batch_id: b.batch_id.0.clone(),
                parent_batch_id: b.parent_batch_id.as_ref().map(|i| i.0.clone()),
                source_name: b.source_name.clone(),
                observations: b.observations.len(),
                format_errors: b.format_errors.len(),
            })
            .collect(),
        events: events.iter().map(event_dto).collect(),
        arcs: hv.view.arcs.clone(),
        candidates: hv.view.candidates.clone(),
        residuals: hv.view.residuals.clone(),
        clock_events: hv.view.clock_events.clone(),
        decisions: st
            .decisions
            .iter()
            .map(|(r, eid)| DecisionDto {
                event_id: eid.0.clone(),
                record: r.clone(),
            })
            .collect(),
        conflicts: st
            .conflicts
            .iter()
            .map(|(k, c)| ConflictDto {
                candidate_id: k.clone(),
                prior_event: c.prior_event.0.clone(),
                later_event: c.later_event.0.clone(),
                detail: c.detail.clone(),
                resolution: c.resolution.as_ref().map(|i| i.0.clone()),
            })
            .collect(),
        raw: RawDumpDto {
            observations: {
                let mut v: Vec<_> = st.obs.values().cloned().collect();
                v.sort_by(|a, b| {
                    a.sv.cmp(&b.sv)
                        .then_with(|| a.epoch_raw.cmp(&b.epoch_raw))
                        .then_with(|| a.code.cmp(&b.code))
                });
                v
            },
            sat_states: {
                let mut v: Vec<_> = st.sat_states.values().cloned().collect();
                v.sort_by(|a, b| a.sv.cmp(&b.sv).then_with(|| a.epoch_raw.cmp(&b.epoch_raw)));
                v
            },
        },
    }
}

fn event_dto(e: &Event) -> EventDto {
    EventDto {
        seq: e.seq,
        id: e.id.0.clone(),
        ts_ms: e.ts_ms,
        kind: format!("{:?}", e.kind),
        parent_event_id: e.parent_event_id.as_ref().map(|i| i.0.clone()),
        payload: e.payload.clone(),
    }
}

async fn head_view(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
) -> ApiResult<Json<HeadDto>> {
    let pid = Id(pid);
    let hv = s.store.head(&pid).map_err(ise)?;
    let events = s.store.events(&pid).map_err(ise)?;
    Ok(Json(head_to_dto(events, &hv)))
}

async fn events(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
) -> ApiResult<Json<Vec<EventDto>>> {
    let evs = s.store.events(&Id(pid)).map_err(ise)?;
    Ok(Json(evs.iter().map(event_dto).collect()))
}

async fn history_view(
    State(s): State<AppState>,
    AxumPath((pid, seq)): AxumPath<(String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    let hv = s.store.at_event(&Id(pid), seq).map_err(ise)?;
    Ok(Json(serde_json::to_value(&hv.view).map_err(ise)?))
}

#[derive(Deserialize)]
struct ParamsBody {
    params: DetectorParams,
    note: String,
}

async fn revise_params(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(b): Json<ParamsBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let ev = s
        .store
        .revise_params(&Id(pid), b.params, &b.note)
        .map_err(ise)?;
    Ok(Json(serde_json::json!({ "event_id": ev.0 })))
}

#[derive(Deserialize)]
struct DecideBody {
    candidate_id: String,
    kind: DecisionKindWire,
    #[serde(default)]
    shift_b1_cycles: Option<i64>,
    #[serde(default)]
    shift_b2_cycles: Option<i64>,
    #[serde(default)]
    note: String,
}

#[derive(Deserialize, Serialize, Clone, Copy)]
pub enum DecisionKindWire {
    Accept,
    Reject,
}

impl From<DecisionKindWire> for DecisionKind {
    fn from(w: DecisionKindWire) -> Self {
        match w {
            DecisionKindWire::Accept => DecisionKind::Accept,
            DecisionKindWire::Reject => DecisionKind::Reject,
        }
    }
}

async fn decide(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(b): Json<DecideBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let ev = s
        .store
        .decide(
            &Id(pid),
            &Id(b.candidate_id),
            b.kind.into(),
            b.shift_b1_cycles,
            b.shift_b2_cycles,
            &b.note,
        )
        .map_err(ise)?;
    Ok(Json(serde_json::json!({ "event_id": ev.0 })))
}

#[derive(Deserialize)]
struct ResolveBody {
    candidate_id: String,
    winning_event_id: String,
    note: String,
}

async fn resolve_conflict(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(b): Json<ResolveBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let ev = s
        .store
        .resolve_conflict(&Id(pid), &Id(b.candidate_id), &Id(b.winning_event_id), &b.note)
        .map_err(ise)?;
    Ok(Json(serde_json::json!({ "event_id": ev.0 })))
}

#[derive(Deserialize)]
struct UndoBody {
    target_event_id: String,
    note: String,
}

async fn undo(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(b): Json<UndoBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let ev = s
        .store
        .undo(&Id(pid), &Id(b.target_event_id), &b.note)
        .map_err(ise)?;
    Ok(Json(serde_json::json!({ "event_id": ev.0 })))
}

async fn export_bundle(
    State(s): State<AppState>,
    AxumPath(pid): AxumPath<String>,
) -> ApiResult<Response> {
    let bundle = s.store.export(&Id(pid)).map_err(ise)?;
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        bundle,
    )
        .into_response())
}

#[derive(Deserialize)]
struct BundleBody {
    bundle: String,
}

async fn import_bundle(
    State(s): State<AppState>,
    Json(b): Json<BundleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = s.store.import_bundle(&b.bundle).map_err(bad)?;
    Ok(Json(serde_json::json!({ "project_id": id.0 })))
}

#[derive(Deserialize)]
struct SampleQuery {
    #[allow(dead_code)]
    raw: Option<String>,
}

async fn sample(
    AxumPath(name): AxumPath<String>,
    Query(_q): Query<SampleQuery>,
) -> ApiResult<Response> {
    let path = match name.as_str() {
        "bench" | "carrier-break-bench" => "cases/carrier-break-bench/bench.sheet.json",
        "bench-rinex" => "cases/carrier-break-bench/bench.obs",
        "bad" | "format-error" => "samples/format-error.sheet.json",
        "bad-rinex" => "samples/format-error.obs",
        other => {
            return Err((
                axum::http::StatusCode::NOT_FOUND,
                format!("unknown sample {other}"),
            ));
        }
    };
    let body = tokio::fs::read(path)
        .await
        .map_err(|e| (axum::http::StatusCode::NOT_FOUND, e.to_string()))?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        body,
    )
        .into_response())
}

async fn index() -> Response {
    let html = include_str!("../../web/index.html");
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html.to_string(),
    )
        .into_response()
}

// --- error plumbing ---
type ApiResult<T> = Result<T, (axum::http::StatusCode, String)>;
fn ise(e: impl std::fmt::Display) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn bad(e: impl std::fmt::Display) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::BAD_REQUEST, e.to_string())
}
