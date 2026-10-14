//! Messages the user sends while a turn is running.
//!
//! The UI pushes text here instead of waiting for the turn to end. The turn
//! loop drains the inbox at the start of each step, after the previous tool
//! results, so the model reads the message before its next request and every
//! tool call already has its result. Whatever is still here when the turn
//! ends was not read; the UI takes it back and sends it as the next turn.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Shared, cloneable inbox: the UI keeps a clone while the agent runs a turn.
#[derive(Clone, Default)]
pub struct SteerInbox(Arc<Mutex<VecDeque<String>>>);

impl SteerInbox {
    pub fn push(&self, text: String) {
        crate::agent::recover_lock(&self.0).push_back(text);
    }

    /// Every waiting message, oldest first.
    pub fn drain(&self) -> Vec<String> {
        crate::agent::recover_lock(&self.0).drain(..).collect()
    }
}

#[cfg(test)]
#[path = "steer_tests.rs"]
mod tests;
