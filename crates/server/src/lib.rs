//! Local warm API server (`whycodes serve`).
//!
//! Keeps a fully configured [`Agent`] (MCP, plugins, workspace index, config)
//! alive across HTTP clients so reconnects skip cold startup. Sessions are
//! held in-process and optionally persisted to the same SQLite DB as the TUI.

pub mod heap;
mod keep_warm;
pub mod perm;
pub mod routes;
pub mod v1;

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use whycodes_agent::agent::Agent;
use whycodes_agent::events::CancelFlag;
use whycodes_config::Config;
use whycodes_session::session::Session;

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
    /// Max agent steps per chat request (`None` = unlimited, as in the TUI).
    pub max_turns: Option<usize>,
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
    /// Bearer token every `/api` and `/v1` request must carry. Loopback is
    /// not a boundary: any web page the user opens can reach 127.0.0.1, and
    /// these routes run tools. Empty refuses every request.
    pub auth_token: Arc<str>,
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

    /// Drop sessions untouched for `idle` from memory, saving each first; the
    /// next request loads them again from the database. Skips a session that
    /// a request, turn or keep-alive still holds, an empty one, and one that
    /// could not be saved. Returns how many were dropped.
    pub fn evict_idle_sessions(&self, idle: std::time::Duration) -> usize {
        let Ok(mut map) = self.sessions.lock() else {
            return 0;
        };
        let before = map.len();
        map.retain(|_, handle| !evictable(handle, idle));
        before - map.len()
    }

    pub fn list_session_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Path to the shared whycodes SQLite file (same as CLI/TUI).
    pub fn db_path() -> PathBuf {
        whycodes_core::paths::data_dir().join("whycodes.db")
    }

    pub fn open_db() -> Option<whycodes_storage::db::Database> {
        whycodes_storage::db::Database::open(&Self::db_path().to_string_lossy()).ok()
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
                    whycodes_agent::events::request_cancel(flag);
                    true
                } else {
                    false
                }
            }
            Err(_poisoned) => false,
        }
    }
}

/// Idle long enough, held by nobody else, non-empty, and saved just now.
fn evictable(handle: &SessionHandle, idle: std::time::Duration) -> bool {
    let Ok(session) = handle.try_lock() else {
        return false;
    };
    let quiet = chrono::Utc::now() - session.updated_at;
    Arc::strong_count(handle) == 1
        && !session.messages.is_empty()
        && quiet.to_std().is_ok_and(|q| q >= idle)
        && routes::save_session(&session).is_ok()
}

/// How often the daemon looks for idle sessions to drop from memory.
pub const SESSION_EVICT_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// A session untouched this long leaves memory (it stays in the database).
pub const SESSION_IDLE_EVICT: std::time::Duration = std::time::Duration::from_secs(600);

/// Run [`AppState::evict_idle_sessions`] every `every` for the daemon's life.
pub fn spawn_session_eviction(
    state: AppState,
    every: std::time::Duration,
    idle: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            heap::after_eviction(state.evict_idle_sessions(idle));
        }
    })
}

/// Env var that fixes the daemon's bearer token (scripts, SDK launches);
/// unset, `serve` makes a random one.
pub const AUTH_TOKEN_ENV: &str = "WHYCODES_SERVE_TOKEN";

/// The token `serve` requires: [`AUTH_TOKEN_ENV`] when set and non-blank,
/// else 64 random hex chars (two v4 UUIDs, 244 random bits).
pub fn auth_token_from(env: Option<String>) -> String {
    env.map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )
        })
}

/// Whether `request` carries `Authorization: Bearer <token>` for `want`.
/// Constant time in the token length; an empty `want` matches nothing.
pub fn bearer_matches(request: &axum::http::HeaderMap, want: &str) -> bool {
    let Some(given) = request
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    !want.is_empty()
        && given.len() == want.len()
        && given
            .bytes()
            .zip(want.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if bearer_matches(request.headers(), &state.auth_token) {
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        "missing or wrong bearer token (see .whycodes/serve.lock or WHYCODES_SERVE_TOKEN)",
    )
        .into_response()
}

/// `/api` and `/v1` need [`AppState::auth_token`]; only the read-only
/// share pages (`/s/...`, opened in a browser) do not. No CORS layer: the
/// clients are the TUI and the SDKs, never a web page.
pub fn create_router(state: AppState) -> Router {
    let shares = Router::new().route("/s/{id}", get(routes::share_dispatch));
    Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/tools", get(routes::list_tools))
        .route("/api/models", get(routes::list_models))
        .route("/api/sessions", get(routes::list_sessions))
        .route("/api/session/new", post(routes::create_session))
        .route("/api/session/{id}", get(routes::get_session))
        .route(
            "/api/session/{id}/messages",
            get(routes::get_session_messages),
        )
        .route("/api/session/{id}/chat", post(routes::chat))
        .route("/api/session/{id}/steer", post(routes::steer))
        .route("/v1/health", get(v1::health))
        .route(
            "/v1/sessions",
            get(v1::list_sessions).post(v1::create_session),
        )
        .route("/v1/sessions/{id}", get(v1::get_session))
        .route("/v1/sessions/{id}/run", post(v1::run))
        .route("/v1/sessions/{id}/cancel", post(v1::cancel))
        .route("/v1/sessions/{id}/permission", post(v1::permission))
        .route("/v1/sessions/{id}/question", post(v1::question))
        .route("/v1/sessions/{id}/messages", get(v1::history))
        .route("/v1/sessions/{id}/model", post(v1::set_model))
        .route("/v1/sessions/{id}/rename", post(v1::rename))
        .route("/v1/sessions/{id}/rewind", post(v1::rewind))
        .route("/v1/sessions/{id}/compact", post(v1::compact))
        .route("/v1/models", get(v1::list_models))
        .route("/api/shares", get(routes::list_shares))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        // Single param route: id may be bare, `foo.json`, or `foo.md`
        // (axum rejects overlapping `/s/{id}` + `/s/{id}.json`).
        .merge(shares)
        .with_state(state)
}

#[cfg(test)]
mod http_tests;
