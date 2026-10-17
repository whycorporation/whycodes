//! WhyCodes ⇄ ACP shapes: turn events → `session/update`, prompt blocks → text.

use std::path::Path;

use serde_json::{Value, json};
use whycodes_agent::TurnEvent;
use whycodes_core::TodoStatus;
use whycodes_core::types::{ContentBlock, Message, MessageContent, Role};

/// Longest tool output forwarded to the client, in chars.
const MAX_TOOL_OUTPUT: usize = 20_000;

/// The `update` object for one turn event, or `None` when ACP has no use for it.
pub(crate) fn update_for(ev: &TurnEvent, cwd: &Path) -> Option<Value> {
    match ev {
        TurnEvent::TextDelta(text) => Some(chunk("agent_message_chunk", text)),
        TurnEvent::ThinkingDelta(text) => Some(chunk("agent_thought_chunk", text)),
        TurnEvent::ToolStart { id, name, input } => Some(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "title": tool_title(name, input),
            "kind": tool_kind(name),
            "status": "in_progress",
            "rawInput": input,
            "locations": locations(input, cwd),
        })),
        TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": if *is_error { "failed" } else { "completed" },
            "content": [{
                "type": "content",
                "content": {"type": "text", "text": truncate(content, MAX_TOOL_OUTPUT)},
            }],
        })),
        TurnEvent::Todos { todos } => Some(json!({
            "sessionUpdate": "plan",
            "entries": todos
                .iter()
                .filter(|t| t.status != TodoStatus::Cancelled)
                .map(|t| json!({
                    "content": t.content,
                    "priority": "medium",
                    "status": match t.status {
                        TodoStatus::InProgress => "in_progress",
                        TodoStatus::Completed => "completed",
                        _ => "pending",
                    },
                }))
                .collect::<Vec<_>>(),
        })),
        _ => None,
    }
}

fn chunk(kind: &str, text: &str) -> Value {
    json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}})
}

/// ACP `ToolKind` for a WhyCodes tool name.
pub(crate) fn tool_kind(name: &str) -> &'static str {
    match name {
        "read" | "list" | "repomap" | "git_diff" | "git_log" | "git_status" | "git_blame" => "read",
        "grep" | "glob" | "tool_search" => "search",
        "edit" | "write" | "apply_patch" | "ast_edit" => "edit",
        "bash" => "execute",
        "webfetch" | "websearch" | "mcp_websearch" => "fetch",
        "plan" | "todoread" => "think",
        _ => "other",
    }
}

/// `name: <main argument>` so the client's tool row says what it touches.
pub(crate) fn tool_title(name: &str, input: &Value) -> String {
    let arg = ["path", "file_path", "command", "pattern", "url", "query"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str));
    match arg {
        Some(arg) => format!("{name}: {}", truncate(arg, 120)),
        None => name.to_string(),
    }
}

fn locations(input: &Value, cwd: &Path) -> Vec<Value> {
    ["path", "file_path"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(|p| vec![json!({"path": cwd.join(p).to_string_lossy()})])
        .unwrap_or_default()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max).collect();
    short.push('…');
    short
}

/// The user's prompt as text. Embedded resources become tagged context and
/// links become `@path` mentions; images and audio are not advertised.
pub(crate) fn prompt_text(blocks: &[Value]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            Some("resource_link") => {
                if let Some(uri) = block.get("uri").and_then(Value::as_str) {
                    parts.push(mention(uri));
                }
            }
            Some("resource") => {
                let res = &block["resource"];
                let uri = res.get("uri").and_then(Value::as_str).unwrap_or("");
                if let Some(text) = res.get("text").and_then(Value::as_str) {
                    parts.push(format!("<context uri=\"{uri}\">\n{text}\n</context>"));
                } else if !uri.is_empty() {
                    parts.push(mention(uri));
                }
            }
            _ => {}
        }
    }
    parts.join("\n")
}

fn mention(uri: &str) -> String {
    let Some(path) = uri.strip_prefix("file://") else {
        return uri.to_string();
    };
    // `file:///C:/x` → `C:/x`; POSIX `/x` keeps its slash.
    let bytes = path.as_bytes();
    let path = if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
        &path[1..]
    } else {
        path
    };
    format!("@{path}")
}

/// `session/update` payloads that replay a loaded session's conversation.
pub(crate) fn history_updates(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for msg in messages {
        let kind = match msg.role {
            Role::User => "user_message_chunk",
            Role::Assistant => "agent_message_chunk",
            Role::System | Role::Tool => continue,
        };
        let text = match &msg.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        if !text.trim().is_empty() {
            out.push(chunk(kind, &text));
        }
    }
    out
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
