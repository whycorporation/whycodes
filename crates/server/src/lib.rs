//! Local warm API server (`whycode serve`).
//!
//! Keeps a fully configured [`Agent`] (MCP, plugins, workspace index, config)
//! alive across HTTP clients so reconnects skip cold startup. Sessions are
//! held in-process and optionally persisted to the same SQLite DB as the TUI.

pub mod routes;

use axum::{
    Router,
    routing::{get, post},
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tower_http::cors::CorsLayer;
use whycode_agent::agent::Agent;
use whycode_config::Config;
use whycode_session::session::Session;

/// One live session handle (async mutex so concurrent chats serialize per id).
pub type SessionHandle = Arc<AsyncMutex<Session>>;

/// Shared state for the warm daemon.
#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<Agent>,
    pub config: Arc<Config>,
    /// Project root used for new sessions and the workspace file index.
    pub project_dir: PathBuf,
    /// In-process session map (warm across requests).
    pub sessions: Arc<std::sync::Mutex<HashMap<String, SessionHandle>>>,
    /// Max agent steps per chat request.
    pub max_turns: usize,
    /// True when MCP / plugins were loaded at startup.
    pub mcp_warm: bool,
    /// True when a workspace file index was started.
    pub index_warm: bool,
    /// When the server process started (for `/api/health` uptime).
    pub started_at: std::time::Instant,
}

impl AppState {
    pub fn insert_session(&self, session: Session) -> SessionHandle {
        let id = session.id.clone();
        let handle = Arc::new(AsyncMutex::new(session));
        if let Ok(mut map) = self.sessions.lock() {
            map.insert(id, Arc::clone(&handle));
        }
        handle
    }

    pub fn get_session(&self, id: &str) -> Option<SessionHandle> {
        self.sessions.lock().ok()?.get(id).cloned()
    }

    pub fn list_session_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Path to the shared whycode SQLite file (same as CLI/TUI).
    pub fn db_path() -> Option<PathBuf> {
        Config::data_dir().ok().map(|d| d.join("whycode.db"))
    }

    pub fn open_db() -> Option<whycode_storage::db::Database> {
        let path = Self::db_path()?;
        whycode_storage::db::Database::open(&path.to_string_lossy()).ok()
    }
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/tools", get(routes::list_tools))
        .route("/api/models", get(routes::list_models))
        .route("/api/sessions", get(routes::list_sessions))
        .route("/api/session/new", post(routes::create_session))
        .route("/api/session/:id", get(routes::get_session))
        .route(
            "/api/session/:id/messages",
            get(routes::get_session_messages),
        )
        .route("/api/session/:id/chat", post(routes::chat))
        // Single param route: id may be bare, `foo.json`, or `foo.md`
        // (axum rejects overlapping `/s/:id` + `/s/:id.json`).
        .route("/s/:id", get(routes::share_dispatch))
        .route("/api/shares", get(routes::list_shares))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
