//! Ignore rules for watcher deltas.
//!
//! The full scan skips everything the project ignores because the walker
//! reads `.ignore`, `.gitignore`, `.git/info/exclude` and the global git
//! ignore file. Watcher events carry bare paths, so without this check every
//! file created later under an ignored directory (build output, the
//! `.whycodes/scratch/` logs of shell commands) entered the index and stayed
//! there until the next rescan. [`IgnoreRules`] answers the same question for
//! one path, reading the rule files of its directories on first use.
//!
//! Precedence follows the walker: a deeper directory beats a shallower one,
//! `.ignore` beats `.gitignore` in the same directory, and the repository
//! exclude file and the global file come last. A path is ignored when it or
//! any directory above it is. Rule files above the root are not read.

use std::path::Path;

use ignore::Match;
use ignore::gitignore::Gitignore;
use rustc_hash::FxHashMap;

/// Per-directory rule files, highest precedence first.
const RULE_FILES: [&str; 2] = [".ignore", ".gitignore"];

/// Directories whose rule files stay cached. Past this the cache starts over.
pub(crate) const MAX_RULE_DIRS: usize = 4096;

/// True for a file name whose edit changes what the index should hold.
pub(crate) fn is_rules_file(name: &str) -> bool {
    RULE_FILES.contains(&name)
}

/// Lazily loaded ignore matchers of one index root.
#[derive(Default)]
pub(crate) struct IgnoreRules {
    /// `.git/info/exclude`, then the global ignore file.
    repo: Option<Vec<Gitignore>>,
    /// Root-relative directory (`""` for the root) → its rule files.
    dirs: FxHashMap<String, Vec<Gitignore>>,
}

impl IgnoreRules {
    /// Forget every loaded matcher; the next query reads the files again.
    pub(crate) fn clear(&mut self) {
        self.repo = None;
        self.dirs.clear();
    }

    /// True when `rel` (root-relative, `/`-separated) is ignored.
    pub(crate) fn is_ignored(&mut self, root: &Path, rel: &str, is_dir: bool) -> bool {
        let mut start = 0;
        loop {
            let slash = rel[start..].find('/').map(|i| start + i);
            let (sub, sub_is_dir) = match slash {
                Some(i) => (&rel[..i], true),
                None => (rel, is_dir),
            };
            if self.matched(root, sub, sub_is_dir) {
                return true;
            }
            match slash {
                Some(i) => start = i + 1,
                None => return false,
            }
        }
    }

    /// Verdict for `sub` alone, from its own directory up to the root.
    fn matched(&mut self, root: &Path, sub: &str, is_dir: bool) -> bool {
        let path = root.join(sub);
        let mut dir = parent(sub);
        loop {
            if let Some(ignored) = verdict(self.dir_rules(root, dir), &path, is_dir) {
                return ignored;
            }
            if dir.is_empty() {
                break;
            }
            dir = parent(dir);
        }
        let repo = self.repo.get_or_insert_with(|| repo_rules(root));
        for gi in repo.iter() {
            if let Some(ignored) = verdict(std::slice::from_ref(gi), &gi.path().join(sub), is_dir) {
                return ignored;
            }
        }
        false
    }

    fn dir_rules(&mut self, root: &Path, dir: &str) -> &[Gitignore] {
        if !self.dirs.contains_key(dir) && self.dirs.len() >= MAX_RULE_DIRS {
            self.dirs.clear();
        }
        self.dirs
            .entry(dir.to_string())
            .or_insert_with(|| load_dir(&root.join(dir)))
    }
}

/// `Some(true)` ignored, `Some(false)` re-included, `None` no rule matched.
fn verdict(rules: &[Gitignore], path: &Path, is_dir: bool) -> Option<bool> {
    rules.iter().find_map(|gi| match gi.matched(path, is_dir) {
        Match::Ignore(_) => Some(true),
        Match::Whitelist(_) => Some(false),
        Match::None => None,
    })
}

/// `a/b/c` → `a/b`; a top-level name → `""`.
fn parent(rel: &str) -> &str {
    rel.rfind('/').map_or("", |i| &rel[..i])
}

fn load_dir(dir: &Path) -> Vec<Gitignore> {
    RULE_FILES
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .map(|path| load(&path))
        .collect()
}

fn repo_rules(root: &Path) -> Vec<Gitignore> {
    let exclude = root.join(".git/info/exclude");
    let mut out = Vec::new();
    if exclude.is_file() {
        out.push(load(&exclude));
    }
    let (global, err) = Gitignore::global();
    log_partial(&global.path().join("<global gitignore>"), err);
    out.push(global);
    out
}

/// A matcher rooted at the file's directory. Bad lines are logged and
/// skipped; the rest still apply, as in the walker.
fn load(path: &Path) -> Gitignore {
    let (gi, err) = Gitignore::new(path);
    log_partial(path, err);
    gi
}

fn log_partial(path: &Path, err: Option<ignore::Error>) {
    if let Some(e) = err {
        tracing::debug!(path = %path.display(), error = %e, "ignore rules partly unread");
    }
}

#[cfg(test)]
#[path = "ignore_rules_tests.rs"]
mod tests;
