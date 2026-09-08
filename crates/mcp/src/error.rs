use thiserror::Error;

/// MCP client / transport / stdio-server failures.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

impl McpError {
    pub fn msg(s: &str) -> Self {
        Self::Message(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, McpError>;

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
