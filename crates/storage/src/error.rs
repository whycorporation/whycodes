use thiserror::Error;

/// SQLite / storage failures.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_error_displays() {
        let err = StorageError::from(rusqlite::Error::InvalidQuery);
        assert!(!err.to_string().is_empty());
    }
}
