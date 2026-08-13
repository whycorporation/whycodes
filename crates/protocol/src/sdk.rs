//! Protocol v1 — the public daemon wire format (`whycode serve` + `whycode-sdk`).
//!
//! The agent loop stays in-process inside the daemon. Clients speak this
//! document, not `whycode-agent` types. New `ev` values may appear in a
//! minor release; unknown tags deserialize as [`SdkEvent::Unknown`].

use serde::{Deserialize, Serialize};

/// Negotiated major version. A client that does not speak this number must
/// refuse the handshake rather than half-working.
pub const PROTOCOL_MAJOR: u32 = 1;

/// `GET /v1/health` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    pub protocol: u32,
    pub version: String,
    pub healthy: bool,
    pub project: String,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub sessions_in_memory: usize,
}

/// Stable error codes. Branch on these, not on the diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Disconnected,
    Timeout,
    UnknownSession,
    InvalidRequest,
    Auth,
    Internal,
    ServeNotFound,
    StartupFailed,
    StartupTimeout,
    UnsupportedVersion,
    Cancelled,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Timeout => "timeout",
            Self::UnknownSession => "unknown_session",
            Self::InvalidRequest => "invalid_request",
            Self::Auth => "auth",
            Self::Internal => "internal",
            Self::ServeNotFound => "serve_not_found",
            Self::StartupFailed => "startup_failed",
            Self::StartupTimeout => "startup_timeout",
            Self::UnsupportedVersion => "unsupported_version",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One streamed event. Discriminated on `ev`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum SdkEvent {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolEnd {
        id: String,
        content: String,
        is_error: bool,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default)]
        cache_read_input_tokens: u64,
        #[serde(default)]
        cache_creation_input_tokens: u64,
    },
    Status {
        message: String,
    },
    Cancelled,
    TurnDone {
        #[serde(default)]
        text: String,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    Intent {
        kind: String,
        confidence: f32,
        #[serde(default)]
        badge: String,
        #[serde(default)]
        notice_kind: String,
        #[serde(default)]
        notice: String,
    },
    FileConflict {
        path: String,
        claimant: String,
        owner: String,
    },
    SwarmStatus {
        active: usize,
        total: usize,
        message: String,
    },
    Background {
        id: String,
        status: String,
        summary: String,
    },
    /// Catch-all so a newer daemon does not break an older client.
    #[serde(other)]
    Unknown,
}

/// `POST /v1/sessions` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub persist: Option<bool>,
}

/// Session metadata returned by list / create / get.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub messages: Option<usize>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<SessionInfo>,
}

/// `POST /v1/sessions/:id/run` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    pub message: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_turns: Option<usize>,
}

/// Collected turn (what `WhycodeClient::run` returns).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnResult {
    pub text: String,
    pub tool_calls: Vec<ToolCallSummary>,
    #[serde(default)]
    pub usage: Option<UsageSnapshot>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub id: String,
    pub name: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_delta_round_trips() {
        let ev = SdkEvent::TextDelta { text: "hi".into() };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"ev\":\"text_delta\""));
        let back: SdkEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn unknown_ev_is_forward_compatible() {
        let ev: SdkEvent = serde_json::from_str(r#"{"ev":"future_thing","x":1}"#).unwrap();
        assert!(matches!(ev, SdkEvent::Unknown));
    }

    #[test]
    fn error_code_is_snake_case() {
        let json = serde_json::to_string(&ErrorCode::UnknownSession).unwrap();
        assert_eq!(json, "\"unknown_session\"");
    }

    #[test]
    fn handshake_requires_protocol_1() {
        let h = Handshake {
            protocol: PROTOCOL_MAJOR,
            version: "0.1.0".into(),
            healthy: true,
            project: "/tmp".into(),
            uptime_secs: 1,
            sessions_in_memory: 0,
        };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["protocol"], 1);
    }
}
