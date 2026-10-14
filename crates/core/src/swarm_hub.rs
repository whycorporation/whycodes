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
    /// plus `parent`). A direct message to an unregistered id is refused with
    /// the known ids, so a typo is not silently queued where nobody reads.
    pub fn send(&self, from: &str, to: &str, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let mut boxes = recover_lock(self.inner.inboxes.lock());
        let targets: Vec<String> = if to.eq_ignore_ascii_case("all") {
            let mut ids: Vec<String> = boxes.keys().cloned().collect();
            if !ids.iter().any(|k| k == "parent") {
                ids.push("parent".into());
            }
            ids.into_iter().filter(|id| id != from).collect()
        } else if boxes.contains_key(to) {
            vec![to.to_string()]
        } else {
            let mut known: Vec<&str> = boxes.keys().map(String::as_str).collect();
            known.sort_unstable();
            return Err(format!(
                "unknown recipient `{to}`; known: all, {}",
                known.join(", ")
            ));
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
        Ok(())
    }

    fn emit(&self, msg: &SwarmMessage) {
        if let Some(ref f) = *recover_lock(self.inner.listener.lock()) {
            f(msg.clone());
        }
    }

    /// Take every pending message for `id`. The inbox stays registered so
    /// later broadcasts still reach it.
    pub fn drain(&self, id: &str) -> Vec<SwarmMessage> {
        recover_lock(self.inner.inboxes.lock())
            .get_mut(id)
            .map(std::mem::take)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn send_broadcast_drain_and_listener() {
        let hub = SwarmHub::new();
        hub.ensure("parent");
        hub.ensure("w0");
        hub.send("w0", "parent", "   ").unwrap();
        hub.send("w0", "parent", "hello").unwrap();
        let got = hub.drain("parent");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "hello");
        assert!(hub.drain("parent").is_empty());

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        hub.set_listener(Some(Arc::new(move |msg: SwarmMessage| {
            seen2.lock().unwrap().push(msg.text);
        })));
        hub.send("parent", "all", "broadcast").unwrap();
        assert!(!hub.drain("w0").is_empty());
        assert!(!seen.lock().unwrap().is_empty());
        hub.set_listener(None);
        assert!(format!("{hub:?}").contains("SwarmHub"));
    }

    #[test]
    fn broadcast_still_reaches_an_inbox_after_it_was_drained() {
        let hub = SwarmHub::new();
        hub.ensure("parent");
        hub.ensure("worker-0");
        hub.ensure("worker-1");
        hub.send("worker-0", "worker-1", "first").unwrap();
        assert_eq!(hub.drain("worker-1").len(), 1);
        hub.send("worker-0", "all", "second").unwrap();
        let got = hub.drain("worker-1");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "second");
        assert!(hub.drain("worker-0").is_empty(), "sender skips itself");
        assert_eq!(hub.drain("parent").len(), 1);
    }
}
