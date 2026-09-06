//! Shared path resolution and directory walking for file tools.
//!
//! Keeps `read` / `list` / `glob` / `grep` consistent and avoids re-walking
//! heavy trees (`target/`, `node_modules/`, cargo registry, …).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Directories pruned during recursive walks (grep/glob/list recursive).
/// Single source of truth lives in `whycodes_index::policy`; re-exported here
/// so existing call sites keep working.
pub const SKIP_DIRS: &[&str] = whycodes_index::policy::SKIP_DIRS;

/// Bytes sniffed for a NUL (binary) marker.
pub const BINARY_SNIFF_LEN: usize = 8192;

/// Soft cap for full-file materialization in tools (bytes).
pub const MAX_FULL_READ_BYTES: u64 = 8 * 1024 * 1024;

/// Soft cap for a single file grepped fully (bytes).
pub const MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Resolve a user path against the tool working directory.
///
/// Empty / `.` → working dir. Relative paths join `working_dir`. Absolute
/// paths are used as-is. Does not require the path to exist.
pub fn resolve_path(working_dir: &str, path: &str) -> PathBuf {
    let p = path.trim();
    if p.is_empty() || p == "." {
        return PathBuf::from(working_dir);
    }
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(working_dir).join(path)
    }
}

/// Display path relative to `working_dir` when possible.
pub fn display_path(path: &Path, working_dir: &str) -> String {
    let base = Path::new(working_dir);
    path.strip_prefix(base)
        .map(|r| {
            let s = r.to_string_lossy();
            if s.is_empty() {
                ".".into()
            } else {
                s.into_owned()
            }
        })
        .unwrap_or_else(|_| path.display().to_string())
}

/// Whether a directory name should be pruned from recursive walks.
/// Delegates to the shared index policy (skip-list + hidden-dir rules).
pub fn is_skip_dir(name: &str) -> bool {
    whycodes_index::policy::is_pruned_dir(name)
}

/// Visit index entries under `root` without cloning the whole store.
///
/// `visit` receives `(abs, rel, is_dir, size)` and returns `false` to stop.
/// `None` means the index is cold / `root` is outside the primary root.
pub fn visit_index(
    index: &whycodes_index::WorkspaceIndex,
    root: &Path,
    visit: &mut dyn FnMut(&Path, &str, bool, u64) -> bool,
) -> Option<()> {
    if !index.is_ready() {
        return None;
    }
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let primary = index.primary_root();
    let rel_root = root.strip_prefix(primary).ok()?;
    let prefix = rel_root.to_string_lossy().replace('\\', "/");
    let prefix = prefix.trim_matches('/').to_string();
    let mut keep = true;
    index.visit(&mut |e| {
        if !keep {
            return false;
        }
        let in_scope = if prefix.is_empty() {
            true
        } else {
            e.rel.len() > prefix.len()
                && e.rel.starts_with(&prefix)
                && e.rel.as_bytes()[prefix.len()] == b'/'
        };
        if in_scope {
            let rel = if prefix.is_empty() {
                e.rel.to_string()
            } else {
                e.rel[prefix.len() + 1..].to_string()
            };
            let abs = primary.join(&*e.rel);
            if !visit(&abs, &rel, e.is_dir, e.size) {
                keep = false;
                return false;
            }
        }
        true
    });
    Some(())
}

/// Human-readable byte size (e.g. `12.4 KB`).
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// True if the first `BINARY_SNIFF_LEN` bytes contain a NUL.
pub fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_LEN).any(|b| *b == 0)
}

/// Sniff the start of a file for binary content without reading everything.
pub fn is_binary_file(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; BINARY_SNIFF_LEN];
    match f.read(&mut buf) {
        Ok(n) => is_binary_bytes(&buf[..n]),
        Err(_) => false,
    }
}

/// Simple `*` glob match against a single path segment or full relative path.
/// Supports `*` wildcards (not full brace expansion).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Fast paths
    if !pattern.contains('*') {
        return pattern == text;
    }
    match glob::Pattern::new(pattern) {
        Ok(p) => p.matches(text),
        Err(_) => pattern == text,
    }
}

/// Suggest similar names in a directory when a path is missing.
pub fn suggest_similar(missing: &Path, limit: usize) -> Vec<String> {
    let Some(parent) = missing.parent() else {
        return Vec::new();
    };
    let Some(want) = missing.file_name().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let want_l = want.to_ascii_lowercase();
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };

    let mut scored: Vec<(usize, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let name_l = name.to_ascii_lowercase();
            // Prefer prefix / substring matches
            let score = if name_l == want_l {
                0
            } else if name_l.starts_with(&want_l) || want_l.starts_with(&name_l) {
                1
            } else if name_l.contains(&want_l) || want_l.contains(&name_l) {
                2
            } else {
                // crude edit distance proxy: shared prefix length
                let common = name_l
                    .chars()
                    .zip(want_l.chars())
                    .take_while(|(a, b)| a == b)
                    .count();
                if common >= 2 {
                    10 - common.min(9)
                } else {
                    return None;
                }
            };
            Some((score, name))
        })
        .collect();

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(limit).map(|(_, n)| n).collect()
}

/// Directory entry for listing / walking.
#[derive(Debug, Clone)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Read one directory level (non-recursive). Sorted: dirs first, then files.
pub fn list_dir_entries(dir: &Path, ignore: &[String]) -> Result<Vec<DirEntryInfo>, String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("Failed to list {}: {}", dir.display(), e))?;

    let mut out = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }
        if ignore.iter().any(|pat| glob_match(pat, &name)) {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let size = if is_dir {
            None
        } else {
            entry.metadata().ok().map(|m| m.len())
        };
        out.push(DirEntryInfo {
            name,
            path,
            is_dir,
            size,
        });
    }

    out.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    Ok(out)
}

/// Callback for recursive file visits. Return `false` to stop the walk.
pub type VisitFn<'a> = dyn FnMut(&Path, &str /* relative path */) -> bool + 'a;

/// Callback for recursive directory+file visits. Return `false` to stop.
/// Args: (absolute path, root-relative `/` path, is_dir, size).
pub type VisitEntryFn<'a> = dyn FnMut(&Path, &str, bool, Option<u64>) -> bool + 'a;

/// Walk files under `root`, honouring `.gitignore` / `.ignore` (same engine
/// as the workspace index / ripgrep) and pruning `SKIP_DIRS` / hidden dirs.
///
/// Hidden *files* are still visited so an explicit glob/grep for `.env` works
/// on the cold path; the warm index continues to omit them (secret hygiene).
/// `relative` paths use `/` separators. Stops early when visitor returns false.
pub fn walk_files(root: &Path, visit: &mut VisitFn<'_>) {
    let _ = walk_entries(
        root,
        usize::MAX,
        usize::MAX,
        &mut |path, rel, is_dir, _size| {
            if is_dir { true } else { visit(path, rel) }
        },
    );
}

/// Gitignore-aware recursive walk of files *and* directories.
///
/// `max_depth` is ignore-crate depth (`1` = children of `root` only).
/// `max_entries` caps delivered entries (visitor is not called past the cap).
/// Returns `true` when the walk stopped early (cap or visitor returned false).
pub fn walk_entries(
    root: &Path,
    max_depth: usize,
    max_entries: usize,
    visit: &mut VisitEntryFn<'_>,
) -> bool {
    if root.is_file() {
        let rel = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let size = file_len(root);
        let _ = visit(root, &rel, false, size);
        return false;
    }
    if !root.is_dir() {
        return false;
    }

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
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
        .threads(1);

    builder.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if is_dir { !is_skip_dir(&name) } else { true }
    });

    let mut truncated = false;
    let mut delivered = 0usize;
    for entry in builder.build() {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        let is_dir = ft.is_dir();
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.display().to_string());
        let size = if is_dir {
            None
        } else {
            entry.metadata().ok().map(|m| m.len())
        };
        if delivered >= max_entries {
            truncated = true;
            break;
        }
        delivered += 1;
        if !visit(path, &rel, is_dir, size) {
            truncated = true;
            break;
        }
    }
    truncated
}

/// Seek-friendly check: file size via metadata.
pub fn file_len(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|m| m.len())
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
