//! Messages the user sends while a turn is running.
//!
//! The UI pushes a message here instead of waiting for the turn to end. The
//! turn loop drains the session's queue at the start of each step, after the
//! previous tool results, so the model reads the message before its next
//! request and every tool call already has its result. Whatever is still here
//! when the turn ends was not read; the UI takes it back and sends it as the
//! next turn.
//!
//! Queues are keyed by session id: the daemon runs many sessions on one agent.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use whycodes_core::types::MessageContent;

/// Shared, cloneable inbox: the UI keeps a clone while the agent runs a turn.
#[derive(Clone, Default)]
pub struct SteerInbox(Arc<Mutex<HashMap<String, VecDeque<MessageContent>>>>);

impl SteerInbox {
    pub fn push(&self, session_id: &str, content: MessageContent) {
        crate::agent::recover_lock(&self.0)
            .entry(session_id.to_string())
            .or_default()
            .push_back(content);
    }

    /// Every message waiting for `session_id`, oldest first.
    pub fn drain(&self, session_id: &str) -> Vec<MessageContent> {
        let queue = crate::agent::recover_lock(&self.0).remove(session_id);
        queue.map(Vec::from).unwrap_or_default()
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
