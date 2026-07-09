use serde::{Deserialize, Serialize};

/// A row from the sessions table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub project_path: String,
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
