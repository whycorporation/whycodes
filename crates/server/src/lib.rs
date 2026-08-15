//! Local warm API server (`whycode serve`).
//!
//! Keeps a fully configured [`Agent`] (MCP, plugins, workspace index, config)
//! alive across HTTP clients so reconnects skip cold startup. Sessions are
//! held in-process and optionally persisted to the same SQLite DB as the TUI.

pub mod perm;
pub mod routes;
pub mod v1;

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
use whycode_agent::events::CancelFlag;
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
    /// Per-session cancel flags for in-flight `/v1` runs.
    pub cancel_flags: Arc<std::sync::Mutex<HashMap<String, CancelFlag>>>,
    /// Permission asks for `/v1` (and auto-approve wrapper for `/api`).
    pub perm: Arc<perm::PermHub>,
    /// Per-session (provider, model) override from `POST /v1/sessions/:id/model`.
    pub session_route: Arc<std::sync::Mutex<HashMap<String, (String, String)>>>,
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

    pub fn register_cancel(&self, session_id: &str, flag: CancelFlag) {
        if let Ok(mut map) = self.cancel_flags.lock() {
            map.insert(session_id.to_string(), flag);
        }
    }

    pub fn take_cancel(&self, session_id: &str) -> Option<CancelFlag> {
        self.cancel_flags.lock().ok()?.remove(session_id)
    }

    pub fn request_cancel(&self, session_id: &str) -> bool {
        match self.cancel_flags.lock() {
            Ok(map) => {
                if let Some(flag) = map.get(session_id) {
                    whycode_agent::events::request_cancel(flag);
                    true
                } else {
                    false
                }
            }
            Err(_poisoned) => false,
        }
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
        .route("/v1/health", get(v1::health))
        .route(
            "/v1/sessions",
            get(v1::list_sessions).post(v1::create_session),
        )
        .route("/v1/sessions/:id", get(v1::get_session))
        .route("/v1/sessions/:id/run", post(v1::run))
        .route("/v1/sessions/:id/cancel", post(v1::cancel))
        .route("/v1/sessions/:id/permission", post(v1::permission))
        .route("/v1/sessions/:id/question", post(v1::question))
        .route("/v1/sessions/:id/messages", get(v1::history))
        .route("/v1/sessions/:id/model", post(v1::set_model))
        .route("/v1/sessions/:id/rename", post(v1::rename))
        .route("/v1/sessions/:id/rewind", post(v1::rewind))
        .route("/v1/sessions/:id/compact", post(v1::compact))
        .route("/v1/models", get(v1::list_models))
        // Single param route: id may be bare, `foo.json`, or `foo.md`
        // (axum rejects overlapping `/s/:id` + `/s/:id.json`).
        .route("/s/:id", get(routes::share_dispatch))
        .route("/api/shares", get(routes::list_shares))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Build a minimal, fully in-memory [`AppState`] for unit tests. Nothing here
/// touches the user's config, database, or workspace.
#[cfg(test)]
pub(crate) fn test_state() -> AppState {
    use whycode_core::types::{AgentInfo, AgentMode, PermissionSet};

    AppState {
        agent: Arc::new(Agent::new(AgentInfo {
            name: "test".into(),
            description: "test agent".into(),
            mode: AgentMode::Primary,
            permission: PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        })),
        config: Arc::new(Config::default()),
        project_dir: std::env::temp_dir(),
        sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        max_turns: 5,
        mcp_warm: false,
        index_warm: false,
        started_at: std::time::Instant::now(),
        cancel_flags: Arc::new(std::sync::Mutex::new(HashMap::new())),
        perm: perm::PermHub::new(),
        session_route: Arc::new(std::sync::Mutex::new(HashMap::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycode_agent::events::new_cancel_flag;
    use whycode_session::session::Session;

    #[test]
    fn session_round_trip_through_the_warm_map() {
        let state = test_state();

        let s1 = Session::new("/tmp".into(), "sys".into());
        let id1 = s1.id.clone();
        state.insert_session(s1);
        assert!(state.get_session(&id1).is_some());
        assert_eq!(state.list_session_ids(), vec![id1.clone()]);

        let s2 = Session::new("/tmp".into(), "sys".into());
        let id2 = s2.id.clone();
        state.insert_session(s2);
        let mut ids = state.list_session_ids();
        ids.sort();
        let mut want = vec![id1, id2];
        want.sort();
        assert_eq!(ids, want);

        assert!(state.get_session("missing").is_none());
    }

    #[test]
    fn cancel_flags_register_take_and_request() {
        let state = test_state();
        assert!(!state.request_cancel("s1"));
        assert!(state.take_cancel("s1").is_none());

        state.register_cancel("s1", new_cancel_flag());
        assert!(state.request_cancel("s1"));
        assert!(state.take_cancel("s1").is_some());
        assert!(state.take_cancel("s1").is_none());
        assert!(!state.request_cancel("s1"));
    }
}
