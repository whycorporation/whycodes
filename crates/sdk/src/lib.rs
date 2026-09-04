//! Thin client for the whycodes local daemon (`whycodes serve`).
//!
//! This crate does **not** embed the agent loop. It speaks protocol v1 over
//! HTTP. Use [`WhyCodesClient::connect`] for a running daemon or
//! [`WhyCodesClient::launch`] to spawn one.

pub mod client;

#[cfg(test)]
mod mock_tests;

pub use client::{EventStream, LaunchOptions, RunOptions, WhyCodesClient};
pub use whycodes_protocol::sdk::{
    CompactRequest, CreateSessionRequest, ErrorCode, Handshake, HistoryMessage, ModelInfo,
    ModelList, PROTOCOL_MAJOR, PermissionDecision, PermissionResponse, QuestionAnswerWire,
    QuestionResponse, RenameRequest, RewindRequest, RunRequest, SdkEvent, SessionHistory,
    SessionInfo, SessionList, SetModelRequest, StructuredAttempt, StructuredResult,
    ToolCallSummary, TurnResult, UsageSnapshot, extract_json, validate_instance, validate_schema,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_error_constructors() {
        let e = SdkError::new(ErrorCode::Internal, "boom");
        assert_eq!(e.code, ErrorCode::Internal);
        assert_eq!(e.message, "boom");
        assert!(e.source.is_none());
        assert!(e.to_string().contains("internal"));

        let e = SdkError::with_source(ErrorCode::Disconnected, "nope", std::io::Error::other("io"));
        assert_eq!(e.code, ErrorCode::Disconnected);
        assert!(e.source.is_some());
    }

    #[tokio::test]
    async fn reqwest_error_timeout_maps_to_timeout_code() {
        let err = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        let wrapped = SdkError::from(err);
        assert!(
            matches!(wrapped.code, ErrorCode::Timeout | ErrorCode::Disconnected),
            "{wrapped:?}"
        );
    }
}
