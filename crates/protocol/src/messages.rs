use serde::{Deserialize, Serialize};

/// Client-to-server message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Chat { content: String },
    CreateSession { project: Option<String> },
    ListSessions,
    GetTools,
    GetModels,
}

/// Server-to-client message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    TextDelta { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { id: String, content: String, is_error: bool },
    Thinking { text: String },
    Usage { input_tokens: u64, output_tokens: u64 },
    Done,
    Error { message: String },
}
