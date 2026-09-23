use std::sync::{Arc, Mutex};

use axum::Router;

use crate::service::ProjectService;

pub fn build_router(_state: Arc<Mutex<ProjectService>>) -> Router {
    Router::new()
}
