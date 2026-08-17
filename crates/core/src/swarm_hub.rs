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
        let n = self
            .inner
            .inboxes
            .lock()
            .map(|m| m.values().map(|v| v.len()).sum::<usize>())
            .unwrap_or(0);
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
        if let Ok(mut slot) = self.inner.listener.lock() {
            *slot = listener;
        }
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
            if !ids.iter().any(|k| k == from) {
                // still deliver to parent even if sender is the only inbox
            }
            if ids.is_empty() {
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
        if let Ok(slot) = self.inner.listener.lock()
            && let Some(ref f) = *slot
        {
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

fn recover_lock<T>(res: std::sync::LockResult<T>) -> T {
    match res {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_reaches_only_target() {
        let hub = SwarmHub::new();
        hub.send("worker-0", "worker-1", "hello");
        assert!(hub.drain("worker-0").is_empty());
        let got = hub.drain("worker-1");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "hello");
        assert!(hub.drain("worker-1").is_empty());
    }

    #[test]
    fn broadcast_skips_sender() {
        let hub = SwarmHub::new();
        hub.ensure("parent");
        hub.ensure("worker-0");
        hub.ensure("worker-1");
        hub.send("worker-0", "all", "heads up");
        let parent = hub.drain("parent");
        assert!(parent.iter().any(|m| m.text == "heads up"));
        assert!(hub.drain("worker-0").is_empty());
        let w1 = hub.drain("worker-1");
        assert!(w1.iter().any(|m| m.text == "heads up"));
    }

    #[test]
    fn empty_send_listener_and_broadcast_without_inboxes() {
        let hub = SwarmHub::new();
        let _ = format!("{hub:?}");
        hub.send("a", "b", "   ");
        assert!(hub.drain("b").is_empty());

        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits2 = std::sync::Arc::clone(&hits);
        hub.set_listener(Some(std::sync::Arc::new(move |_| {
            hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })));
        hub.send("solo", "all", "ping");
        assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        let parent = hub.drain("parent");
        assert!(parent.iter().any(|m| m.text == "ping"));
        hub.set_listener(None);
        hub.ensure("late");
        hub.send("late", "all", "later");
        assert!(hub.drain("parent").iter().any(|m| m.text == "later"));
    }

    #[test]
    fn recover_lock_poisoned() {
        let m = std::sync::Arc::new(std::sync::Mutex::new(1));
        let m2 = std::sync::Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert_eq!(*recover_lock(m.lock()), 1);
    }
}
