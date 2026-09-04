use thiserror::Error;

/// Memory store / ONNX / JSON snapshot failures.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] whycodes_storage::StorageError),
}

impl MemoryError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }

    pub fn wrap(e: impl std::fmt::Display) -> Self {
        Self::Message(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, MemoryError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_io_display() {
        let msg = MemoryError::msg("memory is disabled");
        assert_eq!(msg.to_string(), "memory is disabled");
        let io = MemoryError::from(std::io::Error::other("disk"));
        assert!(io.to_string().contains("disk"));
        assert_eq!(MemoryError::wrap("onnx load").to_string(), "onnx load");
        let json = MemoryError::from(serde_json::from_str::<i32>("nope").unwrap_err());
        assert!(!json.to_string().is_empty());
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("whycodes.db")).unwrap();
        let err =
            crate::MemoryService::open(dir.path(), dir.path(), crate::MemorySettings::default())
                .unwrap()
                .open_db()
                .err()
                .expect("sqlite open should fail on a directory");
        assert!(matches!(err, MemoryError::Storage(_)), "{err}");
    }
}
