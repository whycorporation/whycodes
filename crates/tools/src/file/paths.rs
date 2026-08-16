//! Shared path resolution and directory walking for file tools.
//!
//! Keeps `read` / `list` / `glob` / `grep` consistent and avoids re-walking
//! heavy trees (`target/`, `node_modules/`, cargo registry, …).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Directories pruned during recursive walks (grep/glob/list recursive).
/// Single source of truth lives in `whycode_index::policy`; re-exported here
/// so existing call sites keep working.
pub const SKIP_DIRS: &[&str] = whycode_index::policy::SKIP_DIRS;

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
    whycode_index::policy::is_pruned_dir(name)
}

/// Entries under `root` from the warm workspace index, shaped like
/// [`walk_files`] results: (absolute path, `root`-relative `/` path, is_dir).
///
/// Returns `None` when the index is absent, still scanning (cold), or `root`
/// is not inside the primary index root — callers then fall back to walking.
/// Note the index never lists hidden files (secret hygiene: `.env` & co. stay
/// out of agent context); patterns explicitly targeting dotfiles should walk.
pub fn index_entries(
    index: &whycode_index::WorkspaceIndex,
    root: &Path,
) -> Option<Vec<(PathBuf, String, bool, u64)>> {
    if !index.is_ready() {
        return None;
    }
    // The index canonicalizes roots; match that so symlinked working dirs
    // (e.g. /tmp on macOS) still hit the fast path.
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let primary = index.primary_root();
    let rel_root = root.strip_prefix(primary).ok()?;
    let prefix = rel_root.to_string_lossy().replace('\\', "/");
    let prefix = prefix.trim_matches('/').to_string();
    let mut out = Vec::new();
    index.visit(|e| {
        let in_scope = if prefix.is_empty() {
            true
        } else {
            e.rel.len() > prefix.len()
                && e.rel.starts_with(&prefix)
                && e.rel.as_bytes()[prefix.len()] == b'/'
        };
        if !in_scope {
            return;
        }
        let rel = if prefix.is_empty() {
            e.rel.to_string()
        } else {
            e.rel[prefix.len() + 1..].to_string()
        };
        out.push((primary.join(&*e.rel), rel, e.is_dir, e.size));
    });
    Some(out)
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

/// Walk files under `root`, pruning `SKIP_DIRS` / hidden dirs.
///
/// `relative` paths use `/` separators. Stops early when visitor returns false.
pub fn walk_files(root: &Path, visit: &mut VisitFn<'_>) {
    fn walk_inner(root: &Path, dir: &Path, visit: &mut VisitFn<'_>) -> bool {
        let Ok(rd) = fs::read_dir(dir) else {
            return true;
        };
        // Collect + sort for stable results across filesystems
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();

        for path in paths {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();

            let is_dir = path.is_dir();
            if is_dir {
                if is_skip_dir(&name) {
                    continue;
                }
                if !walk_inner(root, &path, visit) {
                    return false;
                }
            } else {
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.display().to_string());
                if !visit(&path, &rel) {
                    return false;
                }
            }
        }
        true
    }

    if root.is_file() {
        let rel = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let _ = visit(root, &rel);
        return;
    }
    walk_inner(root, root, visit);
}

/// Seek-friendly check: file size via metadata.
pub fn file_len(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_relative_and_absolute() {
        let abs = if cfg!(windows) { r"C:\tmp\x" } else { "/tmp/x" };
        assert_eq!(resolve_path("/proj", abs), PathBuf::from(abs));
        assert_eq!(
            resolve_path("/proj", "src/a.rs"),
            PathBuf::from("/proj/src/a.rs")
        );
        assert_eq!(resolve_path("/proj", "."), PathBuf::from("/proj"));
        assert_eq!(resolve_path("/proj", ""), PathBuf::from("/proj"));
    }

    #[test]
    fn skip_dirs_include_target_and_git() {
        assert!(is_skip_dir("target"));
        assert!(is_skip_dir(".git"));
        assert!(is_skip_dir("node_modules"));
        assert!(!is_skip_dir("src"));
        assert!(!is_skip_dir("crates"));
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(500), "500 B");
        assert!(human_size(2048).contains("KB"));
    }

    #[test]
    fn walk_skips_target() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main(){}").unwrap();
        fs::write(dir.path().join("target/debug/foo.o"), "bin").unwrap();

        let mut found = Vec::new();
        walk_files(dir.path(), &mut |_p, rel| {
            found.push(rel.to_string());
            true
        });
        assert!(found.iter().any(|f| f.contains("main.rs")));
        assert!(!found.iter().any(|f| f.contains("foo.o")));
    }

    #[test]
    fn suggest_similar_finds_neighbor() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("readme.md"), "x").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "x").unwrap();
        let miss = dir.path().join("Readme.md");
        let s = suggest_similar(&miss, 3);
        assert!(s.iter().any(|n| n.eq_ignore_ascii_case("readme.md")));
    }

    #[test]
    fn suggest_similar_empty_parent_or_nonexistent() {
        // Missing parent (relative name with no dir) -> empty
        assert!(suggest_similar(Path::new("solo.rs"), 3).is_empty());
        // Nonexistent parent -> empty
        assert!(suggest_similar(Path::new("/nonexistent-xyz/a.rs"), 3).is_empty());
    }

    #[test]
    fn glob_match_wildcards() {
        assert!(glob_match("*", "anything.go"));
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.py"));
        assert!(glob_match("src/*.rs", "src/lib.rs"));
        assert!(!glob_match("src/*.rs", "lib.rs"));
        // Fast path: patterns without `*` are exact equality (no ?/[] expansion)
        assert!(glob_match("a?c", "a?c"));
        assert!(!glob_match("a?c", "abc"));
        assert!(glob_match("main.rs", "main.rs"));
        assert!(!glob_match("main.rs", "Main.rs"));
        // Invalid pattern falls back to exact equality
        assert!(glob_match("[", "["));
        assert!(!glob_match("[", "x"));
    }

    #[test]
    fn display_path_relative_and_outside() {
        assert_eq!(
            display_path(Path::new("/proj/src/a.rs"), "/proj"),
            "src/a.rs"
        );
        assert_eq!(display_path(Path::new("/proj"), "/proj"), ".");
        // Outside the working dir -> absolute display
        assert_eq!(display_path(Path::new("/etc/hosts"), "/proj"), "/etc/hosts");
    }

    #[test]
    fn binary_sniff_detects_nul() {
        assert!(is_binary_bytes(&[0u8; 4]));
        assert!(is_binary_bytes(&b"text\x00more"[..]));
        assert!(!is_binary_bytes(b"plain text"));
        assert!(!is_binary_bytes(&[]));
        // NUL past the sniff window is ignored
        let mut buf = vec![b'a'; BINARY_SNIFF_LEN];
        buf.push(0);
        assert!(!is_binary_bytes(&buf));
    }

    #[test]
    fn list_dir_entries_sorted_dirs_first() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("b.txt"), "x").unwrap();
        fs::write(dir.path().join("a.rs"), "x").unwrap();
        fs::create_dir(dir.path().join("zdir")).unwrap();
        fs::create_dir(dir.path().join("adir")).unwrap();

        let entries = list_dir_entries(dir.path(), &[]).unwrap();
        // dirs first (alpha), then files (alpha)
        assert_eq!(entries[0].name, "adir");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "zdir");
        assert!(entries[1].is_dir);
        assert_eq!(entries[2].name, "a.rs");
        assert!(!entries[2].is_dir);
        assert_eq!(entries[3].name, "b.txt");
        // dirs have no size, files do
        assert_eq!(entries[0].size, None);
        assert_eq!(entries[2].size, Some(1));
    }

    #[test]
    fn list_dir_entries_respects_ignore_globs() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("keep.rs"), "x").unwrap();
        fs::write(dir.path().join("skip.tmp"), "x").unwrap();
        fs::write(dir.path().join("skip2.tmp"), "x").unwrap();

        let entries = list_dir_entries(dir.path(), &["*.tmp".to_string()]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "keep.rs");
    }

    #[test]
    fn list_dir_entries_missing_dir_errors() {
        let err = list_dir_entries(Path::new("/nonexistent-xyz"), &[]).unwrap_err();
        assert!(err.contains("Failed to list"));
    }

    #[test]
    fn human_size_boundaries() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
        // Caps at GB
        assert!(human_size(1024u64.pow(4)).contains("GB"));
    }

    #[test]
    fn file_len_reports_size() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("f.txt"), "hello").unwrap();
        assert_eq!(file_len(&dir.path().join("f.txt")), Some(5));
        assert_eq!(file_len(&dir.path().join("missing.txt")), None);
    }

    #[test]
    fn walk_single_file_root() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("single.rs");
        fs::write(&f, "x").unwrap();
        let mut found = Vec::new();
        walk_files(&f, &mut |_p, rel| {
            found.push(rel.to_string());
            true
        });
        assert_eq!(found, vec!["single.rs"]);
    }

    #[test]
    fn walk_stops_on_false() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::write(dir.path().join("a/1.txt"), "x").unwrap();
        fs::write(dir.path().join("a/2.txt"), "x").unwrap();
        let mut count = 0;
        walk_files(dir.path(), &mut |_p, _rel| {
            count += 1;
            false
        });
        assert_eq!(count, 1);
    }
}
