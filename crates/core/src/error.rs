use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("{0}")]
    Other(String),
}

impl Clone for Error {
    fn clone(&self) -> Self {
        match self {
            Self::Config(s) => Self::Config(s.clone()),
            Self::Io(e) => Self::Io(std::io::Error::new(e.kind(), e.to_string())),
            Self::Serde(_e) => {
                // serde_json::Error doesn't implement Clone, so we create a new one
                Self::Serde(serde_json::from_str::<serde_json::Value>("not json").unwrap_err())
            }
            Self::Llm(s) => Self::Llm(s.clone()),
            Self::Tool(s) => Self::Tool(s.clone()),
            Self::Session(s) => Self::Session(s.clone()),
            Self::Agent(s) => Self::Agent(s.clone()),
            Self::Provider(s) => Self::Provider(s.clone()),
            Self::Http(s) => Self::Http(s.clone()),
            Self::Other(s) => Self::Other(s.clone()),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_config() {
        let err = Error::Config("bad setting".to_string());
        assert_eq!(err.to_string(), "Configuration error: bad setting");
    }

    #[test]
    fn test_error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = Error::from(io_err);
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_display_serde() {
        let serde_err =
            serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = Error::from(serde_err);
        assert!(err.to_string().contains("Serialization error"));
    }

    #[test]
    fn test_error_display_llm() {
        let err = Error::Llm("rate limit".to_string());
        assert_eq!(err.to_string(), "LLM error: rate limit");
    }

    #[test]
    fn test_error_display_tool() {
        let err = Error::Tool("tool not found".to_string());
        assert_eq!(err.to_string(), "Tool error: tool not found");
    }

    #[test]
    fn test_error_display_session() {
        let err = Error::Session("expired".to_string());
        assert_eq!(err.to_string(), "Session error: expired");
    }

    #[test]
    fn test_error_display_agent() {
        let err = Error::Agent("no agent configured".to_string());
        assert_eq!(err.to_string(), "Agent error: no agent configured");
    }

    #[test]
    fn test_error_display_provider() {
        let err = Error::Provider("no api key".to_string());
        assert_eq!(err.to_string(), "Provider error: no api key");
    }

    #[test]
    fn test_error_display_http() {
        let err = Error::Http("404".to_string());
        assert_eq!(err.to_string(), "HTTP error: 404");
    }

    #[test]
    fn test_error_display_other() {
        let err = Error::Other("something went wrong".to_string());
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("permission denied"));
    }

    #[test]
    fn test_error_debug() {
        let err = Error::Config("test".to_string());
        let debug = format!("{:?}", err);
        // Debug output should include the variant name
        assert!(debug.contains("Config"));
    }
}
