//! `ignore`-crate walker: gitignore-aware, policy-pruned, capped, cancellable.
//!
//! The walker never follows symlinks (confinement: a scanned tree cannot
//! enumerate paths outside its root) and applies gitignore rules even outside
//! git checkouts (`require_git(false)`), mirroring ripgrep defaults.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ignore::{DirEntry, WalkBuilder, WalkState};

use crate::policy;

pub(crate) fn allow_dir_entry(depth: usize, name: &str, is_dir: bool) -> bool {
    if depth == 0 {
        return true;
    }
    if is_dir {
        !policy::is_pruned_dir(name)
    } else {
        !policy::is_pruned_file(name)
    }
}

#[cfg(test)]
pub(crate) fn rel_is_empty(rel: &str) -> bool {
    rel.is_empty()
}

#[cfg(test)]
pub(crate) fn entry_is_err<T, E>(r: &Result<T, E>) -> bool {
    r.is_err()
}

/// One discovered entry, root-relative with `/` separators.
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Relative path, e.g. `crates/tui/src/app.rs`.
    pub rel: Box<str>,
    /// True for directories (symlinks are reported as non-dirs).
    pub is_dir: bool,
    /// File size in bytes (0 for dirs and symlinks).
    pub size: u64,
}

/// Outcome of a walk.
#[derive(Debug, Default, Clone, Copy)]
pub struct WalkStats {
    /// Entries delivered to the sink.
    pub scanned: usize,
    /// True when the walk stopped early at the entry cap.
    pub truncated: bool,
}

/// Walk `root` recursively, invoking `on_entry` for every accepted entry.
///
/// `on_entry` may be called from multiple worker threads (it must be `Sync`).
/// `scanned` is incremented per delivered entry (drives progress UIs).
/// The walk stops early when `cancel` is set or `max_entries` is reached.
pub fn walk_root(
    root: &Path,
    threads: usize,
    max_entries: usize,
    scanned: &AtomicUsize,
    cancel: &AtomicBool,
    on_entry: &(dyn Fn(WalkEntry) + Sync),
) -> WalkStats {
    walk_root_limited(
        root,
        threads,
        max_entries,
        usize::MAX,
        scanned,
        cancel,
        on_entry,
    )
}

/// Like [`walk_root`] with a directory-depth cap (`max_depth`, 1 = children of
/// `root` only). Used by the `list` tool's recursive mode.
pub fn walk_root_limited(
    root: &Path,
    threads: usize,
    max_entries: usize,
    max_depth: usize,
    scanned: &AtomicUsize,
    cancel: &AtomicBool,
    on_entry: &(dyn Fn(WalkEntry) + Sync),
) -> WalkStats {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // hidden entries are filtered by our policy (whitelisted ones stay)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .require_git(false)
        .max_depth(if max_depth == usize::MAX {
            None
        } else {
            Some(max_depth)
        })
        .threads(threads.max(1));

    // `filter_entry` drives descent control in both serial and parallel mode:
    // a pruned directory is never descended into.
    builder.filter_entry(|entry| {
        allow_dir_entry(
            entry.depth(),
            &entry.file_name().to_string_lossy(),
            entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false),
        )
    });

    // Root (depth 0) is always entered; hidden/pruned names are skipped.

    let accept = |entry: &DirEntry| -> Option<WalkEntry> {
        if entry.depth() == 0 {
            return None; // the root itself is not an entry
        }
        let ft = entry.file_type()?;
        let is_symlink = ft.is_symlink();
        let is_dir = ft.is_dir() && !is_symlink;
        let rel = entry.path().strip_prefix(root).ok()?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        let size = if is_dir || is_symlink {
            0
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        Some(WalkEntry {
            rel: rel.into_boxed_str(),
            is_dir,
            size,
        })
    };

    let delivered = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);

    let visit = |entry: Result<DirEntry, ignore::Error>| -> WalkState {
        if cancel.load(Ordering::Relaxed) {
            return WalkState::Quit;
        }
        let Some(entry) = entry.ok() else {
            return WalkState::Continue;
        };
        let Some(we) = accept(&entry) else {
            return WalkState::Continue;
        };
        // Check-then-inc: parallel walkers may overshoot the cap by a few
        // entries — acceptable for a guard rail, exact in serial mode.
        if delivered.load(Ordering::Relaxed) >= max_entries {
            truncated.store(true, Ordering::Relaxed);
            return WalkState::Quit;
        }
        delivered.fetch_add(1, Ordering::Relaxed);
        scanned.fetch_add(1, Ordering::Relaxed);
        on_entry(we);
        WalkState::Continue
    };

    if threads > 1 {
        let visit = &visit;
        builder.build_parallel().run(|| Box::new(visit));
    } else {
        // Serial fallback: `Walk` spawns no threads of its own.
        let visit = &visit;
        for entry in builder.build() {
            if matches!(visit(entry), WalkState::Quit) {
                break;
            }
        }
    }

    WalkStats {
        scanned: delivered.load(Ordering::Relaxed),
        truncated: truncated.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
