//! HTTP handlers for the warm local server.

use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::sse::{Event, KeepAlive, Sse},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use whycodes_agent::Agent;
use whycodes_agent::events::TurnEvent;
use whycodes_session::session::Session;

use crate::AppState;
use crate::perm::{RUN, RunScope};

/// Resolve share file directories: project .whycodes/shares + global data dir shares.
fn share_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(whycodes_core::project_dir(&cwd).join("shares"));
    }
    if let Ok(data) = whycodes_config::Config::data_dir() {
        dirs.push(data.join("shares"));
    }
    dirs
}

fn find_share_file(id: &str, ext: &str) -> Option<PathBuf> {
    find_share_file_in(&share_search_dirs(), id, ext)
}

fn find_share_file_in(dirs: &[PathBuf], id: &str, ext: &str) -> Option<PathBuf> {
    let id = id.trim_end_matches(".json").trim_end_matches(".md");
    for dir in dirs {
        let p = dir.join(format!("{id}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub(crate) fn system_prompt_for(agent: &Agent, project: &std::path::Path) -> String {
    Agent::with_agents_md(
        &Agent::with_runtime_context(&agent.system_prompt()),
        project,
    )
}

/// Env → config api_key → OAuth store (mirrors CLI `get_api_key`).
pub(crate) async fn resolve_api_key(
    provider: &str,
    config: &whycodes_config::Config,
) -> Option<String> {
    let env_var = format!("{}_API_KEY", provider.to_uppercase());
    if let Ok(key) = std::env::var(&env_var)
        && !key.is_empty()
    {
        whycodes_llm::oauth_refresh::unregister(provider);
        return Some(key);
    }
    if let Some(pc) = config.get_provider(provider)
        && let Some(key) = &pc.api_key
        && !key.is_empty()
    {
        whycodes_llm::oauth_refresh::unregister(provider);
        return Some(key.clone());
    }
    if provider == "openai"
        && let Ok(key) = std::env::var("OPENAI_API_KEY")
        && !key.is_empty()
    {
        whycodes_llm::oauth_refresh::unregister(provider);
        return Some(key);
    }
    if whycodes_auth::providers::supports_oauth(provider)
        && let Ok(data_dir) = whycodes_config::Config::data_dir()
        && let Some(token) = whycodes_auth::providers::access_token(provider, &data_dir).await
    {
        whycodes_llm::oauth_refresh::register(provider, data_dir);
        return Some(token);
    }
    None
}

pub(crate) fn default_provider_model(config: &whycodes_config::Config) -> (String, String) {
    if let Some(dm) = &config.default_model {
        return (dm.provider_id.clone(), dm.model_id.clone());
    }
    // First configured provider + a placeholder model id.
    if let Some((name, _)) = config.providers.iter().next() {
        return (name.clone(), "default".into());
    }
    ("anthropic".into(), "claude-sonnet-4-20250514".into())
}

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let live = state.list_session_ids().len();
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "warm": {
            "mcp": state.mcp_warm,
            "index": state.index_warm,
            "sessions_in_memory": live,
        },
        "project": state.project_dir.display().to_string(),
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "max_turns": state.max_turns,
    }))
}

pub async fn list_tools(State(state): State<AppState>) -> Json<serde_json::Value> {
    // Prefer the warm agent's profile-aware catalog (core tools + activated).
    let tools: Vec<_> = whycodes_tools::executor::ToolExecutor::new()
        .get_definitions_profile(
            &state.agent.info.permission,
            whycodes_tools::ToolProfile::Core,
        )
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
            })
        })
        .collect();
    Json(serde_json::json!({
        "tools": tools,
        "profile": "core",
        "note": "Catalog from warm process; MCP tools may be attached on the agent.",
    }))
}

pub async fn list_models(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut models: Vec<_> = state
        .config
        .models
        .values()
        .map(|m| {
            serde_json::json!({
                "id": m.model_id,
                "provider": m.provider_id,
            })
        })
        .collect();
    if models.is_empty()
        && let Some(dm) = &state.config.default_model
    {
        models.push(serde_json::json!({
            "id": dm.model_id,
            "provider": dm.provider_id,
            "default": true,
        }));
    }
    let providers: Vec<_> = state
        .config
        .providers
        .keys()
        .map(|k| serde_json::json!({ "id": k }))
        .collect();
    Json(serde_json::json!({ "models": models, "providers": providers }))
}

pub async fn list_sessions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut sessions = Vec::new();

    // Warm in-memory first.
    let ids = state.list_session_ids();
    for id in &ids {
        if let Some(handle) = state.get_session(id) {
            let s = handle.lock().await;
            sessions.push(serde_json::json!({
                "id": s.id,
                "title": s.title,
                "project": s.project_path.display().to_string(),
                "messages": s.messages.len(),
                "updated_at": s.updated_at.to_rfc3339(),
                "source": "memory",
            }));
        }
    }

    // Merge SQLite rows not already live (same DB as TUI).
    if let Some(db) = AppState::open_db()
        && let Ok(rows) = db.list_sessions()
    {
        for row in rows {
            if ids.iter().any(|i| i == &row.id) {
                continue;
            }
            sessions.push(serde_json::json!({
                "id": row.id,
                "title": row.title,
                "project": row.project_path,
                "updated_at": row.updated_at,
                "source": "db",
            }));
        }
    }

    Json(serde_json::json!({ "sessions": sessions }))
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub project: Option<String>,
    /// When true (default), persist to SQLite immediately.
    pub persist: Option<bool>,
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Json<serde_json::Value> {
    let project = req
        .project
        .map(PathBuf::from)
        .unwrap_or_else(|| state.project_dir.clone());
    let prompt = system_prompt_for(&state.agent, &project);
    let session = Session::new(project, prompt);
    let id = session.id.clone();
    let title = session.title.clone();
    let persist = req.persist.unwrap_or(true);
    if persist
        && let Some(db) = AppState::open_db()
        && let Err(e) = session.save_to_db(&db)
    {
        tracing::warn!(error = %e, "serve: failed to persist new session");
    }
    state.insert_session(session);
    Json(serde_json::json!({
        "session_id": id,
        "title": title,
        "persisted": persist,
    }))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let handle = load_or_get_session(&state, &id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let s = handle.lock().await;
    Ok(Json(serde_json::json!({
        "id": s.id,
        "title": s.title,
        "project": s.project_path.display().to_string(),
        "messages": s.messages.len(),
        "token_estimate": s.token_count(),
        "updated_at": s.updated_at.to_rfc3339(),
    })))
}

pub async fn get_session_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let handle = load_or_get_session(&state, &id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let s = handle.lock().await;
    Ok(Json(serde_json::json!({
        "id": s.id,
        "title": s.title,
        "project": s.project_path.display().to_string(),
        "messages": s.messages,
    })))
}

/// Load from memory, else SQLite into the warm map.
pub(crate) async fn load_or_get_session(
    state: &AppState,
    id: &str,
) -> Option<crate::SessionHandle> {
    if let Some(h) = state.get_session(id) {
        return Some(h);
    }
    let db = AppState::open_db()?;
    let session = Session::load_from_db(&db, id).ok().flatten()?;
    Some(state.insert_session(session))
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub max_turns: Option<usize>,
}

pub async fn chat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> Result<Response, StatusCode> {
    if req.message.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let handle = load_or_get_session(&state, &session_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let (def_provider, def_model) = default_provider_model(&state.config);
    let provider = req.provider.unwrap_or(def_provider);
    let model = req.model.unwrap_or(def_model);
    let api_key = match req.api_key.filter(|k| !k.is_empty()) {
        Some(k) => k,
        None => resolve_api_key(&provider, &state.config)
            .await
            .unwrap_or_default(),
    };

    let keep = KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("ping");

    if api_key.is_empty() && whycodes_llm::provider_requires_api_key(&provider) {
        let msg = format!(
            "No API key for provider `{provider}`. Set {}_API_KEY, config, or `whycodes auth login`.",
            provider.to_uppercase()
        );
        let stream = async_stream::stream! {
            let payload = serde_json::json!({
                "type": "error",
                "message": msg,
            });
            yield Ok::<_, Infallible>(Event::default().data(payload.to_string()));
            yield Ok(Event::default().data(
                serde_json::json!({"type": "done"}).to_string()
            ));
        };
        return Ok(Sse::new(stream).keep_alive(keep).into_response());
    }

    let max_turns = req.max_turns.or(state.max_turns).map(|n| n.max(1));
    let agent = Arc::clone(&state.agent);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let scope = RunScope {
        session_id: session_id.clone(),
        auto_approve: true,
        hub: Arc::clone(&state.perm),
    };

    // Run the agent turn on a task so the SSE stream can forward events.
    let chat_task = tokio::spawn(async move {
        let result = RUN
            .scope(scope, async {
                let mut session = handle.lock().await;
                session.add_user_message(&req.message);
                agent
                    .run_turn_with_events(
                        &mut session,
                        &provider,
                        &model,
                        &api_key,
                        max_turns,
                        Some(tx.clone()),
                        None,
                    )
                    .await
            })
            .await;
        // Persist after the turn (best-effort).
        if let Some(db) = AppState::open_db()
            && let Err(e) = handle.lock().await.save_to_db(&db)
        {
            tracing::warn!(error = %e, "serve: failed to save session after chat");
        }
        match result {
            Ok(text) => {
                emit_status(&tx, format!("done:{}chars", text.len()));
                emit_status(&tx, "__whycodes_done__");
            }
            Err(e) => {
                emit_status(&tx, format!("error:{e}"));
                emit_status(&tx, "__whycodes_done__");
            }
        }
    });

    let stream = async_stream::stream! {
        while let Some(ev) = rx.recv().await {
            if let TurnEvent::Status(ref s) = ev
                && s == "__whycodes_done__"
            {
                break;
            }
            if let Some(payload) = turn_event_json(&ev) {
                yield Ok::<_, Infallible>(Event::default().data(payload.to_string()));
            }
        }
        // Ensure the worker finishes (avoids detach on client drop mid-flight).
        let _ = chat_task.await;
        yield Ok(Event::default().data(
            serde_json::json!({"type": "done"}).to_string()
        ));
    };

    Ok(Sse::new(stream).keep_alive(keep).into_response())
}

fn emit_status(tx: &tokio::sync::mpsc::UnboundedSender<TurnEvent>, status: impl Into<String>) {
    if let Err(e) = tx.send(TurnEvent::Status(status.into())) {
        tracing::debug!(error = %e, "serve: chat event channel closed");
    }
}

fn turn_event_json(ev: &TurnEvent) -> Option<serde_json::Value> {
    Some(match ev {
        TurnEvent::TextDelta(text) => {
            serde_json::json!({"type": "text_delta", "text": text})
        }
        TurnEvent::ThinkingDelta(text) => {
            serde_json::json!({"type": "thinking_delta", "text": text})
        }
        TurnEvent::ToolStart { id, name, input } => {
            serde_json::json!({
                "type": "tool_start",
                "id": id,
                "name": name,
                "input": input,
            })
        }
        TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        } => {
            serde_json::json!({
                "type": "tool_end",
                "id": id,
                "content": content,
                "is_error": is_error,
            })
        }
        TurnEvent::Usage(u) => {
            serde_json::json!({
                "type": "usage",
                "input_tokens": u.input_tokens,
                "output_tokens": u.output_tokens,
                "cache_read_input_tokens": u.cache_read_input_tokens,
                "cache_creation_input_tokens": u.cache_creation_input_tokens,
            })
        }
        TurnEvent::Status(s) if s.starts_with("done:") || s.starts_with("error:") => {
            let msg = s.strip_prefix("error:")?;
            serde_json::json!({"type": "error", "message": msg})
        }
        TurnEvent::Status(s) => {
            serde_json::json!({"type": "status", "message": s})
        }
        TurnEvent::Cancelled => {
            serde_json::json!({"type": "cancelled"})
        }
        TurnEvent::Intent {
            kind,
            confidence,
            badge,
            notice_kind,
            notice,
        } => {
            serde_json::json!({
                "type": "intent",
                "kind": kind,
                "confidence": confidence,
                "badge": badge,
                "notice_kind": notice_kind,
                "notice": notice,
            })
        }
        TurnEvent::FileConflict {
            path,
            claimant,
            owner,
        } => {
            serde_json::json!({
                "type": "file_conflict",
                "path": path,
                "claimant": claimant,
                "owner": owner,
            })
        }
        TurnEvent::SwarmStatus {
            active,
            total,
            message,
        } => {
            serde_json::json!({
                "type": "swarm_status",
                "active": active,
                "total": total,
                "message": message,
            })
        }
        TurnEvent::Background {
            id,
            status,
            summary,
        } => {
            serde_json::json!({
                "type": "background",
                "id": id,
                "status": status,
                "summary": summary,
            })
        }
        TurnEvent::EnqueuePrompt { text } => {
            serde_json::json!({
                "type": "enqueue_prompt",
                "text": text,
            })
        }
        TurnEvent::SwarmMessage { from, to, text } => {
            serde_json::json!({
                "type": "swarm_message",
                "from": from,
                "to": to,
                "text": text,
            })
        }
        TurnEvent::FileStale {
            path,
            reader,
            writer,
        } => {
            serde_json::json!({
                "type": "file_stale",
                "path": path,
                "reader": reader,
                "writer": writer,
            })
        }
        TurnEvent::PermissionAsk {
            request_id,
            tool_name,
            detail,
        } => {
            serde_json::json!({
                "type": "permission_request",
                "request_id": request_id,
                "tool_name": tool_name,
                "detail": detail,
            })
        }
        TurnEvent::QuestionAsk {
            request_id,
            questions,
        } => {
            serde_json::json!({
                "type": "question_request",
                "request_id": request_id,
                "questions": questions,
            })
        }
        TurnEvent::Subagent {
            id,
            kind,
            description,
            status,
            activity,
            elapsed_ms,
            ..
        } => {
            serde_json::json!({
                "type": "subagent",
                "id": id,
                "kind": kind,
                "description": description,
                "status": status,
                "activity": activity,
                "elapsed_ms": elapsed_ms,
            })
        }
        TurnEvent::Todos { todos } => {
            serde_json::json!({
                "type": "todos",
                "count": todos.len(),
                "done": whycodes_core::todo::terminal_count(todos),
            })
        }
        TurnEvent::Panel(update) => match update {
            whycodes_core::PanelUpdate::Clear => {
                serde_json::json!({"type": "panel", "action": "clear"})
            }
            whycodes_core::PanelUpdate::File { path, .. } => {
                serde_json::json!({"type": "panel", "action": "file", "path": path})
            }
            whycodes_core::PanelUpdate::Diff { path, .. } => {
                serde_json::json!({"type": "panel", "action": "diff", "path": path})
            }
            whycodes_core::PanelUpdate::Mermaid { .. } => {
                serde_json::json!({"type": "panel", "action": "mermaid"})
            }
        },
    })
}

// ── share routes (unchanged behaviour) ──────────────────────────────────────

pub async fn list_shares() -> Json<serde_json::Value> {
    let mut shares = Vec::new();
    for dir in share_search_dirs() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) == Some("json")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                {
                    shares.push(serde_json::json!({
                        "id": stem,
                        "json": format!("/s/{stem}.json"),
                        "md": format!("/s/{stem}.md"),
                        "view": format!("/s/{stem}"),
                        "path": path.display().to_string(),
                    }));
                }
            }
        }
    }
    Json(serde_json::json!({ "shares": shares }))
}

/// Dispatch share by extension embedded in `:id` (`uuid`, `uuid.json`, `uuid.md`).
pub async fn share_dispatch(Path(id): Path<String>) -> Response {
    if id.ends_with(".json") {
        return share_json(Path(id)).await;
    }
    if id.ends_with(".md") {
        return share_markdown(Path(id)).await;
    }
    share_view(Path(id)).await
}

pub async fn share_view(Path(id): Path<String>) -> Response {
    let id = id
        .trim_end_matches(".json")
        .trim_end_matches(".md")
        .to_string();
    let md = match find_share_file(&id, "md") {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|_| "(empty)".into()),
        None => match find_share_file(&id, "json") {
            Some(p) => std::fs::read_to_string(p).unwrap_or_else(|_| "Share not found".into()),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Html(format!("<h1>Share not found</h1><p>id={id}</p>")),
                )
                    .into_response();
            }
        },
    };
    let escaped = html_escape(&md);
    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Share {id}</title>
<style>
body {{ font-family: system-ui, sans-serif; max-width: 52rem; margin: 2rem auto; padding: 0 1rem; }}
pre {{ white-space: pre-wrap; background: #f6f8fa; padding: 1rem; border-radius: 6px; }}
a {{ color: #0969da; }}
</style></head>
<body>
<p><a href="/s/{id}.md">markdown</a> · <a href="/s/{id}.json">json</a></p>
<pre>{escaped}</pre>
</body></html>"#
    );
    Html(html).into_response()
}

async fn share_json(Path(id): Path<String>) -> Response {
    let id = id.trim_end_matches(".json").to_string();
    match find_share_file(&id, "json") {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(body) => (
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                body,
            )
                .into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        None => (StatusCode::NOT_FOUND, "Share not found").into_response(),
    }
}

async fn share_markdown(Path(id): Path<String>) -> Response {
    let id = id.trim_end_matches(".md").to_string();
    match find_share_file(&id, "md") {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(body) => (
                [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
                body,
            )
                .into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        None => (StatusCode::NOT_FOUND, "Share not found").into_response(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use whycodes_config::Config;
    use whycodes_core::types::{ModelConfig, ProviderConfig};

    #[test]
    fn html_escape_escapes_special_chars() {
        assert_eq!(html_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(html_escape("plain"), "plain");
        assert_eq!(html_escape(""), "");
    }

    fn model(provider: &str, id: &str) -> ModelConfig {
        ModelConfig {
            model_id: id.into(),
            provider_id: provider.into(),
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            thinking: None,
            supports_tools: None,
            supports_images: None,
        }
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn default_model_wins_when_configured() {
        let mut c = Config::default();
        c.default_model = Some(model("openai", "gpt-4o"));
        let (p, m) = default_provider_model(&c);
        assert_eq!((p.as_str(), m.as_str()), ("openai", "gpt-4o"));
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn first_provider_is_the_fallback_without_a_default_model() {
        let mut c = Config::default();
        c.default_model = None;
        c.providers.insert(
            "groq".into(),
            ProviderConfig {
                name: "groq".into(),
                api_key: None,
                api_base: None,
                base_url: None,
                headers: None,
                models: Vec::new(),
                tool_arguments: None,
                extra: HashMap::new(),
            },
        );
        let (p, m) = default_provider_model(&c);
        assert_eq!(p, "groq");
        assert_eq!(m, "default");
    }

    #[test]
    fn hardcoded_fallback_when_nothing_is_configured() {
        let c = Config::default();
        let (p, m) = default_provider_model(&c);
        assert_eq!(
            (p.as_str(), m.as_str()),
            ("anthropic", "claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn find_share_file_in_strips_ext_and_picks_first_hit() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("abc.md"), "# hi").unwrap();
        std::fs::write(b.join("abc.md"), "# later").unwrap();

        let hit = find_share_file_in(&[a.clone(), b.clone()], "abc.json", "md").unwrap();
        assert_eq!(hit, a.join("abc.md"));
        assert!(find_share_file_in(&[a, b], "missing", "json").is_none());
    }

    #[test]
    fn turn_events_have_stable_api_payloads() {
        let cases = [
            (
                TurnEvent::TextDelta("hello".into()),
                serde_json::json!({"type": "text_delta", "text": "hello"}),
            ),
            (
                TurnEvent::ToolStart {
                    id: "call-1".into(),
                    name: "read".into(),
                    input: serde_json::json!({"path": "README.md"}),
                },
                serde_json::json!({
                    "type": "tool_start",
                    "id": "call-1",
                    "name": "read",
                    "input": {"path": "README.md"},
                }),
            ),
            (
                TurnEvent::PermissionAsk {
                    request_id: "perm-1".into(),
                    tool_name: "bash".into(),
                    detail: "cargo test".into(),
                },
                serde_json::json!({
                    "type": "permission_request",
                    "request_id": "perm-1",
                    "tool_name": "bash",
                    "detail": "cargo test",
                }),
            ),
            (
                TurnEvent::Panel(whycodes_core::PanelUpdate::File {
                    path: "src/lib.rs".into(),
                    text: "fn main() {}".into(),
                }),
                serde_json::json!({
                    "type": "panel",
                    "action": "file",
                    "path": "src/lib.rs",
                }),
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(turn_event_json(&event), Some(expected));
        }

        assert_eq!(
            turn_event_json(&TurnEvent::Status("error:provider unavailable".into())),
            Some(serde_json::json!({
                "type": "error",
                "message": "provider unavailable",
            }))
        );
        assert!(turn_event_json(&TurnEvent::Status("done:42chars".into())).is_none());
    }
}
