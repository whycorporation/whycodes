//! `ignore`-crate walker: gitignore-aware, policy-pruned, capped, cancellable.
//!
//! The walker never follows symlinks (confinement: a scanned tree cannot
//! enumerate paths outside its root) and applies gitignore rules even outside
//! git checkouts (`require_git(false)`), mirroring ripgrep defaults.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ignore::{DirEntry, WalkBuilder, WalkState};

use crate::policy;

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
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // hidden entries are filtered by our policy (whitelisted ones stay)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .require_git(false)
        .threads(threads.max(1));

    // `filter_entry` drives descent control in both serial and parallel mode:
    // a pruned directory is never descended into.
    builder.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true; // the root itself
        }
        let name = entry.file_name().to_string_lossy();
        match entry.file_type() {
            Some(ft) if ft.is_dir() => !policy::is_pruned_dir(&name),
            // Files and symlinks (never followed) are name-filtered only.
            _ => !policy::is_pruned_file(&name),
        }
    });

    let accept = |entry: &DirEntry| -> Option<WalkEntry> {
        if entry.depth() == 0 {
            return None; // the root itself is not an entry
        }
        let ft = entry.file_type()?;
        let is_symlink = ft.is_symlink();
        let is_dir = ft.is_dir() && !is_symlink;
        let rel = entry.path().strip_prefix(root).ok()?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            return None;
        }
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
        let Ok(entry) = entry else {
            return WalkState::Continue; // unreadable entries never block a scan
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
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    fn collect(root: &Path, threads: usize) -> Vec<String> {
        let scanned = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let out = Mutex::new(Vec::new());
        walk_root(root, threads, 100_000, &scanned, &cancel, &|e| {
            out.lock()
                .unwrap()
                .push(format!("{}{}", e.rel, if e.is_dir { "/" } else { "" }));
        });
        out.into_inner().unwrap()
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("README.md"), "hi").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/x.o"), "bin").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/i.js"), "x").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "x").unwrap();
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/ci.yml"), "on: push").unwrap();
        fs::write(root.join(".env"), "SECRET=1").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "x").unwrap();
        dir
    }

    #[test]
    fn walk_respects_policy_and_gitignore() {
        for threads in [1, 4] {
            let dir = fixture();
            let entries = collect(dir.path(), threads);
            assert!(
                entries.iter().any(|e| e == "src/main.rs"),
                "threads={threads}: {entries:?}"
            );
            assert!(entries.iter().any(|e| e == "src/"), "threads={threads}");
            assert!(
                entries.iter().any(|e| e == ".github/workflows/ci.yml"),
                "threads={threads}: {entries:?}"
            );
            assert!(entries.iter().any(|e| e == ".gitignore"));
            for bad in [
                "target/debug/x.o",
                "node_modules/pkg/i.js",
                ".git/config",
                ".env",
                "ignored.txt",
            ] {
                assert!(
                    !entries.iter().any(|e| e == bad),
                    "threads={threads}: {bad} must be excluded: {entries:?}"
                );
            }
        }
    }

    #[test]
    fn walk_caps_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        for i in 0..100 {
            fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let scanned = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let count = AtomicUsize::new(0);
        let stats = walk_root(dir.path(), 1, 10, &scanned, &cancel, &|_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        assert!(stats.truncated);
        assert_eq!(stats.scanned, 10);
        assert_eq!(count.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn walk_is_cancellable() {
        let dir = tempfile::TempDir::new().unwrap();
        for i in 0..100 {
            fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let scanned = AtomicUsize::new(0);
        let cancel = AtomicBool::new(true); // pre-cancelled
        let stats = walk_root(dir.path(), 1, 100_000, &scanned, &cancel, &|_| {});
        assert_eq!(stats.scanned, 0);
        assert!(!stats.truncated);
    }
}
