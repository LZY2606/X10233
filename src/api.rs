//! Axum HTTP surface. All state lives in one local SQLite database; every
//! displayed value carries a stable id that traces back to source rows,
//! algorithm steps and human decisions.
use crate::case;
use crate::engine;
use crate::model::Params;
use crate::store::Store;
use crate::webassets;
use axum::{
    body::Body,
    extract::{Path, Query, State as AxState},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
}

pub fn app(store: Store) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)) };
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(app_js))
        .route("/static/app.css", get(app_css))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{project}/view", get(view))
        .route("/api/projects/{project}/events", get(events))
        .route("/api/projects/{project}/branches", get(branches))
        .route("/api/projects/{project}/context/{event}", get(context_at))
        .route("/api/projects/{project}/import", post(import_text))
        .route("/api/projects/{project}/params", post(revise_params))
        .route("/api/projects/{project}/decide", post(decide))
        .route("/api/projects/{project}/rewind/{event}", post(rewind))
        .route("/api/projects/{project}/export", get(export_bundle))
        .route("/api/import-bundle", post(import_bundle))
        .route("/api/reset", post(reset))
        .route("/api/cases", get(list_cases))
        .route("/api/cases/{case_id}", get(case_detail))
        .route("/api/cases/{case_id}/file/{version}", get(case_file))
        .route("/api/cases/{case_id}/apply", post(apply_case_step))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(webassets::index_html())
}
async fn app_js() -> Response {
    text_response("application/javascript", webassets::app_js())
}
async fn app_css() -> Response {
    text_response("text/css", webassets::app_css())
}

fn text_response(kind: &str, body: &str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, kind)
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[derive(Deserialize)]
struct ProjectBody {
    name: Option<String>,
}

async fn list_projects(AxState(s): AxStateApp) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    Json(store.list_projects().unwrap_or_default())
}

async fn create_project(AxState(s): AxStateApp, Json(b): Json<ProjectBody>) -> impl IntoResponse {
    let mut store = s.store.lock().unwrap();
    let id = format!("proj-{}", crate::hash::sha256_short(b.name.as_deref().unwrap_or("project").as_bytes()));
    let name = b.name.unwrap_or_else(|| "Review project".into());
    store.create_project(&id, &name).unwrap();
    Json(json!({"id": id, "name": name}))
}

type AxStateApp = AxState<AppState>;

#[derive(Deserialize)]
struct ViewQuery {
    event: Option<String>,
}

async fn view(
    AxState(s): AxStateApp,
    Path(project): Path<String>,
    Query(q): Query<ViewQuery>,
) -> Result<Json<Value>, ApiError> {
    let store = s.store.lock().unwrap();
    let ctx = if let Some(ev) = q.event {
        store.context_at(&ev)?
    } else {
        store
            .head_context(&project)?
            .ok_or_else(|| ApiError::not_found("no events yet; import a sample"))?
    };
    Ok(Json(context_json(&ctx)))
}

fn context_json(ctx: &crate::store::Context) -> Value {
    let v = &ctx.view;
    let raw_epochs = raw_epochs_json(&ctx.file);
    json!({
        "head_event_id": ctx.head_event_id,
        "event": event_json(&ctx.event),
        "input_version_id": ctx.input_version_id,
        "param_version_id": ctx.param_version_id,
        "params": ctx.params,
        "project": {
            "station": v.station,
            "datum": v.datum,
            "units": v.units,
            "satellites": v.satellites,
            "wavelengths": v.bands,
        },
        "epochs": v.epochs,
        "arcs": v.arcs,
        "findings": v.findings,
        "raw_epochs": raw_epochs,
        "diagnostics": v.diagnostics,
        "decisions": decisions_json(&ctx.decisions),
    })
}

fn decisions_json(d: &engine::DecisionState) -> Value {
    json!({
        "accepted": d.accepted.iter().map(|(k,(a,b))| json!({"subject_key":k,"n1":a,"n2":b})).collect::<Vec<_>>(),
        "rejected": d.rejected.iter().collect::<Vec<_>>(),
        "acknowledged": d.acknowledged.iter().collect::<Vec<_>>(),
    })
}

fn raw_epochs_json(file: &crate::model::ImportedFile) -> Value {
    Value::Array(
        file.epochs
            .iter()
            .map(|e| {
                json!({
                    "id": format!("raw-epoch-{}", e.seq),
                    "seq": e.seq,
                    "time_raw": e.time_raw,
                    "time_s": e.time_s,
                    "time_unit": e.time_unit,
                    "time_diagnostic": e.time_diagnostic,
                    "clock_events": e.clock_events,
                    "frequencies": e.frequencies,
                    "sat_states": e.sat_states,
                    "sats": e.sats,
                })
            })
            .collect(),
    )
}

fn event_json(ev: &crate::store::EventInfo) -> Value {
    json!({
        "id": ev.id,
        "seq": ev.seq,
        "parent_id": ev.parent_id,
        "branch": ev.branch,
        "kind": ev.kind,
        "label": ev.label,
        "payload": ev.payload,
        "created_at": ev.created_at,
        "input_version_id": ev.input_version_id,
        "param_version_id": ev.param_version_id,
    })
}

async fn events(AxState(s): AxStateApp, Path(project): Path<String>) -> Result<Json<Value>, ApiError> {
    let store = s.store.lock().unwrap();
    let evs = store.list_events(&project)?;
    let head = store.list_projects()?.into_iter().find(|p| p["id"] == json!(project));
    Ok(Json(json!({
        "events": evs.iter().map(event_json).collect::<Vec<_>>(),
        "head_event_id": head.and_then(|p| p.get("head_event_id").cloned()),
    })))
}

async fn branches(AxState(s): AxStateApp, Path(project): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({ "branches": s.store.lock().unwrap().branches(&project)? })))
}

async fn context_at(
    AxState(s): AxStateApp,
    Path((project, event)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let _ = project;
    let ctx = s.store.lock().unwrap().context_at(&event)?;
    Ok(Json(context_json(&ctx)))
}

#[derive(Deserialize)]
struct ImportBody {
    text: String,
    label: Option<String>,
    branch: Option<String>,
}

async fn import_text(
    AxState(s): AxStateApp,
    Path(project): Path<String>,
    Json(b): Json<ImportBody>,
) -> Result<Json<Value>, ApiError> {
    s.store.lock().unwrap().create_project(&project, &project)?;
    let ev = s.store.lock().unwrap().import_input(
        &project,
        b.label.as_deref().unwrap_or("import"),
        &b.text,
        b.branch,
    )?;
    Ok(Json(json!({"event": event_json(&ev)})))
}

#[derive(Deserialize)]
struct ParamsBody {
    label: Option<String>,
    branch: Option<String>,
    params: Params,
}

async fn revise_params(
    AxState(s): AxStateApp,
    Path(project): Path<String>,
    Json(b): Json<ParamsBody>,
) -> Result<Json<Value>, ApiError> {
    s.store.lock().unwrap().create_project(&project, &project)?;
    let ev = s.store.lock().unwrap().revise_params(
        &project,
        b.label.as_deref().unwrap_or("parameter revision"),
        &b.params,
        b.branch,
    )?;
    Ok(Json(json!({"event": event_json(&ev)})))
}

#[derive(Deserialize)]
struct DecisionBody {
    subject_key: String,
    action: String,
    n1: Option<i64>,
    n2: Option<i64>,
    rationale: Option<String>,
    branch: Option<String>,
}

async fn decide(
    AxState(s): AxStateApp,
    Path(project): Path<String>,
    Json(b): Json<DecisionBody>,
) -> Result<Json<Value>, ApiError> {
    let ev = s.store.lock().unwrap().record_decision(
        &project,
        &b.subject_key,
        &b.action,
        b.n1,
        b.n2,
        b.rationale.as_deref().unwrap_or(""),
        b.branch,
    )?;
    Ok(Json(json!({"event": event_json(&ev)})))
}

async fn rewind(
    AxState(s): AxStateApp,
    Path((project, event)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    s.store.lock().unwrap().rewind_head(&project, &event)?;
    Ok((StatusCode::OK, Json(json!({"head_event_id": event}))))
}

async fn export_bundle(AxState(s): AxStateApp, Path(project): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.store.lock().unwrap().export_bundle(&project)?))
}

#[derive(Deserialize)]
struct BundleBody {
    bundle: Value,
    #[serde(default)]
    project: Option<String>,
}

async fn import_bundle(AxState(s): AxStateApp, Json(b): Json<BundleBody>) -> Result<Json<Value>, ApiError> {
    let ids = s.store.lock().unwrap().import_bundle(&b.bundle)?;
    Ok(Json(json!({"imported_projects": ids})))
}

async fn reset(AxState(s): AxStateApp) -> Result<Json<Value>, ApiError> {
    s.store.lock().unwrap().reset()?;
    Ok(Json(json!({"reset": true})))
}

async fn list_cases() -> Json<Value> {
    let steps: Value = serde_json::from_str(case::steps_json()).unwrap_or(Value::Null);
    Json(json!({"cases": [steps]}))
}

async fn case_detail(Path(case_id): Path<String>) -> Result<Json<Value>, ApiError> {
    if case_id != case::CASE_ID {
        return Err(ApiError::not_found("unknown case"));
    }
    let steps: Value = serde_json::from_str(case::steps_json()).unwrap_or(Value::Null);
    Ok(Json(steps))
}

#[derive(Deserialize)]
struct CaseFilePath {
    version: u32,
}

async fn case_file(
    Path((case_id, version)): Path<(String, u32)>,
) -> Result<Response, ApiError> {
    if case_id != case::CASE_ID {
        return Err(ApiError::not_found("unknown case"));
    }
    let txt = case::file_for_version(version).ok_or_else(|| ApiError::not_found("version"))?;
    Ok(text_response("text/plain; charset=utf-8", txt))
}

#[derive(Deserialize)]
struct ApplyStepBody {
    project: String,
    step_id: String,
}

/// Replay one named step of the bundled case against the local store.
async fn apply_case_step(
    AxState(s): AxStateApp,
    Path(case_id): Path<String>,
    Json(b): Json<ApplyStepBody>,
) -> Result<Json<Value>, ApiError> {
    if case_id != case::CASE_ID {
        return Err(ApiError::not_found("unknown case"));
    }
    let pack: Value = serde_json::from_str(case::steps_json()).unwrap_or(Value::Null);
    let steps = pack.get("steps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let step = steps
        .iter()
        .find(|st| st.get("id").and_then(|v| v.as_str()) == Some(b.step_id.as_str()))
        .ok_or_else(|| ApiError::not_found("unknown step"))?;
    let mut store = s.store.lock().unwrap();
    store.create_project(&b.project, &b.project)?;
    let kind = step.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let title = step.get("title").and_then(|v| v.as_str()).unwrap_or("case step");
    let ev = match kind {
        "import_input" => {
            let file = step.get("file").and_then(|v| v.as_str()).unwrap_or("input-v1.txt");
            let version: u32 = file
                .trim_start_matches("input-v")
                .trim_end_matches(".txt")
                .parse()
                .unwrap_or(1);
            let txt = case::file_for_version(version).ok_or_else(|| ApiError::not_found("case file"))?;
            store.import_input(&b.project, title, txt, None)?
        }
        "revise_params" => {
            let params: Params =
                serde_json::from_value(step.get("params").cloned().unwrap_or(Value::Null))?;
            store.revise_params(&b.project, title, &params, None)?
        }
        "rewind" => {
            let target = step
                .get("target_step")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target_event = find_event_by_step_label(&store, &b.project, target)?;
            store.rewind_head(&b.project, &target_event)?;
            store.get_event(&target_event)?
        }
        other => return Err(ApiError::bad_request(format!("unsupported step kind {other}"))),
    };
    Ok(Json(json!({"event": event_json(&ev)})))
}

fn find_event_by_step_label(
    store: &std::sync::MutexGuard<'_, Store>,
    project: &str,
    step_id: &str,
) -> Result<String, ApiError> {
    let pack: Value = serde_json::from_str(case::steps_json()).unwrap_or(Value::Null);
    let steps = pack.get("steps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let title = steps
        .iter()
        .find(|st| st.get("id").and_then(|v| v.as_str()) == Some(step_id))
        .and_then(|st| st.get("title").and_then(|v| v.as_str()))
        .ok_or_else(|| ApiError::not_found("target step"))?;
    let evs = store.list_events(project)?;
    evs.iter()
        .find(|e| e.label == title)
        .map(|e| e.id.clone())
        .ok_or_else(|| ApiError::not_found("target event not replayed yet"))
}

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn not_found(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: msg.into() }
    }
    fn bad_request(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: msg.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.message}))).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: e.to_string() }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: e.to_string() }
    }
}
