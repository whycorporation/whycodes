use thiserror::Error;

/// LSP client / protocol failures.
#[derive(Debug, Error)]
pub enum LspError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LspError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

pub type Result<T> = std::result::Result<T, LspError>;

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
