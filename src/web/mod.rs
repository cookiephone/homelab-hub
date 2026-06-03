//! HTTP layer: router, request handlers and view models.

mod routes;
mod view;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Build the application router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(routes::dashboard))
        .route("/partials/dashboard", get(routes::partial))
        .route("/events", get(routes::events))
        .route("/services/:id", get(routes::detail))
        .route("/healthz", get(routes::healthz))
        .route("/api/status", get(routes::api_status))
        .route("/api/services/:id/history", get(routes::api_history))
        .route("/api/services/:id/check", post(routes::api_check_now))
        .route("/static/*path", get(routes::static_asset))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
