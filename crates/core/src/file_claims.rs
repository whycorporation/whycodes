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

struct FileClaimRegistryInner {
    /// Canonical absolute path string → claim.
    claims: Mutex<HashMap<String, FileClaim>>,
    listener: Mutex<Option<ConflictListener>>,
}

impl Default for FileClaimRegistryInner {
    fn default() -> Self {
        Self {
            claims: Mutex::new(HashMap::new()),
            listener: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for FileClaimRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.inner.claims.lock().map(|m| m.len()).unwrap_or(0);
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
        if let Ok(mut slot) = self.inner.listener.lock() {
            *slot = listener;
        }
    }

    /// Normalize a path for claim keys: absolute, no trailing slash noise.
    pub fn claim_key(path: &Path) -> String {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
        // Prefer canonicalize when the path exists; otherwise use cleaned abs.
        let normalized = abs.canonicalize().unwrap_or_else(|_| {
            // Clean `..` / `.` components without requiring existence.
            let mut out = PathBuf::new();
            for comp in abs.components() {
                match comp {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => out.push(other.as_os_str()),
                }
            }
            out
        });
        normalized.to_string_lossy().into_owned()
    }

    /// Try to claim `path` for `agent_id`. Same agent re-claim is a no-op success.
    pub fn try_claim(&self, agent_id: &str, agent_label: &str, path: &Path) -> ClaimResult {
        let key = Self::claim_key(path);
        let mut map = match self.inner.claims.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match map.get(&key) {
            Some(c) if c.owner_id == agent_id => ClaimResult::Held,
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
                    key,
                    FileClaim {
                        owner_id: agent_id.to_string(),
                        owner_label: agent_label.to_string(),
                    },
                );
                ClaimResult::Acquired
            }
        }
    }

    fn emit_conflict(&self, ev: FileConflictEvent) {
        if let Ok(slot) = self.inner.listener.lock()
            && let Some(ref f) = *slot
        {
            f(ev);
        }
    }

    /// Drop every claim held by `agent_id` (worker finished).
    pub fn release_agent(&self, agent_id: &str) {
        let mut map = match self.inner.claims.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        map.retain(|_, c| c.owner_id != agent_id);
    }

    /// Clear all claims (swarm session end).
    pub fn clear(&self) {
        if let Ok(mut map) = self.inner.claims.lock() {
            map.clear();
        } else if let Err(p) = self.inner.claims.lock() {
            p.into_inner().clear();
        }
    }

    /// Snapshot of current claims (path → owner label), sorted by path.
    pub fn snapshot(&self) -> Vec<(String, String)> {
        let map = match self.inner.claims.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let mut rows: Vec<_> = map
            .iter()
            .map(|(p, c)| (p.clone(), c.owner_label.clone()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    pub fn len(&self) -> usize {
        self.inner.claims.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn first_claim_acquires_second_same_agent_holds() {
        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycode_claim_test_a.rs");
        assert_eq!(reg.try_claim("w0", "worker-0", p), ClaimResult::Acquired);
        assert_eq!(reg.try_claim("w0", "worker-0", p), ClaimResult::Held);
    }

    #[test]
    fn second_agent_conflicts_and_notifies() {
        let reg = FileClaimRegistry::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = Arc::clone(&hits);
        reg.set_listener(Some(Arc::new(move |_ev| {
            hits2.fetch_add(1, Ordering::SeqCst);
        })));
        let p = Path::new("/tmp/whycode_claim_test_b.rs");
        assert_eq!(reg.try_claim("w0", "worker-0", p), ClaimResult::Acquired);
        match reg.try_claim("w1", "worker-1", p) {
            ClaimResult::Conflict {
                owner_id,
                owner_label,
            } => {
                assert_eq!(owner_id, "w0");
                assert_eq!(owner_label, "worker-0");
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn release_agent_frees_paths() {
        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycode_claim_test_c.rs");
        reg.try_claim("w0", "worker-0", p);
        reg.release_agent("w0");
        assert_eq!(reg.try_claim("w1", "worker-1", p), ClaimResult::Acquired);
    }
}
