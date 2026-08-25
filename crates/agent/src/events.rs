//! Streaming turn events for TUI / external consumers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
    Usage(whycodes_core::types::Usage),
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
    /// Two swarm workers tried to write the same file (conflict notify).
    FileConflict {
        path: String,
        claimant: String,
        owner: String,
    },
    /// Swarm fan-out progress (status line / toast).
    SwarmStatus {
        active: usize,
        total: usize,
        message: String,
    },
    /// Background shell job status (started / done / failed / killed).
    Background {
        id: String,
        /// `running` | `done` | `failed` | `killed`
        status: String,
        summary: String,
    },
    /// Queue a user prompt for the next free TUI turn (`schedule` / `/loop`).
    EnqueuePrompt { text: String },
    /// Pin a file / diff / mermaid on the TUI side panel.
    Panel(whycodes_core::PanelUpdate),
    /// A swarm worker sent a message (toast).
    SwarmMessage {
        from: String,
        to: String,
        text: String,
    },
    /// Session todo list for the sticky TUI panel (Grok-style).
    Todos { todos: Vec<whycodes_core::TodoItem> },
    /// Subagent lifecycle for the parent TUI (Grok-style strip / tasks pane).
    Subagent {
        id: String,
        /// `explore` / `general` / swarm worker type.
        kind: String,
        description: String,
        /// `running` | `completed` | `failed` | `cancelled`
        status: String,
        /// Live activity suffix (`Thinking`, `Running: cargo test`).
        activity: String,
        elapsed_ms: u64,
        /// Child transcript (filled when the child finishes).
        output: String,
    },
    /// A reader opened a file another agent wrote.
    FileStale {
        path: String,
        reader: String,
        writer: String,
    },
    /// Daemon asked the SDK client to allow or deny a tool (`Ask` policy).
    PermissionAsk {
        request_id: String,
        tool_name: String,
        detail: String,
    },
    /// Daemon asked the SDK client to answer a `question` tool.
    QuestionAsk {
        request_id: String,
        questions: serde_json::Value,
    },
}

/// Optional sink for turn events (TUI, logging, etc.).
pub type EventSink = tokio::sync::mpsc::UnboundedSender<TurnEvent>;

/// Shared cancel flag — set `true` to abort the current agent turn (Esc / [stop]).
pub type CancelFlag = Arc<AtomicBool>;

pub fn new_cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

pub fn is_cancelled(flag: &Option<CancelFlag>) -> bool {
    flag.as_ref()
        .map(|f| f.load(Ordering::Acquire))
        .unwrap_or(false)
}

pub fn request_cancel(flag: &CancelFlag) {
    flag.store(true, Ordering::Release);
}

/// Await until the cancel flag is set. Never resolves when `flag` is `None`
/// (no cancel channel) — used with `tokio::select!` so a missing flag does
/// not spuriously cancel.
///
/// Poll interval is short enough that Esc / [stop] feels instant even when the
/// LLM stream is idle between tokens (the previous code only checked cancel
/// *after* the next SSE event arrived, which is why "Cancelling…" could hang).
pub async fn wait_until_cancelled(flag: &Option<CancelFlag>) {
    let Some(f) = flag else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        if f.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

pub fn emit(sink: &Option<EventSink>, event: TurnEvent) {
    if let Some(tx) = sink {
        let _ = tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn wait_until_cancelled_resolves_when_flag_set() {
        let flag = new_cancel_flag();
        let opt = Some(Arc::clone(&flag));
        let t0 = Instant::now();
        let waiter = tokio::spawn(async move {
            wait_until_cancelled(&opt).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        request_cancel(&flag);
        waiter.await.expect("join");
        assert!(t0.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn request_cancel_is_visible_to_is_cancelled() {
        let flag = new_cancel_flag();
        let opt = Some(Arc::clone(&flag));
        assert!(!is_cancelled(&opt));
        request_cancel(&flag);
        assert!(is_cancelled(&opt));
    }
}
