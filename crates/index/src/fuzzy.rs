//! Fuzzy matching over the index: a `nucleo` engine fed by streaming
//! injection, so keystroke queries never touch the filesystem and matches
//! appear while the initial walk is still running.
//!
//! Query handling mirrors the proven helix / Grok Build patterns:
//! path-aware scoring (`Config::match_paths`), smart case, incremental
//! reparse when the query grows by one char, and a length-scaled minimum
//! score to keep junk out of short queries.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nucleo::pattern::{CaseMatching, MultiPattern, Normalization, Pattern};
use nucleo::{Config, Injector, Matcher, Nucleo};

/// Matcher threads scale with the host: matching cost grows with item count,
/// and a TUI frame budget is 16 ms. Two threads starve at 35k+ items; four
/// keeps full rematches in the low single-digit ms there.
fn matcher_threads() -> usize {
    matcher_thread_count(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2),
    )
}

pub(crate) fn matcher_thread_count(n: usize) -> usize {
    match n {
        0..=4 => 2,
        5..=8 => 3,
        _ => 4,
    }
}

/// Per-item payload stored alongside the matched column.
#[derive(Debug, Clone, Copy)]
pub struct EntryMeta {
    pub is_dir: bool,
}

/// One fuzzy match returned to the UI.
#[derive(Debug, Clone, Default)]
pub struct FileMatch {
    /// Root-relative path (no trailing slash for dirs).
    pub rel: String,
    pub is_dir: bool,
    /// Matcher score; higher is better. 0 in browse mode.
    pub score: u32,
    /// Matched character positions in `rel` (for highlighting).
    pub indices: Vec<u32>,
    /// Which root the match came from (index into `WorkspaceIndex::roots`).
    pub root: u16,
}

/// Length-scaled floor that keeps weak tail matches out of the picker
/// (helix completion uses `7 * len + 14`; Grok Build uses `7 + len * 14`).
fn min_score(query: &str) -> u32 {
    7 + query.chars().count() as u32 * 14
}

/// Bound for `nucleo::tick` when we must wait for a snapshot. `5` was enough
/// isolated; llvm-cov / parallel CI needs the same wait as a blocking query.
const SETTLE_MS: u64 = 50;

pub(crate) fn below_floor(score: u32, floor: u32) -> bool {
    score < floor
}

/// Streaming fuzzy engine over path strings.
pub struct FuzzyEngine {
    nucleo: Nucleo<EntryMeta>,
    /// Reused for highlight-index computation (holds internal scratch space).
    matcher: Matcher,
    /// Last parsed query (drives the incremental-reparse fast path).
    query: String,
    /// True while the matcher workers are busy (from the last `tick(0)`).
    running: bool,
}

impl FuzzyEngine {
    /// `results_dirty` is shared with the index so one flag wakes the UI;
    /// nucleo's notify callback flips it whenever workers publish results.
    pub fn new(results_dirty: Arc<AtomicBool>) -> Self {
        let config = Config::DEFAULT.match_paths();
        let notify = move || results_dirty.store(true, Ordering::Release);
        let mut nucleo = Nucleo::new(config.clone(), Arc::new(notify), Some(matcher_threads()), 1);
        nucleo.pattern = MultiPattern::new(1);
        Self {
            nucleo,
            matcher: Matcher::new(config),
            query: String::new(),
            running: false,
        }
    }

    /// A clonable handle for walk threads to stream entries into the engine.
    pub fn injector(&self) -> Injector<EntryMeta> {
        self.nucleo.injector()
    }

    /// Push one entry through an injector (thread-safe, callable anywhere).
    pub fn push(injector: &Injector<EntryMeta>, rel: &str, is_dir: bool) {
        // Dirs carry a trailing `/` in the matched column: it reads naturally
        // in the picker and makes `src/` style queries work. Stripped on the
        // way back out via `EntryMeta::is_dir`.
        let text = if is_dir {
            format!("{rel}/")
        } else {
            rel.to_owned()
        };
        injector.push(EntryMeta { is_dir }, move |_, cols| {
            cols[0] = text.into();
        });
    }

    /// Drop all items, keeping allocations. Used before bulk re-injection
    /// after watcher removals (nucleo has no per-item removal).
    pub fn restart(&mut self) {
        self.nucleo.restart(true);
        self.query.clear();
    }

    /// Nudge the matcher without waiting; returns true while work (matching
    /// or walk ingestion) is still in flight.
    pub fn nudge(&mut self) -> bool {
        let status = self.nucleo.tick(0);
        self.running = status.running || self.nucleo.active_injectors() > 0;
        self.running
    }

    /// Re-parse the current query and wait for a snapshot.
    ///
    /// `set_query` does not reparse when the pattern is unchanged. Under load
    /// a `tick(0)` can miss nucleo's notify (workers finish with
    /// `should_notify == false`), so the picker sits on an empty snapshot
    /// until the next keystroke. The idle-empty poll path calls this.
    ///
    /// Wait matches [`query_blocking`]: `tick(5)` times out under llvm-cov /
    /// parallel CI and leaves `picker_flow_over_real_index` on
    /// `Ready { total: 4 }` with `matches=[]`.
    pub fn rearm(&mut self) {
        if self.query.is_empty() {
            return;
        }
        let pattern = self.query.clone();
        self.nucleo.pattern.reparse(
            0,
            &pattern,
            CaseMatching::Smart,
            Normalization::Smart,
            false,
        );
        let status = self.nucleo.tick(SETTLE_MS);
        self.running = status.running || self.nucleo.active_injectors() > 0;
    }

    /// Non-blocking query update: reparse and `tick(0)`. Never wait here —
    /// a keystroke must not block on a full rematch (3–16 ms at 35k items).
    pub fn set_query(&mut self, pattern: &str) {
        if pattern == self.query {
            // Same pattern is not a full no-op: a missed notify still needs
            // a tick so the next poll can adopt an already-finished snapshot.
            let status = self.nucleo.tick(0);
            self.running = status.running || self.nucleo.active_injectors() > 0;
            return;
        }
        let append = pattern.as_bytes().starts_with(self.query.as_bytes())
            && !pattern.ends_with('\\')
            && !pattern
                .as_bytes()
                .last()
                .is_some_and(|ch| ch.is_ascii_whitespace());
        self.nucleo.pattern.reparse(
            0,
            pattern,
            CaseMatching::Smart,
            Normalization::Smart,
            append,
        );
        self.query.clear();
        self.query.push_str(pattern);
        self.nucleo.tick(0);
    }

    /// Read the matches currently in the snapshot (best-first, capped at
    /// `limit`). Never blocks longer than a `tick(0)`; the second return
    /// value is true while the matcher workers are still refining results.
    pub fn read(&mut self, limit: usize) -> (Vec<FileMatch>, bool) {
        let pattern = self.query.clone();
        if pattern.is_empty() {
            let status = self.nucleo.tick(0);
            self.running = status.running;
            return (Vec::new(), self.running); // browse mode is store-backed
        }
        let status = self.nucleo.tick(0);
        self.running = status.running;

        // Split borrows: snapshot borrows `nucleo`, indices need `matcher`.
        let Self {
            nucleo, matcher, ..
        } = self;
        let snapshot = nucleo.snapshot();
        let col_pattern = Pattern::parse(&pattern, CaseMatching::Smart, Normalization::Smart);
        let floor = min_score(&pattern);

        // `matched_items` yields best-first; scores are recomputed per item
        // (cheap at picker sizes) so the length-scaled floor can drop junk.
        let total = snapshot.matched_item_count();
        let mut out = Vec::with_capacity(limit.min(256));
        for item in snapshot.matched_items(0..total.min(limit as u32)) {
            let text = &item.matcher_columns[0];
            let mut indices = Vec::new();
            let score = col_pattern
                .indices(text.slice(..), matcher, &mut indices)
                .unwrap_or(0);
            if below_floor(score, floor) {
                continue;
            }
            let mut rel = text.to_string();
            let is_dir = item.data.is_dir;
            if is_dir && rel.ends_with('/') {
                rel.pop();
            }
            out.push(FileMatch {
                rel,
                is_dir,
                score,
                indices,
                root: 0, // filled in by the caller (per-root engine)
            });
        }
        (out, self.running)
    }

    /// Blocking convenience for tests and one-shot tools: set the query and
    /// wait (bounded) for the matcher to settle, then read.
    pub fn query_blocking(&mut self, pattern: &str, limit: usize) -> Vec<FileMatch> {
        self.set_query(pattern);
        self.nucleo.tick(SETTLE_MS);
        self.read(limit).0
    }
}

impl Default for FuzzyEngine {
    fn default() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)))
    }
}

#[cfg(test)]
#[path = "fuzzy_tests.rs"]
mod tests;
