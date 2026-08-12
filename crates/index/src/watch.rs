//! Live updates: `notify` watcher → policy-filtered change stream.
//!
//! Events land on a channel consumed by the index scanner thread, which
//! debounces them (save storms, `cargo build`, git checkouts) into batches.
//! When the watcher cannot be installed (inotify limits, exotic filesystems)
//! the index still works — it just stops self-refreshing until `rescan()`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};

use crate::policy;

/// Commands understood by the index scanner thread.
#[derive(Debug)]
pub enum Command {
    /// Debounced batch of filesystem changes.
    Batch(Vec<Change>),
    /// Full rescan requested by the user.
    Rescan,
    /// Scanner thread should exit.
    Shutdown,
}

/// One filesystem change, root-relative.
#[derive(Debug, Clone)]
pub struct Change {
    /// Index into the roots vector.
    pub root: u16,
    /// Root-relative path with `/` separators.
    pub rel: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Created or modified — re-stat and upsert.
    Upsert,
    /// Deleted (or renamed away) — remove entry and descendants.
    Remove,
}

/// Spawn one recursive watcher covering all roots. Events are mapped to
/// root-relative [`Change`]s and forwarded on `tx`.
///
/// Returns `None` when no watcher could be installed.
pub fn spawn(roots: &[PathBuf], tx: Sender<Command>) -> Option<RecommendedWatcher> {
    let watched: Vec<PathBuf> = roots.to_vec();
    let mut watcher = match RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else { return };
            let changes = map_event(&watched, &event);
            if !changes.is_empty() {
                let _ = tx.send(Command::Batch(changes));
            }
        },
        notify::Config::default(),
    ) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "file watcher unavailable; index will not self-refresh");
            return None;
        }
    };

    let mut any = false;
    for root in roots {
        match watcher.watch(root, RecursiveMode::Recursive) {
            Ok(()) => any = true,
            Err(e) => {
                tracing::warn!(root = %root.display(), error = %e, "cannot watch root");
            }
        }
    }
    any.then_some(watcher)
}

/// Translate one notify event into policy-filtered changes.
fn map_event(roots: &[PathBuf], event: &Event) -> Vec<Change> {
    use notify::event::{CreateKind, RemoveKind};
    let mut out = Vec::new();
    match event.kind {
        EventKind::Create(CreateKind::File | CreateKind::Folder | CreateKind::Any) => {
            for path in &event.paths {
                push_change(roots, path, ChangeKind::Upsert, &mut out);
            }
        }
        EventKind::Remove(RemoveKind::File | RemoveKind::Folder | RemoveKind::Any) => {
            for path in &event.paths {
                push_change(roots, path, ChangeKind::Remove, &mut out);
            }
        }
        EventKind::Modify(ModifyKind::Name(_)) => {
            // Rename: notify yields [from, to] (platforms with both cookies)
            // or two single-path events. Treat `from` as Remove, `to` as
            // Upsert; single-path forms degrade to Upsert (a stale entry may
            // linger until the next rescan — acceptable for a picker index).
            if event.paths.len() == 2 {
                push_change(roots, &event.paths[0], ChangeKind::Remove, &mut out);
                push_change(roots, &event.paths[1], ChangeKind::Upsert, &mut out);
            } else {
                for path in &event.paths {
                    push_change(roots, path, ChangeKind::Upsert, &mut out);
                }
            }
        }
        EventKind::Modify(_) => {
            for path in &event.paths {
                push_change(roots, path, ChangeKind::Upsert, &mut out);
            }
        }
        _ => {} // access noise, attribute-only events we don't track
    }
    out
}

fn push_change(roots: &[PathBuf], path: &Path, kind: ChangeKind, out: &mut Vec<Change>) {
    for (idx, root) in roots.iter().enumerate() {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() || !policy::rel_path_allowed(&rel) {
            return;
        }
        out.push(Change {
            root: idx as u16,
            rel,
            kind,
        });
        return; // first matching root owns the path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_mapping_filters_policy() {
        let roots = vec![PathBuf::from("/proj")];
        let mut out = Vec::new();
        push_change(&roots, Path::new("/proj/src/main.rs"), ChangeKind::Upsert, &mut out);
        push_change(&roots, Path::new("/proj/target/x.o"), ChangeKind::Upsert, &mut out);
        push_change(&roots, Path::new("/proj/.env"), ChangeKind::Upsert, &mut out);
        push_change(&roots, Path::new("/other/f.rs"), ChangeKind::Upsert, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rel, "src/main.rs");
        assert_eq!(out[0].root, 0);
    }
}
