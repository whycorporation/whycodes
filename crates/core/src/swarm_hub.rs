//! In-process mailbox for swarm workers (DM + broadcast).
//!
//! Process-local. Not a negotiation protocol — just a queue the worker loop
//! drains between turns.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One message on the swarm bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmMessage {
    pub from: String,
    pub to: String,
    pub text: String,
}

/// Toast / log hook for the parent UI.
pub type SwarmMessageListener = Arc<dyn Fn(SwarmMessage) + Send + Sync>;

/// Shared mailbox for one swarm run.
#[derive(Clone, Default)]
pub struct SwarmHub {
    inner: Arc<SwarmHubInner>,
}

struct SwarmHubInner {
    /// Inbox per recipient id (`parent`, `worker-0`, …).
    inboxes: Mutex<HashMap<String, Vec<SwarmMessage>>>,
    listener: Mutex<Option<SwarmMessageListener>>,
}

impl Default for SwarmHubInner {
    fn default() -> Self {
        Self {
            inboxes: Mutex::new(HashMap::new()),
            listener: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for SwarmHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = recover_lock(self.inner.inboxes.lock())
            .values()
            .map(|v| v.len())
            .sum::<usize>();
        f.debug_struct("SwarmHub").field("queued", &n).finish()
    }
}

impl SwarmHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a participant so broadcasts reach them even before they send.
    pub fn ensure(&self, id: &str) {
        recover_lock(self.inner.inboxes.lock())
            .entry(id.to_string())
            .or_default();
    }

    pub fn set_listener(&self, listener: Option<SwarmMessageListener>) {
        *recover_lock(self.inner.listener.lock()) = listener;
    }

    /// Deliver `text` from `from` to `to` (`all` broadcasts to every known inbox
    /// plus `parent`).
    pub fn send(&self, from: &str, to: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut boxes = recover_lock(self.inner.inboxes.lock());
        let targets: Vec<String> = if to.eq_ignore_ascii_case("all") {
            let mut ids: Vec<String> = boxes.keys().cloned().collect();
            if !ids.iter().any(|k| k == "parent") {
                ids.push("parent".into());
            }
            ids.into_iter().filter(|id| id != from).collect()
        } else {
            vec![to.to_string()]
        };
        for dest in targets {
            let msg = SwarmMessage {
                from: from.to_string(),
                to: dest.clone(),
                text: text.to_string(),
            };
            boxes.entry(dest).or_default().push(msg.clone());
            self.emit(&msg);
        }
    }

    fn emit(&self, msg: &SwarmMessage) {
        if let Some(ref f) = *recover_lock(self.inner.listener.lock()) {
            f(msg.clone());
        }
    }

    /// Take every pending message for `id`.
    pub fn drain(&self, id: &str) -> Vec<SwarmMessage> {
        recover_lock(self.inner.inboxes.lock())
            .remove(id)
            .unwrap_or_default()
    }
}

pub(crate) fn recover_lock<T>(res: std::sync::LockResult<T>) -> T {
    match res {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[cfg(test)]
impl SwarmHub {
    pub(crate) fn poison_inboxes(&self) {
        poison_mutex(&self.inner.inboxes);
    }
    pub(crate) fn poison_listener(&self) {
        poison_mutex(&self.inner.listener);
    }
}

#[cfg(test)]
fn poison_mutex<T>(m: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison");
    }));
}
