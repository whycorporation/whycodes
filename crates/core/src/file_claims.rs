//! File ownership claims for concurrent swarm agents.
//!
//! When several agents edit one checkout, each mutator (`write` / `edit` /
//! `apply_patch`) must claim the target path first. A second agent that tries
//! the same path gets a structured conflict instead of a silent overwrite.
//!
//! Claims are process-local (in-memory). They do not replace git worktrees for
//! hard isolation; they are the lightweight conflict-notify layer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Who holds a path while a swarm (or multi-writer) session is active.
#[derive(Debug, Clone)]
pub struct FileClaim {
    /// Stable id for the agent (e.g. `worker-0`).
    pub owner_id: String,
    /// Human-readable label for toasts / tool errors.
    pub owner_label: String,
}

/// Result of attempting to claim a file for writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimResult {
    /// Path was free; now owned by the claimant.
    Acquired,
    /// Same agent already owns the path.
    Held,
    /// Another agent owns the path.
    Conflict {
        owner_id: String,
        owner_label: String,
    },
}

/// Emitted when a claim fails because another agent already owns the path.
#[derive(Debug, Clone)]
pub struct FileConflictEvent {
    pub path: String,
    pub claimant_id: String,
    pub claimant_label: String,
    pub owner_id: String,
    pub owner_label: String,
}

/// Optional listener for conflict notifications (TUI toast, logs, …).
pub type ConflictListener = Arc<dyn Fn(FileConflictEvent) + Send + Sync>;

/// Shared registry of file claims for one swarm (or multi-writer) run.
#[derive(Clone, Default)]
pub struct FileClaimRegistry {
    inner: Arc<FileClaimRegistryInner>,
}

/// Last writer of a path (survives `release_agent` so later reads can go stale).
#[derive(Debug, Clone)]
struct WriteRecord {
    owner_id: String,
    owner_label: String,
    generation: u64,
}

/// A reader opened a file another agent wrote since their last read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStaleEvent {
    pub path: String,
    pub reader_id: String,
    pub writer_id: String,
    pub writer_label: String,
}

/// Optional listener for stale-read notifications.
pub type StaleListener = Arc<dyn Fn(FileStaleEvent) + Send + Sync>;

struct FileClaimRegistryInner {
    /// Canonical absolute path string → claim.
    claims: Mutex<HashMap<String, FileClaim>>,
    listener: Mutex<Option<ConflictListener>>,
    last_write: Mutex<HashMap<String, WriteRecord>>,
    /// `(agent_id, path)` → last write gen this agent has seen.
    last_seen: Mutex<HashMap<(String, String), u64>>,
    stale_listener: Mutex<Option<StaleListener>>,
}

impl Default for FileClaimRegistryInner {
    fn default() -> Self {
        Self {
            claims: Mutex::new(HashMap::new()),
            listener: Mutex::new(None),
            last_write: Mutex::new(HashMap::new()),
            last_seen: Mutex::new(HashMap::new()),
            stale_listener: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for FileClaimRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = recover_lock(self.inner.claims.lock()).len();
        f.debug_struct("FileClaimRegistry")
            .field("claims", &n)
            .finish()
    }
}

impl FileClaimRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install (or replace) the conflict listener.
    pub fn set_listener(&self, listener: Option<ConflictListener>) {
        *recover_lock(self.inner.listener.lock()) = listener;
    }

    /// Normalize a path for claim keys: absolute, no trailing slash noise.
    pub fn claim_key(path: &Path) -> String {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            path_or_dot(std::env::current_dir()).join(path)
        };
        // Prefer canonicalize when the path exists; otherwise use cleaned abs.
        let normalized = abs.canonicalize().unwrap_or_else(|_| {
            // Clean `..` / `.` components without requiring existence.
            let mut out = PathBuf::new();
            for comp in abs.components() {
                apply_component(&mut out, comp);
            }
            out
        });
        normalized.to_string_lossy().into_owned()
    }

    /// Try to claim `path` for `agent_id`. Same agent re-claim is a no-op success.
    pub fn try_claim(&self, agent_id: &str, agent_label: &str, path: &Path) -> ClaimResult {
        let key = Self::claim_key(path);
        let mut map = recover_lock(self.inner.claims.lock());
        match map.get(&key) {
            Some(c) if c.owner_id == agent_id => {
                self.record_write(&key, agent_id, agent_label);
                ClaimResult::Held
            }
            Some(c) => {
                let ev = FileConflictEvent {
                    path: key,
                    claimant_id: agent_id.to_string(),
                    claimant_label: agent_label.to_string(),
                    owner_id: c.owner_id.clone(),
                    owner_label: c.owner_label.clone(),
                };
                self.emit_conflict(ev.clone());
                ClaimResult::Conflict {
                    owner_id: ev.owner_id,
                    owner_label: ev.owner_label,
                }
            }
            None => {
                map.insert(
                    key.clone(),
                    FileClaim {
                        owner_id: agent_id.to_string(),
                        owner_label: agent_label.to_string(),
                    },
                );
                self.record_write(&key, agent_id, agent_label);
                ClaimResult::Acquired
            }
        }
    }

    fn record_write(&self, key: &str, agent_id: &str, agent_label: &str) {
        let mut writes = recover_lock(self.inner.last_write.lock());
        let generation = writes
            .get(key)
            .map(|w| w.generation)
            .unwrap_or(0)
            .saturating_add(1);
        writes.insert(
            key.to_string(),
            WriteRecord {
                owner_id: agent_id.to_string(),
                owner_label: agent_label.to_string(),
                generation,
            },
        );
        recover_lock(self.inner.last_seen.lock())
            .insert((agent_id.to_string(), key.to_string()), generation);
    }

    pub fn set_stale_listener(&self, listener: Option<StaleListener>) {
        *recover_lock(self.inner.stale_listener.lock()) = listener;
    }

    /// Record a read. If another agent wrote since this reader last saw the
    /// path, return a stale event (and notify the listener).
    pub fn note_read(&self, agent_id: &str, path: &Path) -> Option<FileStaleEvent> {
        let key = Self::claim_key(path);
        let writes = recover_lock(self.inner.last_write.lock());
        let write = writes.get(&key)?;
        if write.owner_id == agent_id {
            return None;
        }
        let generation = write.generation;
        let writer_id = write.owner_id.clone();
        let writer_label = write.owner_label.clone();
        drop(writes);

        let mut seen = recover_lock(self.inner.last_seen.lock());
        let last = seen.get(&(agent_id.to_string(), key.clone())).copied();
        seen.insert((agent_id.to_string(), key.clone()), generation);
        if last == Some(generation) {
            return None;
        }
        let ev = FileStaleEvent {
            path: key,
            reader_id: agent_id.to_string(),
            writer_id,
            writer_label,
        };
        if let Some(ref f) = *recover_lock(self.inner.stale_listener.lock()) {
            f(ev.clone());
        }
        Some(ev)
    }

    fn emit_conflict(&self, ev: FileConflictEvent) {
        if let Some(ref f) = *recover_lock(self.inner.listener.lock()) {
            f(ev);
        }
    }

    /// Drop every claim held by `agent_id` (worker finished).
    pub fn release_agent(&self, agent_id: &str) {
        let mut map = recover_lock(self.inner.claims.lock());
        map.retain(|_, c| c.owner_id != agent_id);
    }

    /// Clear all claims (swarm session end).
    pub fn clear(&self) {
        recover_lock(self.inner.claims.lock()).clear();
    }

    /// Snapshot of current claims (path → owner label), sorted by path.
    pub fn snapshot(&self) -> Vec<(String, String)> {
        let map = recover_lock(self.inner.claims.lock());
        let mut rows: Vec<_> = map
            .iter()
            .map(|(p, c)| (p.clone(), c.owner_label.clone()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    pub fn len(&self) -> usize {
        recover_lock(self.inner.claims.lock()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub(crate) fn apply_component(out: &mut PathBuf, comp: std::path::Component<'_>) {
    match comp {
        std::path::Component::ParentDir => {
            out.pop();
        }
        std::path::Component::CurDir => {}
        other => out.push(other.as_os_str()),
    }
}

pub(crate) fn path_or_dot(res: std::io::Result<PathBuf>) -> PathBuf {
    res.unwrap_or_else(|_| PathBuf::from("."))
}

pub(crate) fn recover_lock<T>(res: std::sync::LockResult<T>) -> T {
    match res {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

#[cfg(test)]
impl FileClaimRegistry {
    pub(crate) fn poison_claims(&self) {
        poison_mutex(&self.inner.claims);
    }
    pub(crate) fn poison_last_write(&self) {
        poison_mutex(&self.inner.last_write);
    }
    pub(crate) fn poison_last_seen(&self) {
        poison_mutex(&self.inner.last_seen);
    }
    pub(crate) fn poison_listener(&self) {
        poison_mutex(&self.inner.listener);
    }
    pub(crate) fn poison_stale_listener(&self) {
        poison_mutex(&self.inner.stale_listener);
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
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[test]
    fn claim_hold_conflict_stale_and_snapshot() {
        let reg = FileClaimRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        let a = Path::new("src/a.rs");
        assert_eq!(reg.try_claim("w0", "worker-0", a), ClaimResult::Acquired);
        assert_eq!(reg.try_claim("w0", "worker-0", a), ClaimResult::Held);
        assert_eq!(
            reg.try_claim("w1", "worker-1", a),
            ClaimResult::Conflict {
                owner_id: "w0".into(),
                owner_label: "worker-0".into(),
            }
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        reg.set_listener(Some(Arc::new(move |ev: FileConflictEvent| {
            seen2.lock().unwrap().push(ev.owner_id);
        })));
        let _ = reg.try_claim("w1", "worker-1", a);
        assert!(!seen.lock().unwrap().is_empty());

        let stale = Arc::new(Mutex::new(0usize));
        let stale2 = Arc::clone(&stale);
        reg.set_stale_listener(Some(Arc::new(move |_ev: FileStaleEvent| {
            *stale2.lock().unwrap() += 1;
        })));
        assert!(reg.note_read("w0", a).is_none());
        assert!(reg.note_read("w1", a).is_some());
        assert_eq!(*stale.lock().unwrap(), 1);
        assert!(reg.note_read("w1", a).is_none());
        assert_eq!(*stale.lock().unwrap(), 1);
        assert!(format!("{reg:?}").contains("FileClaimRegistry"));
        assert!(!reg.snapshot().is_empty());
        assert!(!FileClaimRegistry::claim_key(a).is_empty());
        reg.release_agent("w0");
        reg.clear();
        assert!(reg.is_empty());
    }
}
