//! Git working-tree snapshot used by `whycodes slop`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::Error;

#[derive(Debug)]
pub(crate) struct Snapshot {
    pub base: String,
    pub head: String,
    pub delta_loc: i64,
    pub files_changed: usize,
    pub contents: Vec<(String, String)>,
}

pub(crate) fn snapshot(dir: &Path, base: Option<&str>) -> Result<Snapshot, Error> {
    if !dir.join(".git").exists() && git_rev_parse(dir, "HEAD").is_err() {
        return Err(Error {
            message: "not a git repository".into(),
        });
    }

    let resolved = resolve_base(dir, base)?;
    let numstat = git_ok(
        dir,
        &["diff", "--numstat", "--no-color", &resolved, "--"],
        "git diff --numstat",
    )?;
    let (mut delta_loc, mut files_changed, mut paths) = parse_numstat(&numstat);

    if let Ok(extra) = git_ok(
        dir,
        &["ls-files", "--others", "--exclude-standard"],
        "git ls-files",
    ) {
        for rel in extra.lines() {
            let rel = normalize_path(rel.trim());
            if rel.is_empty() || paths.iter().any(|p| p == &rel) {
                continue;
            }
            files_changed += 1;
            let abs = dir.join(&rel);
            if let Ok(text) = std::fs::read_to_string(&abs) {
                delta_loc += text.lines().count() as i64;
                if crate::parse::language_for_path(Path::new(&rel)).is_some() {
                    paths.push(rel);
                    continue;
                }
            }
            paths.push(rel);
        }
    }

    let mut contents = Vec::new();
    for rel in paths {
        if crate::parse::language_for_path(Path::new(&rel)).is_none() {
            continue;
        }
        let abs = dir.join(&rel);
        if let Ok(text) = std::fs::read_to_string(&abs) {
            contents.push((rel, text));
        }
    }

    Ok(Snapshot {
        base: resolved,
        head: "working-tree".into(),
        delta_loc,
        files_changed,
        contents,
    })
}

fn resolve_base(dir: &Path, base: Option<&str>) -> Result<String, Error> {
    if let Some(b) = base.map(str::trim).filter(|s| !s.is_empty()) {
        return git_rev_parse(dir, b);
    }
    if let Ok(default) = default_branch(dir)
        && let Ok(mb) = merge_base(dir, &default)
    {
        return Ok(mb);
    }
    git_rev_parse(dir, "HEAD")
}

fn default_branch(dir: &Path) -> Result<String, Error> {
    if let Ok(out) = git_ok(
        dir,
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
        "git symbolic-ref",
    ) {
        let s = out.trim();
        if let Some(name) = s.strip_prefix("refs/remotes/origin/") {
            return Ok(format!("origin/{name}"));
        }
        if !s.is_empty() {
            return Ok(s.to_string());
        }
    }
    for cand in ["main", "master"] {
        if git_rev_parse(dir, cand).is_ok() {
            return Ok(cand.to_string());
        }
    }
    Err(Error {
        message: "could not determine default branch".into(),
    })
}

fn merge_base(dir: &Path, other: &str) -> Result<String, Error> {
    git_ok(dir, &["merge-base", "HEAD", other], "git merge-base").map(|s| s.trim().to_string())
}

fn git_rev_parse(dir: &Path, rev: &str) -> Result<String, Error> {
    git_ok(dir, &["rev-parse", "--verify", rev], "git rev-parse").map(|s| s.trim().to_string())
}

fn git_ok(dir: &Path, args: &[&str], label: &str) -> Result<String, Error> {
    match git(dir, args) {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let msg = err.lines().next().unwrap_or("failed").trim();
            Err(Error {
                message: format!("{label}: {msg}"),
            })
        }
        Err(e) => Err(Error {
            message: format!("git unavailable: {e}"),
        }),
    }
}

fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git").args(args).current_dir(dir).output()
}

/// `added - deleted` across files. Binary rows (`-  -  path`) contribute 0.
pub(crate) fn parse_numstat(raw: &str) -> (i64, usize, Vec<String>) {
    let mut delta = 0i64;
    let mut files = 0usize;
    let mut paths = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let added = parts.next().unwrap_or("-");
        let deleted = parts.next().unwrap_or("-");
        let path = parts.next().unwrap_or("").trim();
        if path.is_empty() {
            continue;
        }
        files += 1;
        if added != "-"
            && let Ok(n) = added.parse::<i64>()
        {
            delta += n;
        }
        if deleted != "-"
            && let Ok(n) = deleted.parse::<i64>()
        {
            delta -= n;
        }
        paths.push(normalize_path(path));
    }
    (delta, files, paths)
}

fn normalize_path(p: &str) -> String {
    PathBuf::from(p.replace('\\', "/"))
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
