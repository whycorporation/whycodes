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
    StructuredSchemaInvalid,
    StructuredOutputInvalid,
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
            Self::StructuredSchemaInvalid => "structured_schema_invalid",
            Self::StructuredOutputInvalid => "structured_output_invalid",
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
    PermissionRequest {
        request_id: String,
        tool_name: String,
        detail: String,
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
    /// When true, the daemon allows every `Ask` without emitting
    /// [`SdkEvent::PermissionRequest`]. Default false.
    #[serde(default)]
    pub auto_approve: Option<bool>,
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

/// `POST /v1/sessions/:id/permission` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionResponse {
    pub request_id: String,
    pub decision: PermissionDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    AllowAlways,
    Deny,
}

/// One try inside [`crate::StructuredResult`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredAttempt {
    pub text: String,
    pub ok: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredResult {
    pub data: serde_json::Value,
    pub attempts: Vec<StructuredAttempt>,
}

/// Pull the first JSON value out of model text (raw, fenced, or wrapped).
pub fn extract_json(text: &str) -> Result<serde_json::Value, String> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(v);
    }
    if let Some(inner) = fenced_json(trimmed)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(inner)
    {
        return Ok(v);
    }
    if let Some(slice) = first_json_slice(trimmed)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(slice)
    {
        return Ok(v);
    }
    Err("no JSON object or array in the model text".into())
}

fn fenced_json(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    let rest = after
        .strip_prefix("json")
        .or_else(|| after.strip_prefix("JSON"))
        .unwrap_or(after);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("```")?;
    Some(rest[..end].trim())
}

fn first_json_slice(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|b| *b == b'{' || *b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if *b == b'\\' {
                escape = true;
            } else if *b == b'"' {
                in_str = false;
            }
            continue;
        }
        match *b {
            b'"' => in_str = true,
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Subset of JSON Schema used by `run_structured`: `type`, `required`, `properties`.
pub fn validate_schema(schema: &serde_json::Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("schema must be a JSON object".into());
    }
    Ok(())
}

pub fn validate_instance(schema: &serde_json::Value, value: &serde_json::Value) -> Vec<String> {
    let mut errors = Vec::new();
    validate_at(schema, value, "$", &mut errors);
    errors
}

fn validate_at(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    if let Some(expected) = schema.get("type").and_then(|t| t.as_str()) {
        let ok = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true,
        };
        if !ok {
            errors.push(format!("{path}: expected {expected}"));
            return;
        }
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for field in required {
            if let Some(name) = field.as_str()
                && value.get(name).is_none()
            {
                errors.push(format!("{path}: missing required {name}"));
            }
        }
    }
    if let (Some(props), Some(obj)) = (
        schema.get("properties").and_then(|p| p.as_object()),
        value.as_object(),
    ) {
        for (key, sub) in props {
            if let Some(child) = obj.get(key) {
                validate_at(sub, child, &format!("{path}.{key}"), errors);
            }
        }
    }
    if let (Some(item_schema), Some(arr)) = (schema.get("items"), value.as_array()) {
        for (i, item) in arr.iter().enumerate() {
            validate_at(item_schema, item, &format!("{path}[{i}]"), errors);
        }
    }
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

    #[test]
    fn extracts_fenced_and_wrapped_json() {
        let v = extract_json("```json\n{\"a\":1}\n```").unwrap();
        assert_eq!(v["a"], 1);
        let v = extract_json("here you go {\"ok\":true} thanks").unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn validates_required_and_types() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": { "name": { "type": "string" }, "n": { "type": "integer" } }
        });
        assert!(validate_instance(&schema, &serde_json::json!({"name":"x","n":2})).is_empty());
        let errs = validate_instance(&schema, &serde_json::json!({"n":"no"}));
        assert!(errs.iter().any(|e| e.contains("missing required name")));
        assert!(errs.iter().any(|e| e.contains("expected integer")));
    }
}
