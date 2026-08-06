//! Streaming turn events for TUI / external consumers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

/// Events emitted during `Agent::run_turn_with_events`.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Incremental assistant text
    TextDelta(String),
    /// Incremental thinking/reasoning
    ThinkingDelta(String),
    /// Model requested a tool
    ToolStart {
        id: String,
        name: String,
        input: Value,
    },
    /// Tool finished
    ToolEnd {
        id: String,
        content: String,
        is_error: bool,
    },
    /// Token usage for the turn that just finished, as the provider reported
    /// it. Emitted once per turn rather than per streamed chunk: the interesting
    /// figure is what the turn cost, and a running partial is noise.
    Usage(whycode_core::types::Usage),
    /// Human-readable status line
    Status(String),
    /// Heuristic user-intent for this turn (badge + optional toast in TUI).
    Intent {
        /// `question` | `change` | `plan` | `ambiguous` | `trivial`
        kind: String,
        /// 0.0–1.0
        confidence: f32,
        /// Short chrome label when showable (`Q`, `chg`, `plan`), else empty.
        badge: String,
        /// Toast severity: `info`, `warning`, or empty (no toast).
        notice_kind: String,
        /// Toast / status body; empty when no notice.
        notice: String,
    },
    /// Turn was cancelled by the user
    Cancelled,
}

/// Optional sink for turn events (TUI, logging, etc.).
pub type EventSink = tokio::sync::mpsc::UnboundedSender<TurnEvent>;

/// Shared cancel flag — set `true` to abort the current agent turn (Esc).
pub type CancelFlag = Arc<AtomicBool>;

pub fn new_cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

pub fn is_cancelled(flag: &Option<CancelFlag>) -> bool {
    flag.as_ref()
        .map(|f| f.load(Ordering::Relaxed))
        .unwrap_or(false)
}

pub fn request_cancel(flag: &CancelFlag) {
    flag.store(true, Ordering::Relaxed);
}

pub fn emit(sink: &Option<EventSink>, event: TurnEvent) {
    if let Some(tx) = sink {
        let _ = tx.send(event);
    }
}
