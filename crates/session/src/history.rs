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
