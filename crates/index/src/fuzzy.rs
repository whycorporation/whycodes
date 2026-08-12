//! Fuzzy matching over the index: a `nucleo` engine fed by streaming
//! injection, so keystroke queries never touch the filesystem and matches
//! appear while the initial walk is still running.
//!
//! Query handling mirrors the proven helix / Grok Build patterns:
//! path-aware scoring (`Config::match_paths`), smart case, incremental
//! reparse when the query grows by one char, and a length-scaled minimum
//! score to keep junk out of short queries.

use std::sync::Arc;

use nucleo::pattern::{CaseMatching, MultiPattern, Normalization, Pattern};
use nucleo::{Config, Injector, Matcher, Nucleo};

/// Background matcher threads. Two is plenty for path matching; more would
/// only fight the UI thread.
const MATCHER_THREADS: usize = 2;

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

/// Streaming fuzzy engine over path strings.
pub struct FuzzyEngine {
    nucleo: Nucleo<EntryMeta>,
    /// Reused for highlight-index computation (holds internal scratch space).
    matcher: Matcher,
    /// Last parsed query (drives the incremental-reparse fast path).
    query: String,
}

impl FuzzyEngine {
    pub fn new() -> Self {
        let config = Config::DEFAULT.match_paths();
        let mut nucleo = Nucleo::new(config.clone(), Arc::new(|| {}), Some(MATCHER_THREADS), 1);
        nucleo.pattern = MultiPattern::new(1);
        Self {
            nucleo,
            matcher: Matcher::new(config),
            query: String::new(),
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

    /// Run a query and return up to `limit` best matches, best first.
    pub fn query(&mut self, pattern: &str, limit: usize) -> Vec<FileMatch> {
        if pattern.is_empty() {
            return Vec::new(); // browse mode is served by the store, not fuzzy
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
        // Bounded wait: worker threads usually finish in well under this.
        self.nucleo.tick(10);

        // Split borrows: snapshot borrows `nucleo`, indices need `matcher`.
        let Self {
            nucleo, matcher, ..
        } = self;
        let snapshot = nucleo.snapshot();
        let col_pattern = Pattern::parse(pattern, CaseMatching::Smart, Normalization::Smart);
        let floor = min_score(pattern);

        // `matched_items` yields best-first; scores are recomputed per item
        // (cheap at picker sizes) so the length-scaled floor can drop junk.
        let total = snapshot.matched_item_count();
        let mut out = Vec::with_capacity(limit.min(256));
        for item in snapshot.matched_items(0..total.min(limit as u32)) {
            let text = &item.matcher_columns[0];
            let mut indices = Vec::new();
            let Some(score) = col_pattern.indices(text.slice(..), matcher, &mut indices) else {
                continue;
            };
            if score < floor {
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
        out
    }
}

impl Default for FuzzyEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> FuzzyEngine {
        let e = FuzzyEngine::new();
        let inj = e.injector();
        FuzzyEngine::push(&inj, "src/main.rs", false);
        FuzzyEngine::push(&inj, "src/lib.rs", false);
        FuzzyEngine::push(&inj, "crates/tui/src/app.rs", false);
        FuzzyEngine::push(&inj, "docs", true);
        FuzzyEngine::push(&inj, "README.md", false);
        drop(inj);
        e
    }

    #[test]
    fn query_finds_paths_with_indices() {
        let mut e = engine();
        let hits = e.query("main.rs", 10);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].rel, "src/main.rs");
        assert!(!hits[0].is_dir);
        assert!(!hits[0].indices.is_empty());
        assert!(hits[0].score >= min_score("main.rs"));
    }

    #[test]
    fn query_matches_subsequence_across_dirs() {
        let mut e = engine();
        let hits = e.query("tuiapp", 10);
        assert!(
            hits.iter().any(|h| h.rel == "crates/tui/src/app.rs"),
            "{hits:?}"
        );
    }

    #[test]
    fn short_query_drops_weak_tail() {
        let mut e = engine();
        let hits = e.query("zzzzz", 10);
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn dirs_strip_trailing_slash() {
        let mut e = engine();
        let hits = e.query("docs", 10);
        assert!(hits.iter().any(|h| h.rel == "docs" && h.is_dir));
    }

    #[test]
    fn restart_clears_items() {
        let mut e = engine();
        assert!(!e.query("main", 10).is_empty());
        e.restart();
        assert!(e.query("main", 10).is_empty());
    }

    #[test]
    fn empty_query_is_browse_not_fuzzy() {
        let mut e = engine();
        assert!(e.query("", 10).is_empty());
    }
}
