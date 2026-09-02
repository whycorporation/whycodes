use thiserror::Error;

/// Matchable LLM / HTTP transport failure class.
///
/// Retry and TUI copy should match this, not parse [`Error`] display strings.
/// Wire bodies still go through `whycodes_llm::classify_message` when `kind`
/// is [`ErrorKind::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorKind {
    RateLimited,
    Server,
    Network,
    Timeout,
    Auth,
    Client,
    ContextOverflow,
    Cancelled,
    #[default]
    Unknown,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Server => "server",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Auth => "auth",
            Self::Client => "client",
            Self::ContextOverflow => "context_overflow",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    /// Whether another attempt may succeed without changing the request.
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Server | Self::Network | Self::Timeout
        )
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// LLM or HTTP payload with an optional structured kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub kind: ErrorKind,
    pub message: String,
}

impl TransportError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unknown, message)
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for TransportError {
    fn from(message: String) -> Self {
        Self::unknown(message)
    }
}

impl From<&str> for TransportError {
    fn from(message: &str) -> Self {
        Self::unknown(message)
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(String),

    #[error("LLM error: {0}")]
    Llm(TransportError),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("HTTP error: {0}")]
    Http(TransportError),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn llm(message: impl Into<String>) -> Self {
        Self::Llm(TransportError::unknown(message))
    }

    pub fn llm_kind(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Llm(TransportError::new(kind, message))
    }

    pub fn http(message: impl Into<String>) -> Self {
        Self::Http(TransportError::unknown(message))
    }

    pub fn http_kind(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Http(TransportError::new(kind, message))
    }

    /// Structured kind when this is an LLM/HTTP transport error.
    pub fn transport_kind(&self) -> Option<ErrorKind> {
        match self {
            Self::Llm(e) | Self::Http(e) => Some(e.kind),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Serde(err.to_string())
    }
}

impl Clone for Error {
    fn clone(&self) -> Self {
        match self {
            Self::Config(s) => Self::Config(s.clone()),
            Self::Io(e) => Self::Io(std::io::Error::new(e.kind(), e.to_string())),
            Self::Serde(s) => Self::Serde(s.clone()),
            Self::Llm(e) => Self::Llm(e.clone()),
            Self::Tool(s) => Self::Tool(s.clone()),
            Self::Session(s) => Self::Session(s.clone()),
            Self::Agent(s) => Self::Agent(s.clone()),
            Self::Provider(s) => Self::Provider(s.clone()),
            Self::Http(e) => Self::Http(e.clone()),
            Self::Other(s) => Self::Other(s.clone()),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kind_transport_constructors_and_clone() {
        let kinds = [
            ErrorKind::RateLimited,
            ErrorKind::Server,
            ErrorKind::Network,
            ErrorKind::Timeout,
            ErrorKind::Auth,
            ErrorKind::Client,
            ErrorKind::ContextOverflow,
            ErrorKind::Cancelled,
            ErrorKind::Unknown,
        ];
        for k in kinds {
            assert!(!k.as_str().is_empty());
            assert_eq!(k.to_string(), k.as_str());
            let _ = k.retryable();
        }
        assert_eq!(ErrorKind::default(), ErrorKind::Unknown);
        assert!(ErrorKind::Timeout.retryable());
        assert!(!ErrorKind::Auth.retryable());

        let te = TransportError::new(ErrorKind::Server, "s");
        assert_eq!(te.to_string(), "s");
        assert_eq!(TransportError::unknown("u").kind, ErrorKind::Unknown);
        let from_string: TransportError = String::from("x").into();
        assert_eq!(from_string.message, "x");
        let from_str: TransportError = "y".into();
        assert_eq!(from_str.message, "y");

        let errs = vec![
            Error::Config("c".into()),
            Error::Io(std::io::Error::other("i")),
            Error::Serde("s".into()),
            Error::llm("l"),
            Error::llm_kind(ErrorKind::RateLimited, "r"),
            Error::Tool("t".into()),
            Error::Session("se".into()),
            Error::Agent("a".into()),
            Error::Provider("p".into()),
            Error::http("h"),
            Error::http_kind(ErrorKind::Timeout, "ht"),
            Error::Other("o".into()),
        ];
        for e in &errs {
            let cloned = e.clone();
            assert!(!cloned.to_string().is_empty());
            let _ = format!("{e:?}");
        }
        assert_eq!(Error::llm("l").transport_kind(), Some(ErrorKind::Unknown));
        assert_eq!(
            Error::http_kind(ErrorKind::Timeout, "ht").transport_kind(),
            Some(ErrorKind::Timeout)
        );
        assert_eq!(Error::Config("c".into()).transport_kind(), None);
        let json_err = serde_json::from_str::<u8>("not-json").unwrap_err();
        let e = Error::from(json_err);
        assert!(matches!(e, Error::Serde(_)));
    }
}
