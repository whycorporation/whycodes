//! Per-worker cancel requests from the UI (agent hub `k`).
//!
//! A `task` or `swarm` worker registers its id when it starts and leaves when
//! it ends. The UI asks to cancel a live id; the worker reads the request at
//! the start of its next step, stops, and returns "cancelled by the user" as
//! its result, so the parent's tool call still gets its tool result.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Shared, cloneable registry: the UI keeps a clone while the agent runs a turn.
#[derive(Clone, Default)]
pub struct WorkerCancels(Arc<Mutex<HashMap<String, bool>>>);

impl WorkerCancels {
    /// Mark `id` as running with no cancel request.
    pub fn register(&self, id: &str) {
        crate::agent::recover_lock(&self.0).insert(id.to_string(), false);
    }

    /// Ask the running worker `id` to stop. `false` when no such worker runs
    /// (already finished, or never started), so a late request cannot cancel a
    /// later worker that reuses the id.
    pub fn cancel(&self, id: &str) -> bool {
        match crate::agent::recover_lock(&self.0).get_mut(id) {
            Some(flag) => {
                *flag = true;
                true
            }
            None => false,
        }
    }

    /// True once the UI asked `id` to stop.
    pub fn is_cancelled(&self, id: &str) -> bool {
        crate::agent::recover_lock(&self.0)
            .get(id)
            .copied()
            .unwrap_or(false)
    }

    /// The worker ended; forget it.
    pub fn finish(&self, id: &str) {
        crate::agent::recover_lock(&self.0).remove(id);
    }
}

#[cfg(test)]
#[path = "worker_cancel_tests.rs"]
mod tests;
