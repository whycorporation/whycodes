//! Unified tree-pruning policy for project scans.
//!
//! Single source of truth for which parts of a tree whycodes descends into —
//! used by the workspace index, the file tools (`glob` / `list` / `grep`),
//! and memory code indexing. Replaces the drifted per-crate copies that lived
//! in `tools/file/paths.rs` and `memory/code_index.rs`. On top of this name
//! policy the walker also honours `.gitignore` / `.ignore` files (via the
//! `ignore` crate, the same engine ripgrep uses), so build output that a
//! project ignores never reaches the index even if it is not listed here.

/// Directory names never descended into (VCS, build output, caches, vendored deps).
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "target",
    "node_modules",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    ".gradle",
    ".zig-cache",
    ".dart_tool",
    "coverage",
    ".idea",
    ".vscode",
    // Local cargo registry/config dirs are almost never what the model needs.
    ".cargo",
    "Pods",
];

/// Dot-directories still indexed despite the hidden-skip rule (project metadata).
pub const HIDDEN_DIR_WHITELIST: &[&str] = &[".github", ".whycodes", ".config"];

/// Well-known dotfiles kept in the index; every other `.`-prefixed file is
/// skipped (secret hygiene — `.env` and friends must not flow into prompts).
pub const HIDDEN_FILE_WHITELIST: &[&str] = &[
    ".gitignore",
    ".gitattributes",
    ".gitmodules",
    ".editorconfig",
    ".dockerignore",
    ".env.example",
    ".env.sample",
    ".rustfmt.toml",
    ".clippy.toml",
];

/// True when a directory entry must not be descended into or listed.
pub fn is_pruned_dir(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return true;
    }
    if SKIP_DIRS.contains(&name) {
        return true;
    }
    if name.starts_with('.') {
        return !HIDDEN_DIR_WHITELIST.contains(&name);
    }
    false
}

/// True when a file entry is excluded from the index.
pub fn is_pruned_file(name: &str) -> bool {
    if name.starts_with('.') {
        return !HIDDEN_FILE_WHITELIST.contains(&name);
    }
    false
}

/// True when a `/`-separated relative path passes the policy — i.e. no
/// component is a pruned directory and the file name itself is allowed.
/// Used to filter watcher events (where the entry kind may be unknown).
pub fn rel_path_allowed(rel: &str) -> bool {
    let mut components = rel.split('/').filter(|c| !c.is_empty()).peekable();
    while let Some(comp) = components.next() {
        let last = components.peek().is_none();
        if last {
            // Could be a file or a dir — reject only if both policies prune it.
            if is_pruned_dir(comp) && is_pruned_file(comp) {
                return false;
            }
        } else if is_pruned_dir(comp) {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
