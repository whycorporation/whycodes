//! Thin client for the whycode local daemon (`whycode serve`).
//!
//! This crate does **not** embed the agent loop. It speaks protocol v1 over
//! HTTP. Use [`WhycodeClient::connect`] for a running daemon or
//! [`WhycodeClient::launch`] to spawn one.

pub mod client;

pub use client::{EventStream, LaunchOptions, RunOptions, WhycodeClient};
pub use whycode_protocol::sdk::{
    CreateSessionRequest, ErrorCode, Handshake, PROTOCOL_MAJOR, RunRequest, SdkEvent, SessionInfo,
    SessionList, ToolCallSummary, TurnResult, UsageSnapshot,
};

/// SDK-level failure. Branch on [`SdkError::code`].
#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct SdkError {
    pub code: ErrorCode,
    pub message: String,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl SdkError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        code: ErrorCode,
        message: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(source.into()),
        }
    }
}

impl From<reqwest::Error> for SdkError {
    fn from(e: reqwest::Error) -> Self {
        let code = if e.is_timeout() {
            ErrorCode::Timeout
        } else {
            ErrorCode::Disconnected
        };
        Self::with_source(code, e.to_string(), e)
    }
}
