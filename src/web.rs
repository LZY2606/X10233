use crate::Store;
use axum::{routing::get, Router};
use std::sync::Arc;

pub fn router(_store: Arc<Store>) -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}
