//! Per-session runtime state for the TUI event loop.
//!
//! S1 of the parallel multi-session plan: extracts what used to be loose
//! locals in `run()` into one struct so a later step can hold
//! `Vec<SessionRuntime>` and switch between live sessions. `run()` currently
//! creates exactly one runtime — behaviour is unchanged.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use whycode_agent::agent::Agent;
use whycode_agent::permission::{ChannelPermissionPrompter, PermissionRequest};
use whycode_agent::{CancelFlag, ChannelQuestionPrompter, QuestionRequest, TurnEvent};
use whycode_session::SessionHistory;
use whycode_session::session::Session;

use crate::run::TurnOutcome;

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
        }
    }
}
