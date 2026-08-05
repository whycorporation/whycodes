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
        // Keep start + short hash of full so long paths stay unique
        let h = simple_hash(raw);
        format!("{}-{:x}", &trimmed[..120], h)
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
}
