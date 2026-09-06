//! Git worktree isolation for swarm workers.
//!
//! Each worker gets a detached worktree under `.whycodes/swarm/<run>/worker-N`.
//! After the worker finishes, changed files are three-way merged into the main
//! checkout (base vs worktree vs main). Conflicts are reported; worktrees are
//! always removed (success or failure).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of merging one worker's tree back into the main checkout.
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    pub applied: Vec<String>,
    pub conflicts: Vec<MergeConflict>,
    pub deleted: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MergeConflict {
    pub path: String,
    pub reason: String,
}

/// An active swarm worktree.
#[derive(Debug)]
pub struct SwarmWorktree {
    /// Absolute path of the worktree checkout.
    pub path: PathBuf,
    /// Absolute path of the primary repo root (main checkout).
    pub repo_root: PathBuf,
    /// HEAD SHA at create time (merge base).
    pub base_head: String,
    pub worker_id: String,
}

/// True when `dir` is inside a git working tree.
pub fn is_git_repo(dir: &Path) -> bool {
    git_ok(dir, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

/// Resolve the git toplevel for `dir`.
pub fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    git_ok(dir, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
}

/// Create a detached worktree for one swarm worker.
///
/// `dest` must not exist. Uses `git worktree add --detach <dest> HEAD`.
pub fn create_worktree(
    repo_root: &Path,
    dest: &Path,
    worker_id: &str,
) -> Result<SwarmWorktree, String> {
    if dest.exists() {
        return Err(format!("worktree path already exists: {}", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    } else {
        return Err("worktree path has no parent directory".into());
    }

    let base_head = git_ok(repo_root, &["rev-parse", "HEAD"])
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "cannot resolve HEAD (not a git repo?)".to_string())?;

    let dest_s = dest.to_string_lossy();
    let status = Command::new("git")
        .args(["worktree", "add", "--detach", dest_s.as_ref(), "HEAD"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("git worktree add failed to spawn: {e}"))?;

    if !status.status.success() {
        let err = String::from_utf8_lossy(&status.stderr);
        return Err(format!("git worktree add failed: {}", err.trim()));
    }

    Ok(SwarmWorktree {
        path: dest.to_path_buf(),
        repo_root: repo_root.to_path_buf(),
        base_head,
        worker_id: worker_id.to_string(),
    })
}

/// List paths changed in the worktree relative to its index/HEAD (tracked + untracked).
///
/// Returns repo-relative paths (forward slashes when possible).
pub fn changed_relative_paths(worktree: &Path) -> Result<Vec<String>, String> {
    let out = git_ok(worktree, &["status", "--porcelain", "-uall"])
        .ok_or_else(|| "git status failed in worktree".to_string())?;
    let mut paths = Vec::new();
    for line in out.lines() {
        if let Some(path) = porcelain_path(line)
            && !paths.iter().any(|p| p == &path)
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Parse one `git status --porcelain` line into a repo-relative path.
fn porcelain_path(line: &str) -> Option<String> {
    if line.len() < 4 {
        return None;
    }
    // XY PATH or XY ORIG -> PATH (renames)
    let rest = line[3..].trim();
    let path = if let Some((_, right)) = rest.split_once(" -> ") {
        right
    } else {
        rest
    };
    // Unquoted paths; git quotes with " when special chars — strip lightly.
    let path = path.trim_matches('"').to_string();
    if path.is_empty() { None } else { Some(path) }
}

/// Three-way merge worktree changes into `main_root` (usually the same as repo_root).
///
/// For each changed path:
/// - base = `git show <base_head>:path` (empty if missing at base)
/// - work = file in worktree (None if deleted)
/// - main = file in main checkout (None if missing)
/// - apply work when main still equals base; conflict when main diverged
pub fn merge_into_main(wt: &SwarmWorktree, main_root: &Path) -> MergeReport {
    let mut report = MergeReport::default();
    let paths = match changed_relative_paths(&wt.path) {
        Ok(p) => p,
        Err(e) => {
            report.notes.push(e);
            return report;
        }
    };
    if paths.is_empty() {
        report.notes.push("no file changes in worktree".into());
        return report;
    }

    for rel in paths {
        let wt_path = wt.path.join(&rel);
        let main_path = main_root.join(&rel);
        let work = read_optional(&wt_path);
        let main = read_optional(&main_path);
        let base = git_show_blob(&wt.repo_root, &wt.base_head, &rel);

        match (base.as_ref(), work.as_ref(), main.as_ref()) {
            (None, None, _) => continue,
            // Deleted in worktree
            (Some(b), None, Some(m)) if m == b => {
                if let Err(e) = std::fs::remove_file(&main_path) {
                    report.conflicts.push(MergeConflict {
                        path: rel.clone(),
                        reason: format!("failed to delete: {e}"),
                    });
                } else {
                    report.deleted.push(rel);
                }
            }
            (Some(_), None, Some(_)) => {
                report.conflicts.push(MergeConflict {
                    path: rel,
                    reason: "deleted in worker but main checkout diverged".into(),
                });
            }
            (Some(_), None, None) => {
                // already gone on main
                report.deleted.push(rel);
            }
            // Created or modified in worktree
            (_, Some(w), m) => {
                let main_matches_base = match (base.as_ref(), m) {
                    (None, None) => true,
                    (Some(b), Some(cur)) => b == cur,
                    (None, Some(_)) => false, // main created something worker also created
                    (Some(_), None) => false, // main deleted while worker edited
                };
                let already_same = m.map(|cur| cur == w).unwrap_or(false);
                if already_same {
                    report.applied.push(rel);
                    continue;
                }
                if !main_matches_base {
                    report.conflicts.push(MergeConflict {
                        path: rel,
                        reason: "main checkout changed the same path (three-way conflict)".into(),
                    });
                    continue;
                }
                if let Some(parent) = main_path.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    report.conflicts.push(MergeConflict {
                        path: rel,
                        reason: format!("mkdir failed: {e}"),
                    });
                    continue;
                }
                match std::fs::write(&main_path, w) {
                    Ok(()) => report.applied.push(rel),
                    Err(e) => report.conflicts.push(MergeConflict {
                        path: rel,
                        reason: format!("write failed: {e}"),
                    }),
                }
            }
        }
    }
    report
}

/// Remove the worktree and prune metadata. Best-effort; logs errors into the Result Err string.
pub fn remove_worktree(wt: &SwarmWorktree) -> Result<(), String> {
    let dest_s = wt.path.to_string_lossy();
    let output = Command::new("git")
        .args(["worktree", "remove", "--force", dest_s.as_ref()])
        .current_dir(&wt.repo_root)
        .output()
        .map_err(|e| format!("git worktree remove spawn: {e}"))?;

    if !output.status.success() {
        // Fallback: force-delete directory and prune.
        let _ = std::fs::remove_dir_all(&wt.path);
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&wt.repo_root)
            .output();
        let err = String::from_utf8_lossy(&output.stderr);
        if wt.path.exists() {
            return Err(format!(
                "worktree remove failed and path remains: {}",
                err.trim()
            ));
        }
    }
    Ok(())
}

/// Directory for one swarm run: `{project}/.whycodes/swarm/{run_id}`.
pub fn run_dir(project: &Path, run_id: &str) -> PathBuf {
    whycodes_core::project_dir(project)
        .join("swarm")
        .join(run_id)
}

/// Format merge report lines for the swarm worker section.
pub fn format_merge_report(report: &MergeReport) -> String {
    let mut lines = Vec::new();
    if !report.applied.is_empty() {
        lines.push(format!(
            "**Merged into main:** {}",
            report.applied.join(", ")
        ));
    }
    if !report.deleted.is_empty() {
        lines.push(format!(
            "**Deleted on main:** {}",
            report.deleted.join(", ")
        ));
    }
    if !report.conflicts.is_empty() {
        lines.push("**Merge conflicts:**".into());
        for c in &report.conflicts {
            lines.push(format!("- `{}`: {}", c.path, c.reason));
        }
    }
    for n in &report.notes {
        lines.push(format!("_{n}_"));
    }
    lines.join("\n")
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn git_show_blob(repo: &Path, rev: &str, rel: &str) -> Option<Vec<u8>> {
    let spec = format!("{rev}:{rel}");
    let output = Command::new("git")
        .args(["show", &spec])
        .current_dir(repo)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

fn git_ok(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "swarm_worktree_tests.rs"]
mod tests;
