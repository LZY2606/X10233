use crate::model::DecisionRequest;
use crate::store::Store;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
}

pub fn router(store: Arc<Store>) -> Router {
    let state = AppState { store };
    Router::new()
        .route("/", get(index))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{id}", get(get_project))
        .route("/api/projects/{id}/source", post(revise_source))
        .route("/api/projects/{id}/rules", post(revise_rules))
        .route("/api/projects/{id}/decisions", post(decide))
        .route("/api/projects/{id}/export", get(export_project))
        .route("/api/import", post(import_project))
        .route("/api/case", get(bundled_case))
        .with_state(state)
}

async fn index() -> Response {
    Html(include_str!("../static/index.html")).into_response()
}

async fn list_projects(State(state): State<AppState>) -> Response {
    json_response(state.store.projects())
}

async fn get_project(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    json_response(state.store.project(&id))
}

async fn create_project(State(state): State<AppState>, Json(body): Json<CreateProject>) -> Response {
    let status = if body.status.trim().is_empty() { include_str!("../cases/carrier-break-station/satellite-status.txt").to_string() } else { body.status };
    json_response(state.store.create_project(&body.name, &body.observations, &status))
}

async fn revise_source(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<SourceRevision>) -> Response {
    json_response(state.store.revise_sources(&id, "source", &body.observations, &body.status, &body.reason))
}

async fn revise_rules(State(state): State<AppState>, Path(id): Path<String>, Json(body): Json<RuleRevision>) -> Response {
    json_response(state.store.revise_rules(&id, body.params(), &body.reason))
}

async fn decide(State(state): State<AppState>, Path(id): Path<String>, Json(mut body): Json<DecisionRequest>) -> Response {
    if body.parent_run_id == "active" {
        if let Ok(project) = state.store.project(&id) {
            body.parent_run_id = project["active_run_id"].as_str().unwrap_or_default().to_string();
        }
    }
    json_response(state.store.decide(&id, &body))
}

async fn export_project(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.store.export_project(&id) {
        Ok(value) => ([(header::CONTENT_TYPE, "application/json; charset=utf-8")], Json(value)).into_response(),
        Err(error) => api_error(error),
    }
}

async fn import_project(State(state): State<AppState>, Json(bundle): Json<Value>) -> Response {
    json_response(state.store.import_project(bundle))
}

async fn bundled_case() -> Response {
    ([(header::CONTENT_TYPE, "application/json; charset=utf-8")], Json(json!({
        "name": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
        "observations": include_str!("../cases/carrier-break-station/observations-v1.rnx"),
        "status": include_str!("../cases/carrier-break-station/satellite-status.txt"),
        "source_revision": include_str!("../cases/carrier-break-station/observations-v2.rnx"),
        "parameter_revision": {"integer_search_radius": 6, "reason": "扩大整数搜索半径，检验多检测器候选差异"},
        "source_revision_reason": "只修订 G01 L1 原始 SNR，其他站星频率材料保持不变"
    }))).into_response()
}

#[derive(Deserialize)]
struct CreateProject {
    name: String,
    observations: String,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct SourceRevision {
    observations: String,
    #[serde(default)]
    status: String,
    reason: String,
}

#[derive(Deserialize)]
struct RuleRevision {
    reason: String,
    integer_search_radius: Option<i64>,
    td_sigma_cycles: Option<f64>,
    mw_sigma_cycles: Option<f64>,
    iono_tolerance_m: Option<f64>,
    lli_low_snr_dbhz: Option<f64>,
}

impl RuleRevision {
    fn params(&self) -> crate::model::RuleParams {
        let default = crate::model::RuleParams::default();
        crate::model::RuleParams {
            integer_search_radius: self.integer_search_radius.unwrap_or(default.integer_search_radius),
            td_sigma_cycles: self.td_sigma_cycles.unwrap_or(default.td_sigma_cycles),
            mw_sigma_cycles: self.mw_sigma_cycles.unwrap_or(default.mw_sigma_cycles),
            iono_tolerance_m: self.iono_tolerance_m.unwrap_or(default.iono_tolerance_m),
            lli_low_snr_dbhz: self.lli_low_snr_dbhz.unwrap_or(default.lli_low_snr_dbhz),
        }
    }
}

fn json_response(result: Result<Value, String>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => api_error(error),
    }
}

fn api_error(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response()
}
