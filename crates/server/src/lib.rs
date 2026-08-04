pub mod routes;

use axum::{
    Router,
    routing::{get, post},
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use whycode_agent::agent::Agent;
use whycode_config::Config;

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<Agent>,
    pub config: Arc<Config>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/tools", get(routes::list_tools))
        .route("/api/models", get(routes::list_models))
        .route("/api/sessions", get(routes::list_sessions))
        .route("/api/session/new", post(routes::create_session))
        .route("/api/session/:id/chat", post(routes::chat))
        // OpenCode-style share links (local)
        .route("/s/:id", get(routes::share_view))
        .route("/s/:id.json", get(routes::share_json))
        .route("/s/:id.md", get(routes::share_markdown))
        .route("/api/shares", get(routes::list_shares))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
