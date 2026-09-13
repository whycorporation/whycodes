//! Headless / CI output: structured JSON and streaming NDJSON.
//!
//! Used by `whycodes generate --format …` and `whycodes run <prompt> --format …`.
//! Maps agent [`whycodes_agent::TurnEvent`] values onto a stable wire format
//! that scripts and CI can parse without scraping the TUI.

use serde::{Deserialize, Serialize};
use whycodes_core::types::Usage;

/// How headless commands write results to stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Final assistant text only (human / simple pipes). Default.
    #[default]
    Text,
    /// One JSON object after the turn completes.
    Json,
    /// Newline-delimited JSON events as the turn progresses.
    StreamJson,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }

    /// Parse a CLI / env value (`text`, `json`, `stream-json`, `stream_json`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" | "plain" => Some(Self::Text),
            "json" => Some(Self::Json),
            "stream-json" | "stream_json" | "ndjson" | "jsonl" => Some(Self::StreamJson),
            _ => None,
        }
    }

    pub fn is_structured(self) -> bool {
        !matches!(self, Self::Text)
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| {
            format!("invalid output format '{s}' (expected text, json, or stream-json)")
        })
    }
}

/// One NDJSON line (or the single object for `--format json`) on stdout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CiEvent {
    /// Emitted once at the start of a headless turn.
    Init {
        session_id: String,
        provider: String,
        model: String,
        agent: String,
        cwd: String,
    },
    /// Incremental assistant text.
    TextDelta { text: String },
    /// Incremental thinking / reasoning text.
    ThinkingDelta { text: String },
    /// Model requested a tool.
    ToolStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool finished.
    ToolEnd {
        id: String,
        content: String,
        is_error: bool,
    },
    /// Token usage for a completed LLM step (provider-reported).
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_creation_input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_input_tokens: Option<u64>,
    },
    /// Human-readable status (e.g. "LLM request (step 2)…").
    Status { message: String },
    /// Final envelope: always last event for stream-json; sole object for json.
    Result {
        result: String,
        session_id: String,
        provider: String,
        model: String,
        agent: String,
        usage: Usage,
        duration_ms: u64,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Present on `generate --format json|stream-json` when the turn mutated
        /// a git worktree. Deterministic slop metrics — never LLM-as-judge.
        #[serde(skip_serializing_if = "Option::is_none")]
        slop: Option<serde_json::Value>,
    },
    /// Hard failure before / outside a completed turn result.
    Error { message: String },
    /// Turn cancelled (e.g. interrupt).
    Cancelled,
    /// Parallel fan-out (`generate -j N`): wraps any event with the owning
    /// prompt's session id so interleaved NDJSON stays attributable.
    Session {
        session_id: String,
        event: Box<CiEvent>,
    },
}

impl CiEvent {
    /// Write one JSON line to `w` and flush (required for live stream-json pipes).
    pub fn write_line<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        serde_json::to_writer(&mut *w, self).map_err(std::io::Error::other)?;
        w.write_all(b"\n")?;
        w.flush()
    }

    /// Convenience: write a line to stdout.
    pub fn emit_stdout(&self) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        self.write_line(&mut out)
    }
}

/// Build a final `Result` event from turn outcome + metadata.
#[derive(Debug, Clone)]
pub struct ResultMeta {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub agent: String,
    pub usage: Usage,
    pub duration_ms: u64,
    pub slop: Option<serde_json::Value>,
}

impl ResultMeta {
    pub fn ok(self, result: impl Into<String>) -> CiEvent {
        CiEvent::Result {
            result: result.into(),
            session_id: self.session_id,
            provider: self.provider,
            model: self.model,
            agent: self.agent,
            usage: self.usage,
            duration_ms: self.duration_ms,
            is_error: false,
            error: None,
            slop: self.slop,
        }
    }

    pub fn err(self, message: impl Into<String>) -> CiEvent {
        let message = message.into();
        CiEvent::Result {
            result: String::new(),
            session_id: self.session_id,
            provider: self.provider,
            model: self.model,
            agent: self.agent,
            usage: self.usage,
            duration_ms: self.duration_ms,
            is_error: true,
            error: Some(message),
            slop: self.slop,
        }
    }
}
