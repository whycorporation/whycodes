//! Stable per-repository memory key (shared across worktrees).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Derive a filesystem-safe key for the project that owns `path`.
///
/// Prefers the git toplevel so all worktrees of the same repo share memory
/// (Claude Code / Grok Build parity). Falls back to the canonical path.
pub fn project_key(path: &Path) -> String {
    let root = git_toplevel(path).unwrap_or_else(|| canonicalize_or_self(path));
    sanitize_key(&root.to_string_lossy())
}

/// Absolute path used as the logical project root for memory.
pub fn project_root(path: &Path) -> PathBuf {
    git_toplevel(path).unwrap_or_else(|| canonicalize_or_self(path))
}

fn git_toplevel(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

fn canonicalize_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Map an absolute path to a single path-component key.
fn sanitize_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => out.push(c),
            '/' | '\\' | ':' | ' ' | '.' => out.push('-'),
            _ => out.push('_'),
        }
    }
    // Collapse runs of '-'
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_dash = false;
    for c in out.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push(c);
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "default".into()
    } else if trimmed.len() > 180 {
        // Keep start + short hash of full so long paths stay unique.
        // `trimmed` is ASCII after sanitize, but floor anyway (same class as
        // mid-UTF-8 `&s[..n]` panics).
        let h = simple_hash(raw);
        format!("{}-{:x}", &trimmed[..trimmed.floor_char_boundary(120)], h)
    } else {
        trimmed.to_string()
    }
}

fn simple_hash(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(16777619);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_separators() {
        let k = sanitize_key("/home/user/dev/my project");
        assert!(!k.contains('/'));
        assert!(!k.contains(' '));
        assert!(k.contains("home"));
    }

    #[test]
    fn empty_becomes_default() {
        assert_eq!(sanitize_key("///"), "default");
    }

    #[test]
    fn long_key_is_hashed_and_stays_on_char_boundary() {
        let raw = format!("/{}", "a".repeat(250));
        let k = sanitize_key(&raw);
        assert!(k.len() < raw.len(), "{k}");
        assert!(k.contains('-'), "{k}");
        assert!(k.is_char_boundary(k.len()));
        // Unicode path chars become `_` (1 byte) so the 120-byte prefix is ASCII.
        let uni = format!("/{}", "ö".repeat(200));
        let ku = sanitize_key(&uni);
        assert!(ku.is_char_boundary(ku.len()));
        assert!(ku.len() < uni.len());
    }

    #[test]
    fn git_toplevel_none_when_not_a_repo() {
        let _guard = crate::TEST_PATH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        assert!(git_toplevel(dir.path()).is_none());
        let root = project_root(dir.path());
        assert!(root.exists() || root == dir.path());
        let key = project_key(dir.path());
        assert!(!key.is_empty());
    }

    #[test]
    fn git_toplevel_reads_show_toplevel() {
        let _guard = crate::TEST_PATH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let top = git_toplevel(dir.path()).expect("git repo should have toplevel");
        assert!(
            top.ends_with(dir.path().file_name().unwrap())
                || top == dir.path().canonicalize().unwrap()
        );
        let missing = dir.path().join("no-such-dir");
        let self_path = canonicalize_or_self(&missing);
        assert_eq!(self_path, missing);

        #[cfg(unix)]
        {
            let real = tempfile::tempdir().unwrap();
            let status = std::process::Command::new("git")
                .args(["init"])
                .current_dir(real.path())
                .status()
                .unwrap();
            assert!(status.success());
            let link_dir = tempfile::tempdir().unwrap();
            let link = link_dir.path().join("alias");
            std::os::unix::fs::symlink(real.path(), &link).unwrap();
            let top = git_toplevel(&link).expect("symlink repo");
            assert_eq!(top, link.canonicalize().unwrap());
        }
    }

    #[test]
    fn git_toplevel_empty_stdout_and_nonzero_exit() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::TEST_PATH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join("git");
        std::fs::write(&git, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let prev = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", dir.path()) };
        let cwd = tempfile::tempdir().unwrap();
        assert!(git_toplevel(cwd.path()).is_none());
        std::fs::write(&git, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(git_toplevel(cwd.path()).is_none());
        match prev {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        let restore = std::env::var_os("PATH");
        unsafe { std::env::remove_var("PATH") };
        let none = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", dir.path()) };
        assert!(git_toplevel(cwd.path()).is_none());
        match none {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        match restore {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }
}
