//! Import errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("refusing to import a symlinked settings file: {0}")]
    SymlinkRejected(String),

    #[error("settings import needs explicit approval for this source path: {0}")]
    ConsentRequired(String),

    #[error("unknown import product `{0}` (supported: claude, opencode, grok, codex)")]
    UnknownProduct(String),

    #[error("failed to parse {path}: {message}")]
    Parse { path: String, message: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, ImportError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        assert!(
            ImportError::UnknownProduct("x".into())
                .to_string()
                .contains("claude")
        );
        assert!(
            ImportError::ConsentRequired("/tmp/a".into())
                .to_string()
                .contains("approval")
        );
        assert!(
            ImportError::SymlinkRejected("/tmp/b".into())
                .to_string()
                .contains("symlink")
        );
        assert!(
            ImportError::Parse {
                path: "a.json".into(),
                message: "nope".into()
            }
            .to_string()
            .contains("a.json")
        );
        assert!(ImportError::Msg("x".into()).to_string().contains('x'));
        let io = ImportError::from(std::io::Error::other("io"));
        assert!(io.to_string().contains("I/O"));
        let json = ImportError::from(serde_json::from_str::<u8>("x").unwrap_err());
        assert!(json.to_string().contains("JSON"));
        let toml = ImportError::from(toml::from_str::<toml::Value>("[[[").unwrap_err());
        assert!(toml.to_string().contains("TOML"));
    }
}
