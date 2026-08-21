//! Session undo/redo with optional git file restore (OpenCode parity).

use std::path::{Path, PathBuf};
use std::process::Command;

use whycode_core::types::Message;

/// Snapshot of conversation + working tree for one undo step.
#[derive(Debug, Clone)]
pub struct HistorySnapshot {
    pub messages: Vec<Message>,
    /// `git stash create` ref, if available
    pub stash_ref: Option<String>,
    /// Absolute paths of untracked files created during the turn
    pub new_files: Vec<PathBuf>,
}

/// Undo/redo stacks for a session.
#[derive(Debug, Default, Clone)]
pub struct SessionHistory {
    undo_stack: Vec<HistorySnapshot>,
    redo_stack: Vec<HistorySnapshot>,
}

impl SessionHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Capture current messages and git state before a user turn.
    pub fn push_before_turn(&mut self, messages: &[Message], project_path: &Path) {
        let stash_ref = git_stash_create(project_path);
        let new_files = Vec::new(); // filled after turn via mark_new_files if needed
        self.undo_stack.push(HistorySnapshot {
            messages: messages.to_vec(),
            stash_ref,
            new_files,
        });
        self.redo_stack.clear();
    }

    /// Record untracked files created during the last turn (for cleanup on undo).
    pub fn mark_new_files(&mut self, files: Vec<PathBuf>) {
        if let Some(snap) = self.undo_stack.last_mut() {
            snap.new_files = files;
        }
    }

    /// Pop undo: returns previous messages. Restores git working tree if possible.
    pub fn undo(
        &mut self,
        current_messages: &[Message],
        project_path: &Path,
    ) -> Option<Vec<Message>> {
        let snap = self.undo_stack.pop()?;
        // Save current state for redo
        self.redo_stack.push(HistorySnapshot {
            messages: current_messages.to_vec(),
            stash_ref: git_stash_create(project_path),
            new_files: Vec::new(),
        });

        // Restore files from stash if we had one
        if let Some(ref stash) = snap.stash_ref {
            let _ = Command::new("git")
                .args(["checkout", stash, "--", "."])
                .current_dir(project_path)
                .output();
        } else {
            // Best-effort: restore tracked files to HEAD
            let _ = Command::new("git")
                .args(["checkout", "--", "."])
                .current_dir(project_path)
                .output();
        }

        for f in &snap.new_files {
            let _ = std::fs::remove_file(f);
        }

        Some(snap.messages)
    }

    /// Pop redo: returns messages after redo. Restores git if possible.
    pub fn redo(
        &mut self,
        current_messages: &[Message],
        project_path: &Path,
    ) -> Option<Vec<Message>> {
        let snap = self.redo_stack.pop()?;
        self.undo_stack.push(HistorySnapshot {
            messages: current_messages.to_vec(),
            stash_ref: git_stash_create(project_path),
            new_files: Vec::new(),
        });

        if let Some(ref stash) = snap.stash_ref {
            let _ = Command::new("git")
                .args(["checkout", stash, "--", "."])
                .current_dir(project_path)
                .output();
        }

        Some(snap.messages)
    }
}

/// Create a stash commit without modifying the working tree. Returns the commit hash.
fn git_stash_create(project_path: &Path) -> Option<String> {
    // Ensure we're in a git repo
    let status = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_path)
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }

    let output = Command::new("git")
        .args(["stash", "create"])
        .current_dir(project_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() { None } else { Some(hash) }
}

/// List untracked + modified files relative to project (for diagnostics).
pub fn git_changed_files(project_path: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_path)
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycode_core::types::{Message, MessageContent, Role};

    fn msg(content: &str) -> Message {
        Message {
            role: Role::User,
            content: MessageContent::text(content),
            tool_call_id: None,
            name: None,
            created_at: None,
        }
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn run_git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn new_history_has_no_undo_or_redo() {
        let h = SessionHistory::new();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn push_then_undo_then_redo_round_trips_messages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();

        // Before the turn: one message on the stack.
        h.push_before_turn(&[msg("m1")], dir.path());
        assert!(h.can_undo());
        assert!(!h.can_redo());

        // Turn added m2; undo restores the pre-turn snapshot.
        let restored = h.undo(&[msg("m1"), msg("m2")], dir.path());
        let restored = restored.expect("undo returns a snapshot");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content.as_text(), Some("m1"));
        assert!(!h.can_undo(), "undo popped the only snapshot");
        assert!(h.can_redo());

        // Redo moves forward again.
        let forward = h.redo(&[msg("m1")], dir.path());
        let forward = forward.expect("redo returns a snapshot");
        assert_eq!(forward.len(), 2);
        assert!(!h.can_redo());
        assert!(h.can_undo());
    }

    #[test]
    fn push_clears_the_redo_stack() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();
        h.push_before_turn(&[msg("a")], dir.path());
        let _ = h.undo(&[msg("a"), msg("b")], dir.path());
        assert!(h.can_redo());

        // A new turn invalidates redo.
        h.push_before_turn(&[msg("a")], dir.path());
        assert!(!h.can_redo(), "new turn clears redo");
    }

    #[test]
    fn undo_and_redo_on_empty_stacks_return_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();
        assert!(h.undo(&[], dir.path()).is_none());
        assert!(h.redo(&[], dir.path()).is_none());
    }

    #[test]
    fn mark_new_files_removes_created_files_on_undo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let new_file = dir.path().join("scratch.txt");
        std::fs::write(&new_file, "data").expect("write");
        assert!(new_file.exists());

        let mut h = SessionHistory::new();
        h.push_before_turn(&[msg("m")], dir.path());
        h.mark_new_files(vec![new_file.clone()]);

        let _ = h.undo(&[msg("m"), msg("m2")], dir.path());
        assert!(!new_file.exists(), "undo must remove marked new files");
    }

    #[test]
    fn git_changed_files_empty_outside_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No git repo (or no git binary) → empty diagnostic list, no panic.
        assert!(git_changed_files(dir.path()).is_empty());
    }

    #[test]
    fn undo_with_stash_restores_tree_when_git_available() {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(run_git(dir.path(), &["init", "-q"]), "git init");
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "v1").expect("write v1");
        assert!(run_git(dir.path(), &["add", "a.txt"]), "git add");
        assert!(
            run_git(
                dir.path(),
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "init",
                ],
            ),
            "git commit"
        );

        // Pre-turn state: uncommitted edit. A turn then changes the file again.
        std::fs::write(&f, "v2").expect("write v2");
        let mut h = SessionHistory::new();
        h.push_before_turn(&[msg("m")], dir.path());
        std::fs::write(&f, "v3").expect("write v3");

        let restored = h.undo(&[msg("m"), msg("m2")], dir.path());
        assert!(restored.is_some(), "undo proceeds even with git restore");
        let content = std::fs::read_to_string(&f).unwrap_or_default();
        assert_eq!(content, "v2", "undo checks out the pre-turn tree");
    }

    #[test]
    fn mark_new_files_without_snapshot_is_a_noop() {
        let mut history = SessionHistory::new();
        history.mark_new_files(vec![PathBuf::from("unused")]);
        assert!(!history.can_undo());
    }

    #[test]
    fn changed_files_reports_porcelain_lines_in_repo() {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(run_git(dir.path(), &["init", "-q"]));
        std::fs::write(dir.path().join("new.txt"), "new").unwrap();
        let changed = git_changed_files(dir.path());
        assert_eq!(changed, vec!["?? new.txt"]);
    }

    #[test]
    fn redo_restores_its_stashed_tree_when_available() {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(run_git(dir.path(), &["init", "-q"]));
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        assert!(run_git(dir.path(), &["add", "a.txt"]));
        assert!(run_git(
            dir.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                "init"
            ]
        ));

        let mut history = SessionHistory::new();
        history.push_before_turn(&[msg("before")], dir.path());
        std::fs::write(&file, "after").unwrap();
        let _ = history.undo(&[msg("after")], dir.path());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
        let restored = history.redo(&[msg("before")], dir.path()).unwrap();
        assert_eq!(restored[0].content.as_text(), Some("after"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
    }

    #[test]
    fn git_helpers_handle_disappearing_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(git_stash_create(&path).is_none());
        assert!(git_changed_files(&path).is_empty());

        let mut history = SessionHistory::new();
        history.push_before_turn(&[msg("before")], &path);
        assert_eq!(history.undo(&[msg("after")], &path).unwrap().len(), 1);
        assert_eq!(history.redo(&[msg("before")], &path).unwrap().len(), 1);
    }

    #[test]
    fn stash_creation_failure_in_damaged_repo_returns_none() {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().unwrap();
        assert!(run_git(dir.path(), &["init", "-q"]));
        std::fs::write(dir.path().join("tracked"), "data").unwrap();
        assert!(run_git(dir.path(), &["add", "tracked"]));
        assert!(run_git(
            dir.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                "init"
            ]
        ));
        std::fs::write(dir.path().join("tracked"), "changed").unwrap();
        std::fs::remove_file(dir.path().join(".git/index")).unwrap();
        std::fs::create_dir(dir.path().join(".git/index")).unwrap();
        assert!(git_stash_create(dir.path()).is_none());
    }
}
