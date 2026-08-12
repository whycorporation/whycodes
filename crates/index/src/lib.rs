//! whycode-index — fast, gitignore-aware workspace file index.
//!
//! One background scan of the project roots feeds every consumer:
//!
//! * **TUI file picker** — fuzzy path queries (`@file`, Ctrl+Space) served by
//!   a resident [`nucleo`] engine; keystroke queries never touch the disk.
//! * **File tools** — `glob` / `list` / `grep` enumerate the in-memory store
//!   instead of re-walking the tree on every tool call.
//!
//! The walk honours `.gitignore` / `.ignore` (via the [`ignore`] crate, the
//! same engine ripgrep uses) plus the shared pruning [`policy`]. After the
//! initial scan a `notify` watcher streams debounced deltas so the index
//! stays warm as files are created, edited and deleted during a session.
//!
//! ```no_run
//! let index = whycode_index::WorkspaceIndex::start(vec![".".into()]);
//! let hits = index.query("main.rs", 20);
//! ```

pub mod policy;
mod fuzzy;
mod store;
pub mod walk;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use fuzzy::FileMatch;
pub use store::{Entry, IndexStore};
pub use walk::{WalkEntry, WalkStats, walk_root};
use fuzzy::FuzzyEngine;
use watch::{Change, ChangeKind, Command};

/// Default cap on indexed entries per root.
pub const DEFAULT_MAX_ENTRIES: usize = 200_000;

/// Debounce window for watcher events (save storms, builds, checkouts).
const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

const STATE_SCANNING: u8 = 0;
const STATE_READY: u8 = 1;

/// Scan progress reported by [`WorkspaceIndex::status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    /// Initial walk still running; fuzzy queries already return partial hits.
    Scanning {
        /// Entries discovered so far (all roots).
        scanned: usize,
    },
    /// Index warm; tools may skip their own walks.
    Ready {
        /// Total indexed entries.
        total: usize,
        /// True when the entry cap stopped the scan early.
        truncated: bool,
    },
}

/// Options for [`WorkspaceIndex::start_with`].
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Keep the index fresh with a filesystem watcher (default true).
    pub watch: bool,
    /// Hard cap on indexed entries per root (default 200k).
    pub max_entries: usize,
    /// Walk threads per root; 0 = auto (min(8, cores)), 1 = serial.
    pub threads: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            watch: true,
            max_entries: DEFAULT_MAX_ENTRIES,
            threads: 0,
        }
    }
}

/// Per-root state: the store (tools/browse) and the fuzzy engine (picker).
struct RootState {
    store: RwLock<IndexStore>,
    fuzzy: Mutex<FuzzyEngine>,
}

/// State shared between the public handle and the scanner thread.
struct Shared {
    roots: Vec<PathBuf>,
    states: Vec<RootState>,
    scanned: AtomicUsize,
    state: AtomicU8,
    truncated: AtomicBool,
    cancel: AtomicBool,
    cmd_tx: Sender<Command>,
    threads: usize,
    max_entries: usize,
    watch: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}
fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

/// Resident workspace file index. Clone-handle via `Arc`.
///
/// The background scanner thread is owned by this handle: dropping the last
/// handle cancels the walk and joins the thread (bounded — the walk checks
/// the cancel flag per entry and the command loop wakes every 250 ms).
pub struct WorkspaceIndex {
    shared: Arc<Shared>,
    scanner: Option<JoinHandle<()>>,
}

impl WorkspaceIndex {
    /// Start scanning `roots` in the background with default options.
    pub fn start(roots: Vec<PathBuf>) -> Arc<Self> {
        Self::start_with(roots, IndexOptions::default())
    }

    /// Like [`start`](Self::start) with explicit options.
    ///
    /// Roots are canonicalized; missing/non-dir roots and roots nested inside
    /// an earlier root are dropped (they would duplicate entries).
    pub fn start_with(roots: Vec<PathBuf>, opts: IndexOptions) -> Arc<Self> {
        let roots = sanitize_roots(roots);
        let states = (0..roots.len())
            .map(|_| RootState {
                store: RwLock::new(IndexStore::new()),
                fuzzy: Mutex::new(FuzzyEngine::new()),
            })
            .collect();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let threads = if opts.threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(1)
        } else {
            opts.threads
        };
        let shared = Arc::new(Shared {
            roots,
            states,
            scanned: AtomicUsize::new(0),
            state: AtomicU8::new(STATE_SCANNING),
            truncated: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            cmd_tx,
            threads,
            max_entries: opts.max_entries,
            watch: opts.watch,
        });
        let thread_shared = shared.clone();
        let scanner = std::thread::Builder::new()
            .name("whycode-index".into())
            .spawn(move || scanner_main(thread_shared, cmd_rx))
            .map_err(|e| tracing::warn!(error = %e, "index scanner thread failed to spawn"))
            .ok();
        Arc::new(Self { shared, scanner })
    }

    /// Roots of the index: `[working_dir]` plus every directory listed in
    /// `.whycode/external_dirs_allowed` (blank lines / `#` comments skipped).
    pub fn project_roots(working_dir: &Path) -> Vec<PathBuf> {
        let mut roots = vec![working_dir.to_path_buf()];
        let allow = working_dir.join(".whycode").join("external_dirs_allowed");
        if let Ok(content) = std::fs::read_to_string(&allow) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                roots.push(PathBuf::from(line));
            }
        }
        roots
    }

    /// The canonicalized roots being indexed.
    pub fn roots(&self) -> &[PathBuf] {
        &self.shared.roots
    }

    /// The primary root (working directory).
    pub fn primary_root(&self) -> &Path {
        &self.shared.roots[0]
    }

    /// Current scan progress.
    pub fn status(&self) -> ScanStatus {
        if self.shared.state.load(Ordering::Acquire) == STATE_READY {
            ScanStatus::Ready {
                total: self.len(),
                truncated: self.shared.truncated.load(Ordering::Relaxed),
            }
        } else {
            ScanStatus::Scanning {
                scanned: self.shared.scanned.load(Ordering::Relaxed),
            }
        }
    }

    /// True once the initial walk has finished.
    pub fn is_ready(&self) -> bool {
        self.shared.state.load(Ordering::Acquire) == STATE_READY
    }

    /// Total indexed entries across roots.
    pub fn len(&self) -> usize {
        self.shared
            .states
            .iter()
            .map(|s| read(&s.store).len())
            .sum()
    }

    /// True when no entries are indexed (yet).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Block until the index is ready or `timeout` elapses. For tests and
    /// one-shot CLI paths; the TUI should poll [`status`](Self::status).
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.is_ready() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.is_ready()
    }


    /// Fuzzy query across all roots; merged best-first, capped at `limit`.
    ///
    /// Never touches the filesystem. While the initial scan runs, partial
    /// results stream back (entries injected so far are already matchable).
    ///
    /// Special forms: an empty pattern browses the top level of the primary
    /// root; a pattern ending in `/` browses that directory.
    pub fn query(&self, pattern: &str, limit: usize) -> Vec<FileMatch> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return self.browse(0, "");
        }
        if let Some(dir) = pattern.strip_suffix('/') {
            return self.browse(0, dir);
        }
        let mut all = Vec::new();
        for (i, state) in self.shared.states.iter().enumerate() {
            let mut hits = lock(&state.fuzzy).query(pattern, limit);
            for h in &mut hits {
                h.root = i as u16;
            }
            all.extend(hits);
        }
        all.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.rel.len().cmp(&b.rel.len()))
        });
        all.truncate(limit);
        all
    }

    /// Depth-1 listing of `rel_dir` inside `root` (dirs first, alpha).
    /// `""` lists the top level. Filesystem-free — reads the store.
    pub fn browse(&self, root: u16, rel_dir: &str) -> Vec<FileMatch> {
        let Some(state) = self.shared.states.get(root as usize) else {
            return Vec::new();
        };
        read(&state.store)
            .browse(root, rel_dir)
            .into_iter()
            .map(|e| FileMatch {
                rel: e.rel.to_string(),
                is_dir: e.is_dir,
                score: 0,
                indices: Vec::new(),
                root,
            })
            .collect()
    }

    /// Visit every primary-root entry without cloning (tools hot path).
    pub fn visit(&self, mut f: impl FnMut(&Entry)) {
        if let Some(state) = self.shared.states.first() {
            for e in read(&state.store).entries() {
                f(e);
            }
        }
    }

    /// Clone of all primary-root entries (tools that need owned data).
    pub fn entries(&self) -> Vec<Entry> {
        self.shared
            .states
            .first()
            .map(|s| read(&s.store).entries().to_vec())
            .unwrap_or_default()
    }

    /// Absolute path for a match.
    pub fn resolve(&self, m: &FileMatch) -> PathBuf {
        self.shared.roots[m.root as usize].join(&m.rel)
    }

    /// Trigger a full background rescan (e.g. after branch switches).
    pub fn rescan(&self) {
        let _ = self.shared.cmd_tx.send(Command::Rescan);
    }
}

impl Drop for WorkspaceIndex {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        let _ = self.shared.cmd_tx.send(Command::Shutdown);
        if let Some(h) = self.scanner.take() {
            let _ = h.join();
        }
    }
}

impl std::fmt::Debug for WorkspaceIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceIndex")
            .field("roots", &self.shared.roots)
            .field("status", &self.status())
            .finish()
    }
}

/// Canonicalize roots, drop missing/non-dirs, dedup exact and nested roots.
fn sanitize_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(canon) = std::fs::canonicalize(&root) else {
            continue;
        };
        if !canon.is_dir() {
            continue;
        }
        // A root equal to or nested inside an earlier one duplicates entries.
        if out.iter().any(|r| canon.starts_with(r)) {
            continue;
        }
        out.push(canon);
    }
    out
}


/// Scanner thread body: initial scan, then watcher-fed delta loop.
fn scanner_main(shared: Arc<Shared>, cmd_rx: Receiver<Command>) {
    full_scan(&shared);

    let _watcher = if shared.watch {
        watch::spawn(&shared.roots, shared.cmd_tx.clone())
    } else {
        None
    };

    let mut pending: Vec<Change> = Vec::new();
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        match cmd_rx.recv_timeout(WATCH_DEBOUNCE) {
            Ok(Command::Shutdown) => break,
            Ok(Command::Rescan) => {
                pending.clear();
                full_scan(&shared);
            }
            Ok(Command::Batch(cs)) => pending.extend(cs),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !pending.is_empty() {
            apply_changes(&shared, std::mem::take(&mut pending));
        }
    }
}

/// Full (re)scan of every root: streams entries into the fuzzy engines
/// (queries stay live during the scan) and fills the stores.
fn full_scan(shared: &Arc<Shared>) {
    shared.scanned.store(0, Ordering::Relaxed);
    shared.truncated.store(false, Ordering::Relaxed);
    shared.state.store(STATE_SCANNING, Ordering::Release);

    for (i, state) in shared.states.iter().enumerate() {
        if shared.cancel.load(Ordering::Relaxed) {
            return;
        }
        write(&state.store).clear();
        let injector = {
            let mut fz = lock(&state.fuzzy);
            fz.restart();
            fz.injector()
        };
        let remaining = shared
            .max_entries
            .saturating_sub(shared.scanned.load(Ordering::Relaxed));
        if remaining == 0 {
            shared.truncated.store(true, Ordering::Relaxed);
            break;
        }
        // Walk threads push straight into nucleo (streaming matches) and
        // queue entries for a bulk store insert afterwards.
        let queue = Mutex::new(Vec::new());
        let stats = walk::walk_root(
            &shared.roots[i],
            shared.threads,
            remaining,
            &shared.scanned,
            &shared.cancel,
            &|e: WalkEntry| {
                FuzzyEngine::push(&injector, &e.rel, e.is_dir);
                lock(&queue).push(e);
            },
        );
        let mut store = write(&state.store);
        for e in lock(&queue).drain(..) {
            store.insert(i as u16, e.rel, e.is_dir, e.size);
        }
        drop(store);
        if stats.truncated {
            shared.truncated.store(true, Ordering::Relaxed);
            break;
        }
    }

    shared.state.store(STATE_READY, Ordering::Release);
}

/// Apply one debounced watcher batch: upserts push into nucleo; removals
/// rebuild that root's engine from the store (nucleo has no item removal).
fn apply_changes(shared: &Arc<Shared>, changes: Vec<Change>) {
    // Group by root so each engine is touched at most once per batch.
    let mut by_root: rustc_hash::FxHashMap<u16, Vec<Change>> = Default::default();
    for c in changes {
        by_root.entry(c.root).or_default().push(c);
    }
    for (root_idx, cs) in by_root {
        let Some(state) = shared.states.get(root_idx as usize) else {
            continue;
        };
        let root = &shared.roots[root_idx as usize];
        let mut saw_removal = false;
        let mut upserts: Vec<(Box<str>, bool)> = Vec::new();
        {
            let mut store = write(&state.store);
            for c in cs {
                match c.kind {
                    ChangeKind::Remove => {
                        store.remove_tree(root_idx, &c.rel);
                        saw_removal = true;
                    }
                    ChangeKind::Upsert => {
                        // Re-stat; a create+delete inside one debounce window
                        // collapses to a remove.
                        match std::fs::symlink_metadata(root.join(&c.rel)) {
                            Ok(md) => {
                                let is_dir = md.is_dir();
                                let size = if is_dir { 0 } else { md.len() };
                                upserts.push((c.rel.clone().into_boxed_str(), is_dir));
                                store.insert(root_idx, c.rel.into_boxed_str(), is_dir, size);
                            }
                            Err(_) => {
                                store.remove_tree(root_idx, &c.rel);
                                saw_removal = true;
                            }
                        }
                    }
                }
            }
        }
        // Lock order is always fuzzy → store (never the reverse).
        let mut fz = lock(&state.fuzzy);
        if saw_removal {
            fz.restart();
            let inj = fz.injector();
            for e in read(&state.store).iter_root(root_idx) {
                FuzzyEngine::push(&inj, &e.rel, e.is_dir);
            }
        } else {
            let inj = fz.injector();
            for (rel, is_dir) in upserts {
                FuzzyEngine::push(&inj, &rel, is_dir);
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/nested/deep.rs"), "// deep").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/x.o"), "bin").unwrap();
        fs::write(root.join(".env"), "SECRET=1").unwrap();
        dir
    }

    #[test]
    fn end_to_end_scan_query_browse() {
        let dir = fixture();
        let idx = WorkspaceIndex::start_with(
            vec![dir.path().to_path_buf()],
            IndexOptions {
                watch: false,
                ..Default::default()
            },
        );
        assert!(idx.wait_ready(Duration::from_secs(10)));
        match idx.status() {
            ScanStatus::Ready { total, truncated } => {
                assert!(total >= 5, "total={total}");
                assert!(!truncated);
            }
            other => panic!("expected Ready, got {other:?}"),
        }

        // Fuzzy.
        let hits = idx.query("main.rs", 10);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].rel, "src/main.rs");
        assert_eq!(hits[0].root, 0);

        // Pruned entries never made it in.
        assert!(idx.query("x.o", 10).is_empty());
        assert!(idx.query(".env", 10).is_empty());

        // Browse: empty query → top level, dirs first.
        let top = idx.query("", 20);
        assert!(top.iter().any(|m| m.rel == "src" && m.is_dir));
        assert!(top.iter().any(|m| m.rel == "Cargo.toml" && !m.is_dir));
        assert!(top[0].is_dir, "dirs first: {top:?}");

        // Browse subdir via trailing slash.
        let src = idx.query("src/", 20);
        assert!(src.iter().any(|m| m.rel == "src/main.rs"));
        assert!(src.iter().all(|m| m.rel.starts_with("src/")));

        // Tools view.
        assert!(idx.entries().iter().any(|e| &*e.rel == "src/main.rs"));
        let mut seen = 0;
        idx.visit(|_| seen += 1);
        assert!(seen >= 5);

        // Resolve.
        let m = &hits[0];
        assert!(idx.resolve(m).ends_with("src/main.rs"));
    }

    #[test]
    fn watcher_picks_up_changes() {
        let dir = fixture();
        let idx = WorkspaceIndex::start(vec![dir.path().to_path_buf()]);
        assert!(idx.wait_ready(Duration::from_secs(10)));
        let before = idx.len();

        // Create → appears.
        fs::write(dir.path().join("src/new_file.rs"), "// new").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while idx.len() < before + 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(idx.len(), before + 1, "create must be indexed");
        assert!(idx.query("new_file", 10).iter().any(|m| m.rel == "src/new_file.rs"));

        // Delete → disappears from the store (fuzzy engine rebuilds).
        fs::remove_file(dir.path().join("src/new_file.rs")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let gone = {
                let mut found = false;
                idx.visit(|e| found |= &*e.rel == "src/new_file.rs");
                !found
            };
            if gone || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut found = false;
        idx.visit(|e| found |= &*e.rel == "src/new_file.rs");
        assert!(!found, "delete must be removed from store");
    }

    #[test]
    fn sanitize_roots_dedups_nested() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let roots = sanitize_roots(vec![
            root.clone(),
            root.join("src"),          // nested → dropped
            root.join("missing"),      // nonexistent → dropped
            root.clone(),              // dup → dropped
        ]);
        assert_eq!(roots, vec![root]);
    }

    #[test]
    fn project_roots_reads_allowlist() {
        let dir = fixture();
        let ext = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".whycode")).unwrap();
        fs::write(
            dir.path().join(".whycode/external_dirs_allowed"),
            format!("# comment\n{}\n\n", ext.path().display()),
        )
        .unwrap();
        let roots = WorkspaceIndex::project_roots(dir.path());
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0], dir.path());
        assert_eq!(roots[1], ext.path());
    }

    #[test]
    fn empty_roots_is_safe() {
        let idx = WorkspaceIndex::start_with(vec![], IndexOptions::default());
        assert!(idx.roots().is_empty());
        assert!(idx.query("x", 5).is_empty());
    }
}

