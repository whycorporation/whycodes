use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::sse::{Event, Sse},
    response::{Html, IntoResponse, Response},
};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use whycode_session::session::Session;

use crate::AppState;

/// Resolve share file directories: project .whycode/shares + global data dir shares.
fn share_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".whycode").join("shares"));
    }
    if let Ok(data) = whycode_config::Config::data_dir() {
        dirs.push(data.join("shares"));
    }
    dirs
}

fn find_share_file(id: &str, ext: &str) -> Option<PathBuf> {
    // Strip accidental extension from id
    let id = id.trim_end_matches(".json").trim_end_matches(".md");
    for dir in share_search_dirs() {
        let p = dir.join(format!("{id}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

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
    let models: Vec<_> = state
        .config
        .models
        .values()
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

/// List locally shared sessions.
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

/// Human-readable HTML view of a shared session.
pub async fn share_view(Path(id): Path<String>) -> Response {
    let id = id
        .trim_end_matches(".json")
        .trim_end_matches(".md")
        .to_string();
    let md = match find_share_file(&id, "md") {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|_| "(empty)".into()),
        None => {
            // fall back to JSON pretty as text
            match find_share_file(&id, "json") {
                Some(p) => std::fs::read_to_string(p).unwrap_or_else(|_| "Share not found".into()),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Html(format!("<h1>Share not found</h1><p>id={id}</p>")),
                    )
                        .into_response();
                }
            }
        }
    };

    let escaped = html_escape(&md);
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Whycode share · {id}</title>
<style>
  :root {{ color-scheme: dark light; }}
  body {{ font-family: ui-sans-serif, system-ui, sans-serif; max-width: 52rem; margin: 2rem auto; padding: 0 1rem; line-height: 1.55; }}
  pre {{ white-space: pre-wrap; word-break: break-word; background: #111; color: #e8e8e8; padding: 1.25rem; border-radius: 8px; overflow-x: auto; }}
  a {{ color: #6cb6ff; }}
  header {{ margin-bottom: 1rem; opacity: 0.85; font-size: 0.9rem; }}
</style>
</head>
<body>
<header>
  <strong>Whycode share</strong> · <code>{id}</code>
  · <a href="/s/{id}.md">markdown</a>
  · <a href="/s/{id}.json">json</a>
</header>
<pre>{escaped}</pre>
</body>
</html>"#
    );
    Html(html).into_response()
}

pub async fn share_json(Path(id): Path<String>) -> Response {
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
        None => (StatusCode::NOT_FOUND, "share not found").into_response(),
    }
}

pub async fn share_markdown(Path(id): Path<String>) -> Response {
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
        None => {
            // synthesize from json conversation if md missing
            match find_share_file(&id, "json") {
                Some(p) => match std::fs::read_to_string(p) {
                    Ok(body) => (
                        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                        body,
                    )
                        .into_response(),
                    Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
                },
                None => (StatusCode::NOT_FOUND, "share not found").into_response(),
            }
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
