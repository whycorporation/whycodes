//! Messages the user sends while a turn is running.
//!
//! The UI pushes a message here instead of waiting for the turn to end. The
//! turn loop drains the session's queue at the start of each step, after the
//! previous tool results, so the model reads the message before its next
//! request and every tool call already has its result. Whatever is still here
//! when the turn ends was not read; the UI takes it back and sends it as the
//! next turn.
//!
//! Each message carries a [`SteerMode`]. `NextStep` (the default) waits for
//! the current tool calls; `Urgent` also skips the calls of a sequential
//! batch that have not started yet; `Queue` is never read by the turn and
//! comes back as the next turn.
//!
//! Queues are keyed by session id: the daemon runs many sessions on one agent.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use whycodes_core::types::MessageContent;

/// When the running turn reads a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerMode {
    /// Before the next request, after the current tool calls finish.
    #[default]
    NextStep,
    /// Never read by the turn: it runs as the next user turn.
    Queue,
    /// Before the next call of a sequential batch: the calls not started yet
    /// get a "not run" result and the next request carries the message.
    Urgent,
}

type Queue = VecDeque<(SteerMode, MessageContent)>;

/// Shared, cloneable inbox: the UI keeps a clone while the agent runs a turn.
#[derive(Clone, Default)]
pub struct SteerInbox(Arc<Mutex<HashMap<String, Queue>>>);

impl SteerInbox {
    /// Queue `content` for the next step ([`SteerMode::NextStep`]).
    pub fn push(&self, session_id: &str, content: MessageContent) {
        self.push_mode(session_id, SteerMode::NextStep, content);
    }

    pub fn push_mode(&self, session_id: &str, mode: SteerMode, content: MessageContent) {
        crate::agent::recover_lock(&self.0)
            .entry(session_id.to_string())
            .or_default()
            .push_back((mode, content));
    }

    /// Every message waiting for `session_id`, oldest first.
    pub fn drain(&self, session_id: &str) -> Vec<MessageContent> {
        let queue = crate::agent::recover_lock(&self.0).remove(session_id);
        queue
            .into_iter()
            .flatten()
            .map(|(_, content)| content)
            .collect()
    }

    /// The messages the running turn reads now, oldest first; queued ones
    /// ([`SteerMode::Queue`]) stay for the end of the turn.
    pub fn take_for_turn(&self, session_id: &str) -> Vec<MessageContent> {
        let mut map = crate::agent::recover_lock(&self.0);
        let Some(queue) = map.get_mut(session_id) else {
            return Vec::new();
        };
        let (queued, read): (Queue, Queue) = queue
            .drain(..)
            .partition(|(mode, _)| *mode == SteerMode::Queue);
        *queue = queued;
        read.into_iter().map(|(_, content)| content).collect()
    }

    /// Whether an urgent message waits for `session_id`.
    pub fn has_urgent(&self, session_id: &str) -> bool {
        crate::agent::recover_lock(&self.0)
            .get(session_id)
            .is_some_and(|q| q.iter().any(|(mode, _)| *mode == SteerMode::Urgent))
    }
}

/// Append a steering message to `session` as a user message.
pub fn add_to_session(session: &mut whycodes_session::session::Session, content: MessageContent) {
    match content {
        MessageContent::Text(text) => session.add_user_message(&text),
        MessageContent::Blocks(blocks) => session.add_user_message_blocks(blocks),
    }
}

/// The text a message carries (image blocks dropped), for clients that can
/// only replay text.
pub fn content_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                whycodes_core::types::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
#[path = "steer_tests.rs"]
mod tests;
