//! Session undo/redo with optional git file restore (OpenCode parity).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use whycodes_core::types::Message;

/// A `git stash create` ref that may still be computing on its own thread.
///
/// Two `git` spawns cost ~150 ms on Windows, and the TUI ran them on its
/// event loop at every submit. The snapshot owns the thread: an undo after a
/// cancelled or force-stopped turn still waits for the real ref instead of
/// falling back to `git checkout -- .`, which would drop uncommitted work.
#[derive(Debug, Clone)]
pub struct StashSlot(Arc<Mutex<Stash>>);

#[derive(Debug)]
enum Stash {
    Running(JoinHandle<Option<String>>),
    Done(Option<String>),
}

impl StashSlot {
    fn spawn(project_path: &Path) -> Self {
        let dir = project_path.to_path_buf();
        Self::with(Stash::Running(std::thread::spawn(move || {
            git_stash_create(&dir)
        })))
    }

    fn done(stash: Option<String>) -> Self {
        Self::with(Stash::Done(stash))
    }

    fn with(stash: Stash) -> Self {
        Self(Arc::new(Mutex::new(stash)))
    }

    /// The stash ref, waiting for the background `git` if it still runs.
    pub fn wait(&self) -> Option<String> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let stash = match std::mem::replace(&mut *state, Stash::Done(None)) {
            // A panicked `git` thread leaves no ref, same as `git` failing.
            Stash::Running(handle) => handle.join().unwrap_or_default(),
            Stash::Done(stash) => stash,
        };
        *state = Stash::Done(stash.clone());
        stash
    }
}

/// Snapshot of conversation + working tree for one undo step.
#[derive(Debug, Clone)]
pub struct HistorySnapshot {
    pub messages: Vec<Message>,
    /// `git stash create` ref, if available (see [`StashSlot`]).
    pub stash: StashSlot,
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

    /// Capture current messages before a user turn and start `git stash
    /// create` on a background thread. Wait on the returned slot before the
    /// turn can edit files, so the stash shows the tree the user had.
    pub fn push_before_turn(&mut self, messages: &[Message], project_path: &Path) -> StashSlot {
        let stash = StashSlot::spawn(project_path);
        let new_files = Vec::new(); // filled after turn via mark_new_files if needed
        self.undo_stack.push(HistorySnapshot {
            messages: messages.to_vec(),
            stash: stash.clone(),
            new_files,
        });
        self.redo_stack.clear();
        stash
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
            stash: StashSlot::done(git_stash_create(project_path)),
            new_files: Vec::new(),
        });

        // Restore files from stash if we had one
        if let Some(ref stash) = snap.stash.wait() {
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
            stash: StashSlot::done(git_stash_create(project_path)),
            new_files: Vec::new(),
        });

        if let Some(ref stash) = snap.stash.wait() {
            let _ = Command::new("git")
                .args(["checkout", stash, "--", "."])
                .current_dir(project_path)
                .output();
        }

        Some(snap.messages)
    }
}

/// Create a stash commit without modifying the working tree. Returns the commit hash.
///
/// One spawn: outside a work tree `git stash create` itself exits non-zero,
/// so a separate `rev-parse --is-inside-work-tree` probe only added a second
/// process (~40 ms on Windows) to every turn.
fn git_stash_create(project_path: &Path) -> Option<String> {
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
    use whycodes_core::types::{Message, MessageContent, Role};

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
        // Callers wait before the turn can edit (TUI `prepare_turn`, CLI).
        let stash = h.push_before_turn(&[msg("m")], dir.path()).wait();
        assert!(stash.is_some(), "dirty tree yields a stash ref");
        std::fs::write(&f, "v3").expect("write v3");

        let restored = h.undo(&[msg("m"), msg("m2")], dir.path());
        assert!(restored.is_some(), "undo proceeds even with git restore");
        let content = std::fs::read_to_string(&f).unwrap_or_default();
        assert_eq!(content, "v2", "undo checks out the pre-turn tree");
    }

    #[test]
    fn stash_slot_wait_survives_a_poisoned_lock() {
        let slot = StashSlot::done(Some("kept".into()));
        let held = slot.clone();
        let poisoned = std::thread::spawn(move || {
            let _guard = held.0.lock().expect("first lock");
            panic!("poison the slot");
        })
        .join();
        assert!(poisoned.is_err());
        assert_eq!(slot.wait().as_deref(), Some("kept"));
    }

    #[test]
    fn stash_slot_waits_once_and_keeps_the_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();
        let slot = h.push_before_turn(&[msg("m")], dir.path());
        // Outside a repo the background `git` yields no ref; a second wait
        // reads the stored result instead of joining again.
        assert_eq!(slot.wait(), None);
        assert_eq!(slot.wait(), None);
        let done = StashSlot::done(Some("abc".into()));
        assert_eq!(done.clone().wait().as_deref(), Some("abc"));
        assert_eq!(done.wait().as_deref(), Some("abc"));
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
        history
            .push_before_turn(&[msg("before")], dir.path())
            .wait();
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
