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
