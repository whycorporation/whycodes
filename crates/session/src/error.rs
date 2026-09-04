use thiserror::Error;

/// Session persist / import / export failures.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] whycodes_storage::StorageError),
    #[error(transparent)]
    Chrono(#[from] chrono::ParseError),
}

impl SessionError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

pub type Result<T> = std::result::Result<T, SessionError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_io_display() {
        let msg = SessionError::msg("no messages");
        assert_eq!(msg.to_string(), "no messages");
        let io = SessionError::from(std::io::Error::other("disk"));
        assert!(io.to_string().contains("disk"));
        let json = SessionError::from(serde_json::from_str::<i32>("nope").unwrap_err());
        assert!(!json.to_string().is_empty());
        let chrono = SessionError::from(chrono::DateTime::parse_from_rfc3339("bad").unwrap_err());
        assert!(!chrono.to_string().is_empty());
        let storage = SessionError::from(whycodes_storage::StorageError::from(
            rusqlite::Error::InvalidQuery,
        ));
        assert!(!storage.to_string().is_empty());
    }
}
