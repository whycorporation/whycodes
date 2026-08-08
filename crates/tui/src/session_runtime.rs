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

use crate::app::{AgentState, ChatMessage, ChatRole};
use crate::run::TurnOutcome;

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
    /// Last turn ended in error (sticky until a new turn starts).
    pub last_error: bool,
    /// When this runtime was created (dashboard age).
    pub created_at: std::time::Instant,
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
        self.view
            .messages
            .iter()
            .rev()
            .find(|m| {
                matches!(m.role, ChatRole::Assistant | ChatRole::User) && !m.content.is_empty()
            })
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
}
