use axum::{extract::{Path, State}, response::sse::{Event, Sse}, Json};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use whycode_session::session::Session;

use crate::AppState;

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}))
}

pub async fn list_tools(State(_state): State<AppState>) -> Json<serde_json::Value> {
    let tools: Vec<_> = whycode_tools::executor::ToolExecutor::new()
        .get_definitions(&whycode_core::types::PermissionSet::default())
        .iter()
        .map(|t| serde_json::json!({"name": t.name, "description": t.description}))
        .collect();
    Json(serde_json::json!({"tools": tools}))
}

pub async fn list_models(State(state): State<AppState>) -> Json<serde_json::Value> {
    let models: Vec<_> = state.config.models.values()
        .map(|m| serde_json::json!({"id": m.model_id, "provider": m.provider_id}))
        .collect();
    Json(serde_json::json!({"models": models}))
}

pub async fn list_sessions(State(_state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"sessions": []}))
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub project: Option<String>,
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Json<serde_json::Value> {
    let project = std::path::PathBuf::from(req.project.unwrap_or_else(|| ".".to_string()));
    let session = Session::new(project, state.agent.system_prompt());
    Json(serde_json::json!({"session_id": session.id}))
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

pub async fn chat(
    State(_state): State<AppState>,
    Path(_session_id): Path<String>,
    Json(_req): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = futures::stream::once(async {
        Ok(Event::default()
            .data(serde_json::json!({"type": "text_delta", "text": "Server mode: send a message to get started"}).to_string()))
    });
    Sse::new(stream)
}
