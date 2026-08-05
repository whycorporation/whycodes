use serde::{Deserialize, Serialize};
use whycode_core::types::Usage;

/// A row from the sessions table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub project_path: String,
    /// Provider-reported token totals for this session.
    #[serde(default)]
    pub usage: Usage,
}

/// Aggregated usage across all sessions (for `whycode stats`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageTotals {
    pub session_count: usize,
    pub message_count: usize,
    pub usage: Usage,
}

/// A row from the messages table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    pub created_at: String,
}

/// A row from the state key-value table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateRow {
    pub key: String,
    pub value: String,
}

/// A row from the memories table (cross-session semantic / auto memory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    pub project_key: String,
    pub text: String,
    /// Little-endian f32 embedding blob.
    pub embedding: Vec<u8>,
    pub source_session: Option<String>,
    pub created_at: String,
    pub last_recalled_at: Option<String>,
    pub recall_count: i64,
}
