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
mod tests {
    use super::*;

    #[test]
    fn message_io_json_display() {
        assert_eq!(LspError::msg("no stdin").to_string(), "no stdin");
        let io = LspError::from(std::io::Error::other("pipe"));
        assert!(io.to_string().contains("pipe"));
        let json = LspError::from(serde_json::from_str::<u8>("x").unwrap_err());
        assert!(!json.to_string().is_empty());
    }
}
