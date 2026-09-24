//! Axum HTTP API：全部审阅动作（导入、修订、裁定、分叉、回查、导出/导入）。

use crate::bundle;
use crate::domain::DetectParams;
use crate::engine::{self, AppState};
use crate::sample;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

type Shared = Arc<AppState>;

fn err(status: StatusCode, message: impl Into<String>) -> impl IntoResponse {
    (status, Json(json!({"error": message.into()})))
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/samples/{name}", get(get_sample))
        .route("/api/projects/{project}/import", post(import_text))
        .route("/api/projects/{project}/import-bundle", post(import_bundle))
        .route("/api/projects/{project}/export", get(export))
        .route("/api/projects/{project}/branches", get(list_branches).post(create_branch))
        .route("/api/projects/{project}/branches/{branch}", get(branch_detail))
        .route("/api/projects/{project}/branches/{branch}/batches", get(list_batches))
        .route("/api/projects/{project}/branches/{branch}/events", get(list_events))
        .route("/api/projects/{project}/branches/{branch}/params", get(get_params).post(put_params))
        .route("/api/projects/{project}/branches/{branch}/decisions", post(decide))
        .route("/api/projects/{project}/branches/{branch}/derived", get(derived))
        .route("/api/projects/{project}/branches/{branch}/derived/{kind}", get(derived_kind))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "algo_version": crate::domain::ALGO_VERSION}))
}

async fn list_projects(State(s): State<Shared>) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    Json(json!({"projects": engine::list_projects(&store).unwrap_or_default()}))
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    #[serde(default)]
    seed_ms: Option<i64>,
}

async fn create_project(State(s): State<Shared>, Json(b): Json<CreateBody>) -> impl IntoResponse {
    let seed = b.seed_ms.unwrap_or(sample::CASE_SEED);
    let mut store = s.store.lock().unwrap();
    match engine::create_project(&mut store, s.clock.as_ref(), &b.name, seed) {
        Ok(uid) => (
            StatusCode::CREATED,
            Json(json!({"project_uid": uid, "seed_ms": seed, "main_branch": "main"})),
        ),
    }
}

async fn get_sample(Path(name): Path<String>) -> impl IntoResponse {
    let content = match name.as_str() {
        "carrier-break-v1" => sample::generate_v1(),
        "carrier-break-v2-amendment" => sample::generate_v2_amendment(),
        "mini" => sample::mini_sample().to_string(),
        "bad" => sample::bad_sample().to_string(),
        _ => return err(StatusCode::NOT_FOUND, "未知样例名").into_response(),
    };
    ([("content-type", "text/plain; charset=utf-8")], content).into_response()
}

#[derive(Deserialize)]
struct ImportQuery {
    filename: Option<String>,
    kind: Option<String>,
    parent_batch: Option<String>,
}

#[derive(Deserialize)]
struct ImportBody {
    content: String,
    filename: Option<String>,
    kind: Option<String>,
    parent_batch: Option<String>,
}

async fn import_text(
    State(s): State<Shared>,
    Path(project): Path<String>,
    Query(q): Query<ImportQuery>,
    body: Option<Json<ImportBody>>,
) -> impl IntoResponse {
    let (content, filename, kind, parent) = match body {
        Some(Json(b)) => (
            b.content,
            b.filename.or(q.filename).unwrap_or_else(|| "pasted.gbsv1".to_string()),
            b.kind.or(q.kind).unwrap_or_else(|| "manual".to_string()),
            b.parent_batch.or(q.parent_batch),
        ),
        None => {
            // 纯文本 body 也支持（便于 curl --data-binary）。
            return err(
                StatusCode::BAD_REQUEST,
                "请使用 JSON: {\"content\": \"...\", \"filename\": \"...\"}",
            )
            .into_response();
        }
    };
    let mut store = s.store.lock().unwrap();
    let res = engine::commit_material(
        &mut store,
        s.clock.as_ref(),
        &project,
        "main",
        &filename,
        &content,
        &kind,
        parent.as_deref(),
    );
    match res {
        Ok(r) => (
            StatusCode::CREATED,
            Json(json!({
                "batch_uid": r.batch_uid,
                "accepted": r.accepted,
                "gen_no": r.gen_no,
                "diagnostics": r.diagnostics,
            })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_branches(State(s): State<Shared>, Path(project): Path<String>) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    Json(json!({"branches": engine::list_branches(&store, &project).unwrap_or_default()}))
}

#[derive(Deserialize)]
struct ForkBody {
    name: String,
    from_seq: Option<i64>,
    #[serde(default)]
    note: Option<String>,
}

async fn create_branch(
    State(s): State<Shared>,
    Path(project): Path<String>,
    Json(b): Json<ForkBody>,
) -> impl IntoResponse {
    let mut store = s.store.lock().unwrap();
    match engine::fork_branch(
        &mut store,
        s.clock.as_ref(),
        &project,
        "main",
        &b.name,
        b.from_seq,
        b.note.as_deref().unwrap_or(""),
    ) {
        Ok((uid, gen)) => (
            StatusCode::CREATED,
            Json(json!({"branch_uid": uid, "name": b.name, "base_gen": gen})),
        )
            .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn branch_detail(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    let events = engine::event_log(&store, &project, &branch).unwrap_or_default();
    let batches = engine::batch_list(&store, &project, &branch).unwrap_or_default();
    let gen = engine::current_gen(&store, &project, &branch).unwrap_or(0);
    let params = engine::effective_params(&store, &project, &branch).unwrap_or_default();
    Json(json!({
        "branch": branch,
        "current_gen": gen,
        "params": params,
        "events": events,
        "batches": batches,
    }))
}

async fn list_batches(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    Json(json!({"batches": engine::batch_list(&store, &project, &branch).unwrap_or_default()}))
}

async fn list_events(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    Json(json!({"events": engine::event_log(&store, &project, &branch).unwrap_or_default()}))
}

async fn get_params(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    Json(json!({"params": engine::effective_params(&store, &project, &branch).unwrap_or_default()}))
}

#[derive(Deserialize)]
struct ParamsBody {
    params: DetectParams,
    #[serde(default)]
    note: Option<String>,
}

async fn put_params(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
    Json(b): Json<ParamsBody>,
) -> impl IntoResponse {
    let mut store = s.store.lock().unwrap();
    match engine::revise_params(
        &mut store,
        s.clock.as_ref(),
        &project,
        &branch,
        &b.params,
        b.note.as_deref().unwrap_or(""),
    ) {
        Ok(gen) => Json(json!({"gen_no": gen})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct DecisionBody {
    candidate_uid: Option<String>,
    action: String,
    n_l1: Option<i64>,
    n_l2: Option<i64>,
    #[serde(default)]
    reason: Option<String>,
}

async fn decide(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
    Json(b): Json<DecisionBody>,
) -> impl IntoResponse {
    if !["ACCEPT", "REJECT", "REVOKE"].contains(&b.action.as_str()) {
        return err(StatusCode::BAD_REQUEST, "action 必须是 ACCEPT/REJECT/REVOKE").into_response();
    }
    let mut store = s.store.lock().unwrap();
    match engine::record_decision(
        &mut store,
        s.clock.as_ref(),
        &project,
        &branch,
        b.candidate_uid.as_deref(),
        &b.action,
        b.n_l1,
        b.n_l2,
        b.reason.as_deref().unwrap_or(""),
    ) {
        Ok(uid) => (StatusCode::CREATED, Json(json!({"event_uid": uid}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct DerivedQuery {
    gen: Option<i64>,
}

async fn derived(
    State(s): State<Shared>,
    Path((project, branch)): Path<(String, String)>,
    Query(q): Query<DerivedQuery>,
) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    match engine::derived_view(&store, &project, &branch, q.gen, None) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn derived_kind(
    State(s): State<Shared>,
    Path((project, branch, kind)): Path<(String, String, String)>,
    Query(q): Query<DerivedQuery>,
) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    match engine::derived_view(&store, &project, &branch, q.gen, Some(&kind)) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn export(State(s): State<Shared>, Path(project): Path<String>) -> impl IntoResponse {
    let store = s.store.lock().unwrap();
    match bundle::export_project(&store, &project) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn import_bundle(
    State(s): State<Shared>,
    Path(project): Path<String>,
    bytes: Bytes,
) -> impl IntoResponse {
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("bundle JSON 解析失败: {e}")).into_response(),
    };
    let uid_in_bundle = value["project"]["project_uid"].as_str().unwrap_or("");
    if uid_in_bundle != project {
        return err(
            StatusCode::BAD_REQUEST,
            "URL 中的 project 与 bundle 内 project_uid 不一致（身份不可变）",
        )
        .into_response();
    }
    let mut store = s.store.lock().unwrap();
    match bundle::import_bundle(&mut store, &value, s.clock.as_ref()) {
        Ok(uid) => Json(json!({"project_uid": uid, "imported": true})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
