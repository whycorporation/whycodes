use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(String),

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
