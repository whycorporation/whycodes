//! Per-session runtime state for the TUI event loop.
//!
//! S1 of the parallel multi-session plan: extracts what used to be loose
//! locals in `run()` into one struct so a later step can hold
//! `Vec<SessionRuntime>` and switch between live sessions.
//!
//! S2 adds the per-session state machine and the view snapshot that make
//! background sessions lossless: every runtime owns its channels, turn
//! guard, and a snapshot of the TUI view state (transcript, scroll, draft)
//! taken when the user switches away.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use whycode_agent::agent::Agent;
use whycode_agent::permission::{ChannelPermissionPrompter, PermissionRequest};
use whycode_agent::{CancelFlag, ChannelQuestionPrompter, QuestionRequest, TurnEvent};
use whycode_session::SessionHistory;
use whycode_session::session::Session;
use whycode_storage::db::Database;

use crate::app::{AgentState, ChatMessage, ChatRole};
use crate::run::TurnOutcome;

/// Open a dedicated connection for one runtime (same db file as the rest
/// of the app; SQLite serializes writers). `None` when the data dir is
/// unavailable.
fn open_runtime_db() -> Option<Database> {
    let data_dir = whycode_config::Config::data_dir().ok()?;
    std::fs::create_dir_all(&data_dir).ok()?;
    let db_path = data_dir.join("whycode.db");
    Database::open(&db_path.to_string_lossy()).ok()
}

/// Live-session state for the dashboard and cycle ordering.
///
/// Derived from the turn guard + prompter queues, never set directly:
/// `working` while a turn owns the agent, `waiting_permission` /
/// `waiting_input` when the turn is parked on a UI reply, `idle`
/// otherwise. `error` is sticky until the next turn starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Working,
    WaitingPermission,
    WaitingInput,
    Idle,
    Error,
}

impl SessionState {
    /// Dashboard group: needs-input first, then working, then idle.
    pub fn group_rank(self) -> u8 {
        match self {
            Self::WaitingPermission | Self::WaitingInput => 0,
            Self::Working => 1,
            Self::Idle | Self::Error => 2,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Working => "…",
            Self::WaitingPermission | Self::WaitingInput => "!",
            Self::Idle => "·",
            Self::Error => "✗",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::WaitingPermission => "needs permission",
            Self::WaitingInput => "needs input",
            Self::Idle => "idle",
            Self::Error => "error",
        }
    }
}

/// TUI view state owned by one session — swapped in/out of `TuiApp` on
/// switch so each session keeps its own transcript, scroll and draft.
#[derive(Debug)]
pub struct ViewSnapshot {
    pub messages: Vec<ChatMessage>,
    pub session_title: String,
    pub status_message: String,
    pub current_agent_state: AgentState,
    pub scroll_offset: usize,
    pub auto_scroll: bool,
    pub selected_msg: Option<usize>,
    pub input_buffer: String,
    pub input_lines: Vec<String>,
    pub input_cursor: usize,
    pub intent_badge: Option<String>,
    pub intent_kind: Option<String>,
    pub turn_usage: Option<whycode_core::types::Usage>,
    pub context_used: u64,
    pub pending_suggestion: Option<String>,
}

impl Default for ViewSnapshot {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            session_title: String::new(),
            status_message: String::new(),
            current_agent_state: AgentState::Idle,
            scroll_offset: 0,
            auto_scroll: true,
            selected_msg: None,
            input_buffer: String::new(),
            input_lines: Vec::new(),
            input_cursor: 0,
            intent_badge: None,
            intent_kind: None,
            turn_usage: None,
            context_used: 0,
            pending_suggestion: None,
        }
    }
}

/// Everything the TUI needs to keep one session alive: the agent, the
/// transcript, the in-flight turn guard, and the channels that connect the
/// background turn task to the event loop.
pub struct SessionRuntime {
    pub agent: Agent,
    pub session: Session,
    pub history: SessionHistory,

    /// True while a turn task owns the real agent/session (moved out).
    pub agent_busy: bool,
    pub cancel_flag: Option<CancelFlag>,
    /// Live turn task — aborted on force-stop so a hung LLM/tool cannot pin UI.
    pub turn_join: Option<JoinHandle<()>>,
    /// Session clone taken just before the turn task starts (after user msg).
    /// Restored if we have to abort the task (which would otherwise drop
    /// agent/session).
    pub session_backup: Option<Session>,

    /// Agent → UI turn events (text deltas, tool calls, status, …).
    pub event_tx: mpsc::UnboundedSender<TurnEvent>,
    pub event_rx: mpsc::UnboundedReceiver<TurnEvent>,
    /// Turn task → UI completion (returns the moved-out agent/session).
    pub done_tx: mpsc::UnboundedSender<TurnOutcome>,
    pub done_rx: mpsc::UnboundedReceiver<TurnOutcome>,

    /// Permission asks awaiting a UI reply (multi-ask queue).
    pub pending_perm_queue: VecDeque<PermissionRequest>,
    /// Questionnaire requests awaiting a UI reply.
    pub pending_question_queue: VecDeque<QuestionRequest>,

    /// Interactive prompters bound into the agent; the loop drains the
    /// matching receivers returned at construction.
    pub perm_prompter: Arc<ChannelPermissionPrompter>,
    pub question_prompter: Arc<ChannelQuestionPrompter>,
    /// Permission asks arriving from the agent (drained into the queue).
    pub perm_rx: mpsc::UnboundedReceiver<PermissionRequest>,
    /// Questionnaire requests arriving from the agent.
    pub question_rx: mpsc::UnboundedReceiver<QuestionRequest>,

    /// View state while this session is in the background.
    pub view: ViewSnapshot,
    /// Something happened since the user last looked at this session.
    pub unread: bool,
    /// Last turn ended in error (sticky until the next turn starts).
    pub last_error: bool,
    /// When this runtime was created (dashboard age).
    pub created_at: std::time::Instant,
    /// Per-runtime SQLite connection (S4) — connections are cheap, and one
    /// per runtime avoids a global Mutex around every persist. `None` when
    /// the data dir is unavailable; persists become best-effort no-ops.
    pub db: Option<Database>,
}

impl SessionRuntime {
    /// Bundle the pieces `run()` already built. Channels and prompters are
    /// created by the caller because the prompter receivers and the
    /// `wire_event_sink` call happen at different points in `run()` setup.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent: Agent,
        session: Session,
        history: SessionHistory,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        event_rx: mpsc::UnboundedReceiver<TurnEvent>,
        done_tx: mpsc::UnboundedSender<TurnOutcome>,
        done_rx: mpsc::UnboundedReceiver<TurnOutcome>,
        perm_prompter: Arc<ChannelPermissionPrompter>,
        question_prompter: Arc<ChannelQuestionPrompter>,
        perm_rx: mpsc::UnboundedReceiver<PermissionRequest>,
        question_rx: mpsc::UnboundedReceiver<QuestionRequest>,
    ) -> Self {
        Self {
            agent,
            session,
            history,
            agent_busy: false,
            cancel_flag: None,
            turn_join: None,
            session_backup: None,
            event_tx,
            event_rx,
            done_tx,
            done_rx,
            pending_perm_queue: VecDeque::new(),
            pending_question_queue: VecDeque::new(),
            perm_prompter,
            question_prompter,
            perm_rx,
            question_rx,
            view: ViewSnapshot::default(),
            unread: false,
            last_error: false,
            created_at: std::time::Instant::now(),
            db: open_runtime_db(),
        }
    }

    /// Persist the session through this runtime's own connection.
    pub fn persist(&self, reason: &str) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        if let Err(e) = self.session.save_to_db(db) {
            whycode_core::logging::emit_sid(
                "session",
                "warn",
                "session.persist_failed",
                Some(self.session.id.as_str()),
                Some(serde_json::json!({ "reason": reason, "error": e.to_string() })),
            );
        }
    }

    /// Current live state, derived from the turn guard and prompter queues.
    pub fn state(&self) -> SessionState {
        if self.agent_busy {
            if !self.pending_perm_queue.is_empty() {
                SessionState::WaitingPermission
            } else if !self.pending_question_queue.is_empty() {
                SessionState::WaitingInput
            } else {
                SessionState::Working
            }
        } else if self.last_error {
            SessionState::Error
        } else {
            SessionState::Idle
        }
    }

    /// One-line activity preview for the dashboard (last non-empty
    /// assistant/user text, or the live status line while working).
    pub fn preview(&self) -> String {
        if self.agent_busy && !self.view.status_message.is_empty() {
            return self.view.status_message.clone();
        }
        preview_from_messages(&self.view.messages)
    }
}

/// Last non-empty user/assistant first line, capped at 80 chars.
///
/// Shared by parked `view.messages` and the *active* `TuiApp` transcript —
/// after a move-on-switch the live session's snapshot is empty.
pub(crate) fn preview_from_messages(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, ChatRole::Assistant | ChatRole::User) && !m.content.is_empty())
        .map(|m| {
            m.content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(80)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_string(),
            blocks: Vec::new(),
            results_expanded: false,
            tool_calls: Vec::new(),
            error: None,
            duration_ms: None,
            image_labels: Vec::new(),
            created_at: None,
            layout_cache: None,
            line_cache: None,
        }
    }

    #[test]
    fn session_state_group_rank_orders_needs_input_first() {
        assert_eq!(SessionState::WaitingPermission.group_rank(), 0);
        assert_eq!(SessionState::WaitingInput.group_rank(), 0);
        assert_eq!(SessionState::Working.group_rank(), 1);
        assert_eq!(SessionState::Idle.group_rank(), 2);
        assert_eq!(SessionState::Error.group_rank(), 2);
    }

    #[test]
    fn session_state_glyphs_and_labels() {
        assert_eq!(SessionState::Working.glyph(), "…");
        assert_eq!(SessionState::WaitingPermission.glyph(), "!");
        assert_eq!(SessionState::WaitingInput.glyph(), "!");
        assert_eq!(SessionState::Idle.glyph(), "·");
        assert_eq!(SessionState::Error.glyph(), "✗");
        assert_eq!(SessionState::Working.label(), "working");
        assert_eq!(SessionState::WaitingPermission.label(), "needs permission");
        assert_eq!(SessionState::WaitingInput.label(), "needs input");
        assert_eq!(SessionState::Idle.label(), "idle");
        assert_eq!(SessionState::Error.label(), "error");
    }

    #[test]
    fn view_snapshot_default_is_sane() {
        let v = ViewSnapshot::default();
        assert!(v.messages.is_empty());
        assert!(v.session_title.is_empty());
        assert_eq!(v.current_agent_state, AgentState::Idle);
        assert_eq!(v.scroll_offset, 0);
        assert!(v.auto_scroll);
        assert!(v.selected_msg.is_none());
        assert!(v.input_buffer.is_empty());
        assert_eq!(v.context_used, 0);
    }

    #[test]
    fn runtime_state_transitions() {
        let rt = make_runtime();
        assert_eq!(rt.state(), SessionState::Idle);

        let mut rt = make_runtime();
        rt.agent_busy = true;
        assert_eq!(rt.state(), SessionState::Working);

        let mut rt = make_runtime();
        rt.agent_busy = true;
        rt.pending_perm_queue.push_back(perm_request());
        assert_eq!(rt.state(), SessionState::WaitingPermission);

        let mut rt = make_runtime();
        rt.agent_busy = true;
        rt.pending_question_queue.push_back(question_request());
        assert_eq!(rt.state(), SessionState::WaitingInput);

        let mut rt = make_runtime();
        rt.last_error = true;
        assert_eq!(rt.state(), SessionState::Error);

        let mut rt = make_runtime();
        rt.agent_busy = true;
        rt.pending_perm_queue.push_back(perm_request());
        rt.pending_question_queue.push_back(question_request());
        assert_eq!(
            rt.state(),
            SessionState::WaitingPermission,
            "permission outranks input"
        );
    }

    #[test]
    fn preview_prefers_status_while_busy() {
        let mut rt = make_runtime();
        rt.agent_busy = true;
        rt.view.status_message = "Running tests…".into();
        assert_eq!(rt.preview(), "Running tests…");
    }

    #[test]
    fn preview_uses_last_non_empty_message_when_idle() {
        let mut rt = make_runtime();
        rt.view.messages = vec![
            msg(ChatRole::System, "sys"),
            msg(ChatRole::User, "hello"),
            msg(ChatRole::Assistant, "hi there\nsecond line"),
        ];
        assert_eq!(rt.preview(), "hi there");
    }

    #[test]
    fn preview_truncates_to_80_chars() {
        let mut rt = make_runtime();
        let long = "x".repeat(200);
        rt.view.messages = vec![msg(ChatRole::User, &long)];
        assert_eq!(rt.preview().chars().count(), 80);
    }

    #[test]
    fn preview_empty_when_no_messages() {
        let rt = make_runtime();
        assert_eq!(rt.preview(), "");
    }

    // ── helpers ────────────────────────────────────────────────────────

    /// Point the data dir at a throwaway temp dir once per test binary so
    /// `SessionRuntime::new` (which opens `whycode.db`) never touches real
    /// user data. Parallel tests share the same isolated root.
    fn isolate_data_dir() {
        use std::sync::OnceLock;
        static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
        unsafe { std::env::set_var("WHYCODE_HOME", dir.path()) };
    }

    fn perm_request() -> PermissionRequest {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        PermissionRequest {
            tool_name: "bash".into(),
            detail: "echo hi".into(),
            reply: tx,
        }
    }

    fn question_request() -> QuestionRequest {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        QuestionRequest {
            questions: vec![whycode_tools::question::QuestionSpec {
                prompt: "Pick?".into(),
                options: vec![whycode_tools::question::QuestionOption {
                    label: "A".into(),
                    description: String::new(),
                    preview: None,
                }],
                multi_select: false,
            }],
            reply: tx,
        }
    }

    fn make_runtime() -> SessionRuntime {
        use whycode_core::types::{AgentInfo, AgentMode, PermissionSet};
        use whycode_session::SessionHistory;
        use whycode_session::session::Session;

        isolate_data_dir();
        let info = AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        };
        let agent = whycode_agent::agent::Agent::new(info);
        let session = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
        let history = SessionHistory::new();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
        let (question_prompter, question_rx) = ChannelQuestionPrompter::new(None);
        SessionRuntime::new(
            agent,
            session,
            history,
            event_tx,
            event_rx,
            done_tx,
            done_rx,
            Arc::new(perm_prompter),
            Arc::new(question_prompter),
            perm_rx,
            question_rx,
        )
    }
}
