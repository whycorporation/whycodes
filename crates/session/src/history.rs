//! Session undo/redo with optional git file restore.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use whycodes_core::types::Message;

use crate::worktree_snapshot::{TreeSnapshot, restore_files, snapshot};

/// A working-tree snapshot that may still be computing on its own thread.
///
/// The `git` spawns cost ~150 ms on Windows, and the TUI ran them on its
/// event loop at every submit. The snapshot owns the thread: an undo after a
/// cancelled or force-stopped turn still waits for the real snapshot.
#[derive(Debug, Clone)]
pub struct StashSlot(Arc<Mutex<Stash>>);

#[derive(Debug)]
enum Stash {
    Running(JoinHandle<TreeSnapshot>),
    Done(TreeSnapshot),
}

impl StashSlot {
    fn spawn(project_path: &Path) -> Self {
        let dir = project_path.to_path_buf();
        Self::with(Stash::Running(std::thread::spawn(move || snapshot(&dir))))
    }

    fn done(stash: TreeSnapshot) -> Self {
        Self::with(Stash::Done(stash))
    }

    fn with(stash: Stash) -> Self {
        Self(Arc::new(Mutex::new(stash)))
    }

    /// The snapshot, waiting for the background `git` if it still runs.
    pub fn wait(&self) -> TreeSnapshot {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let stash = match std::mem::replace(&mut *state, Stash::Done(TreeSnapshot::NoRepo)) {
            // A panicked `git` thread leaves no snapshot to restore from.
            Stash::Running(handle) => handle
                .join()
                .unwrap_or_else(|_| TreeSnapshot::Failed("snapshot thread panicked".into())),
            Stash::Done(stash) => stash,
        };
        *state = Stash::Done(stash.clone());
        stash
    }
}

/// What `/undo` or `/redo` went back to.
#[derive(Debug)]
pub struct Restored {
    pub messages: Vec<Message>,
    /// Why the working tree was not put back, when it was not. Show it:
    /// the conversation moved but the files did not.
    pub files_note: Option<String>,
}

/// Default for `[session] undo_depth`: turns `/undo` can step back.
pub const DEFAULT_UNDO_DEPTH: usize = 50;

/// Conversation + working tree for one undo step.
///
/// The transcript is not copied per turn: `len` names a prefix of the shared
/// message log `store` (see [`SessionHistory::capture`]). A full copy made
/// before every turn grew memory with the square of the turn count.
#[derive(Debug, Clone)]
struct HistorySnapshot {
    store: u64,
    len: usize,
    /// The tree before the turn (see [`StashSlot`]).
    stash: StashSlot,
}

/// One copy of the transcript that snapshots index into by length.
///
/// Turns only append, so one log serves every snapshot until a compaction,
/// spill or prune rewrites a message an older snapshot still needs.
#[derive(Debug, Clone)]
struct Store {
    id: u64,
    messages: Vec<Message>,
}

/// Undo/redo stacks for a session.
#[derive(Debug, Clone)]
pub struct SessionHistory {
    undo_stack: VecDeque<HistorySnapshot>,
    redo_stack: Vec<HistorySnapshot>,
    /// Message logs still referenced; the last one is extended next.
    stores: Vec<Store>,
    next_store: u64,
    /// Most undo entries kept; `0` = unlimited.
    depth: usize,
    /// The cap dropped at least one entry.
    dropped: bool,
}

impl Default for SessionHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionHistory {
    /// History capped at [`DEFAULT_UNDO_DEPTH`] turns.
    pub fn new() -> Self {
        Self::with_depth(DEFAULT_UNDO_DEPTH)
    }

    /// History keeping at most `depth` undo entries (`0` = unlimited). The
    /// oldest entries drop first.
    pub fn with_depth(depth: usize) -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            stores: Vec::new(),
            next_store: 0,
            depth,
            dropped: false,
        }
    }

    /// Forget every entry (new or switched session), keeping the depth cap.
    pub fn clear(&mut self) {
        *self = Self::with_depth(self.depth);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// `Some(undo_depth)` when nothing is left to undo because the cap
    /// dropped older turns, so `/undo` can say why it stops there.
    pub fn exhausted_depth(&self) -> Option<usize> {
        (self.dropped && self.undo_stack.is_empty()).then_some(self.depth)
    }

    /// Capture current messages before a user turn and start the working
    /// tree snapshot on a background thread. Wait on the returned slot
    /// before the turn can edit files, so it shows the tree the user had.
    pub fn push_before_turn(&mut self, messages: &[Message], project_path: &Path) -> StashSlot {
        let stash = StashSlot::spawn(project_path);
        self.record_turn(messages, stash.clone());
        stash
    }

    fn record_turn(&mut self, messages: &[Message], stash: StashSlot) {
        self.redo_stack.clear();
        let (store, len) = self.capture(messages);
        self.push_undo(HistorySnapshot { store, len, stash });
        self.gc();
    }

    /// Pop undo: returns previous messages. Puts files back as they were
    /// before that turn, new files removed, when git can do so safely.
    pub fn undo(&mut self, current_messages: &[Message], project_path: &Path) -> Option<Restored> {
        let snap = self.undo_stack.pop_back()?;
        // Copy out before `capture` may reuse the log past this prefix.
        let messages = self.messages_of(&snap);
        // Save current state for redo, files included: the undo below may
        // delete files the turn created, and redo brings them back.
        let (store, len) = self.capture(current_messages);
        let now = snapshot(project_path);
        self.redo_stack.push(HistorySnapshot {
            store,
            len,
            stash: StashSlot::done(now.clone()),
        });
        self.gc();

        let files_note = restore_files(&snap.stash.wait(), &now, project_path);
        Some(Restored {
            messages,
            files_note,
        })
    }

    /// Pop redo: returns messages after redo, files put back the same way.
    pub fn redo(&mut self, current_messages: &[Message], project_path: &Path) -> Option<Restored> {
        let snap = self.redo_stack.pop()?;
        let messages = self.messages_of(&snap);
        let (store, len) = self.capture(current_messages);
        let now = snapshot(project_path);
        self.push_undo(HistorySnapshot {
            store,
            len,
            stash: StashSlot::done(now.clone()),
        });
        self.gc();

        let files_note = restore_files(&snap.stash.wait(), &now, project_path);
        Some(Restored {
            messages,
            files_note,
        })
    }

    /// Record `messages` as `(store, len)` without copying the prefix the
    /// live log already holds.
    ///
    /// The live log is cut back to the shared prefix and extended when no
    /// remaining snapshot reads past that prefix (appended turns, or a new
    /// turn after `/undo`). Otherwise an earlier message was rewritten while
    /// a snapshot still needs the old one, and a new log starts.
    fn capture(&mut self, messages: &[Message]) -> (u64, usize) {
        if let Some(live) = self.stores.last_mut() {
            let needed = self
                .undo_stack
                .iter()
                .chain(&self.redo_stack)
                .filter(|snap| snap.store == live.id)
                .map(|snap| snap.len)
                .max()
                .unwrap_or(0);
            let shared = live
                .messages
                .iter()
                .zip(messages)
                .take_while(|(kept, now)| kept == now)
                .count();
            if shared >= needed {
                live.messages.truncate(shared);
                live.messages.extend_from_slice(&messages[shared..]);
                return (live.id, messages.len());
            }
        }
        self.next_store += 1;
        self.stores.push(Store {
            id: self.next_store,
            messages: messages.to_vec(),
        });
        (self.next_store, messages.len())
    }

    fn messages_of(&self, snap: &HistorySnapshot) -> Vec<Message> {
        self.stores
            .iter()
            .find(|store| store.id == snap.store)
            .and_then(|store| store.messages.get(..snap.len))
            .map(<[Message]>::to_vec)
            .unwrap_or_default()
    }

    fn push_undo(&mut self, snap: HistorySnapshot) {
        self.undo_stack.push_back(snap);
        while self.depth > 0 && self.undo_stack.len() > self.depth {
            self.undo_stack.pop_front();
            self.dropped = true;
        }
    }

    /// Drop logs no snapshot reads any more (the live one stays).
    fn gc(&mut self) {
        let live = self.stores.last().map(|store| store.id);
        let (undo, redo) = (&self.undo_stack, &self.redo_stack);
        self.stores.retain(|store| {
            Some(store.id) == live || undo.iter().chain(redo).any(|snap| snap.store == store.id)
        });
    }
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

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages
            .iter()
            .map(|m| m.content.as_text().unwrap_or_default())
            .collect()
    }

    /// Text bytes every message log holds (what the history keeps alive).
    fn retained_bytes(h: &SessionHistory) -> usize {
        h.stores
            .iter()
            .flat_map(|store| &store.messages)
            .map(|m| m.content.as_text().unwrap_or_default().len())
            .sum()
    }

    fn record(h: &mut SessionHistory, messages: &[Message]) {
        h.record_turn(messages, StashSlot::done(TreeSnapshot::NoRepo));
    }

    #[test]
    fn two_hundred_turns_retain_linear_bytes() {
        let mut h = SessionHistory::with_depth(0);
        let mut transcript = Vec::new();
        for turn in 0..200 {
            record(&mut h, &transcript);
            transcript.push(msg(&format!("{turn:04}{}", "x".repeat(2044))));
        }
        let transcript_bytes: usize = transcript
            .iter()
            .map(|m| m.content.as_text().unwrap_or_default().len())
            .sum();
        assert_eq!(transcript_bytes, 200 * 2048);
        // A full copy per turn kept 199 * 200 / 2 messages (~40 MB here).
        let retained = retained_bytes(&h);
        assert!(retained < 3 * transcript_bytes, "{retained} retained");
        assert_eq!(h.stores.len(), 1, "appended turns share one log");
        assert_eq!(h.undo_stack.len(), 200);

        let dir = tempfile::tempdir().expect("tempdir");
        let back = h.undo(&transcript, dir.path()).expect("undo").messages;
        assert_eq!(back.as_slice(), &transcript[..199]);
        let back = h.undo(&back, dir.path()).expect("undo again").messages;
        assert_eq!(back.as_slice(), &transcript[..198]);
        let forward = h.redo(&back, dir.path()).expect("redo").messages;
        assert_eq!(forward.as_slice(), &transcript[..199]);
        assert!(retained_bytes(&h) < 3 * transcript_bytes);
    }

    #[test]
    fn rewritten_prefix_keeps_both_logs_until_unreferenced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::with_depth(0);
        record(&mut h, &[msg("a")]);
        record(&mut h, &[msg("a"), msg("b")]);
        // Compaction rewrote the transcript; older snapshots keep the old log.
        record(&mut h, &[msg("summary")]);
        assert_eq!(h.stores.len(), 2);

        let now = [msg("summary"), msg("c")];
        let back = h.undo(&now, dir.path()).expect("undo 1").messages;
        assert_eq!(texts(&back), ["summary"]);
        let back = h.undo(&back, dir.path()).expect("undo 2").messages;
        assert_eq!(texts(&back), ["a", "b"]);
        let back = h.undo(&back, dir.path()).expect("undo 3").messages;
        assert_eq!(texts(&back), ["a"]);
        assert!(!h.can_undo());

        // A new turn clears redo; only the live log survives.
        record(&mut h, &back);
        assert_eq!(h.stores.len(), 1);
        h.clear();
        assert!(h.stores.is_empty());
        assert!(!h.can_undo() && !h.can_redo());
    }

    #[test]
    fn new_turn_after_undo_reuses_the_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();
        record(&mut h, &[msg("a")]);
        let back = h
            .undo(&[msg("a"), msg("b")], dir.path())
            .expect("undo")
            .messages;
        assert_eq!(texts(&back), ["a"]);

        // A different second message after undo replaces `b` in place.
        record(&mut h, &back);
        assert_eq!(h.stores.len(), 1);
        let back = h
            .undo(&[msg("a"), msg("c")], dir.path())
            .expect("undo")
            .messages;
        assert_eq!(texts(&back), ["a"]);
        let forward = h.redo(&back, dir.path()).expect("redo").messages;
        assert_eq!(texts(&forward), ["a", "c"]);
        assert_eq!(h.stores.len(), 1);
    }

    #[test]
    fn depth_cap_drops_oldest_and_reports_exhaustion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::with_depth(2);
        record(&mut h, &[msg("1")]);
        record(&mut h, &[msg("1"), msg("2")]);
        assert_eq!(h.exhausted_depth(), None);
        record(&mut h, &[msg("1"), msg("2"), msg("3")]);
        assert_eq!(h.undo_stack.len(), 2, "oldest entry dropped");
        assert_eq!(h.exhausted_depth(), None, "entries remain");

        let now = [msg("1"), msg("2"), msg("3"), msg("4")];
        let back = h.undo(&now, dir.path()).expect("undo 1").messages;
        assert_eq!(texts(&back), ["1", "2", "3"]);
        let back = h.undo(&back, dir.path()).expect("undo 2").messages;
        assert_eq!(texts(&back), ["1", "2"]);
        assert!(h.undo(&back, dir.path()).is_none());
        assert_eq!(h.exhausted_depth(), Some(2));

        h.clear();
        assert_eq!(h.exhausted_depth(), None, "clear forgets the drop");
        assert_eq!(h.depth, 2, "clear keeps the cap");
    }

    #[test]
    fn default_depth_is_fifty_and_zero_is_unlimited() {
        assert_eq!(SessionHistory::new().depth, DEFAULT_UNDO_DEPTH);
        assert_eq!(SessionHistory::default().depth, 50);
        let mut h = SessionHistory::with_depth(0);
        for n in 0..60 {
            record(&mut h, &vec![msg("m"); n]);
        }
        assert_eq!(h.undo_stack.len(), 60);
        assert_eq!(h.exhausted_depth(), None);
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
        let restored = restored.expect("undo returns a snapshot").messages;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content.as_text(), Some("m1"));
        assert!(!h.can_undo(), "undo popped the only snapshot");
        assert!(h.can_redo());

        // Redo moves forward again.
        let forward = h.redo(&[msg("m1")], dir.path());
        let forward = forward.expect("redo returns a snapshot").messages;
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
    fn git_changed_files_empty_outside_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No git repo (or no git binary) → empty diagnostic list, no panic.
        assert!(git_changed_files(dir.path()).is_empty());
    }

    #[test]
    fn stash_slot_wait_survives_a_poisoned_lock() {
        let slot = StashSlot::done(TreeSnapshot::Failed("kept".into()));
        let held = slot.clone();
        let poisoned = std::thread::spawn(move || {
            let _guard = held.0.lock().expect("first lock");
            panic!("poison the slot");
        })
        .join();
        assert!(poisoned.is_err());
        assert_eq!(slot.wait(), TreeSnapshot::Failed("kept".into()));
    }

    #[test]
    fn stash_slot_waits_once_and_keeps_the_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();
        let slot = h.push_before_turn(&[msg("m")], dir.path());
        // Outside a repo the background `git` yields no snapshot; a second
        // wait reads the stored result instead of joining again.
        assert_eq!(slot.wait(), TreeSnapshot::NoRepo);
        assert_eq!(slot.wait(), TreeSnapshot::NoRepo);
        let done = StashSlot::done(TreeSnapshot::Failed("abc".into()));
        assert_eq!(done.clone().wait(), TreeSnapshot::Failed("abc".into()));
        assert_eq!(done.wait(), TreeSnapshot::Failed("abc".into()));
    }

    #[test]
    fn panicked_snapshot_thread_counts_as_failed() {
        let slot = StashSlot::with(Stash::Running(std::thread::spawn(|| -> TreeSnapshot {
            panic!("git thread died")
        })));
        assert!(matches!(slot.wait(), TreeSnapshot::Failed(_)));
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
    fn git_helpers_handle_disappearing_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(git_changed_files(&path).is_empty());

        let mut history = SessionHistory::new();
        history.push_before_turn(&[msg("before")], &path);
        let undone = history.undo(&[msg("after")], &path).unwrap();
        assert_eq!((undone.messages.len(), undone.files_note), (1, None));
        let redone = history.redo(&[msg("before")], &path).unwrap();
        assert_eq!((redone.messages.len(), redone.files_note), (1, None));
    }

    // --- Working tree restore against real git -------------------------

    /// `git` in `dir` with a fixed identity; panics with stderr on failure.
    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("spawn git");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "git {args:?}: {stderr}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `git status` including every untracked and ignored file.
    fn status(dir: &Path) -> String {
        git_ok(
            dir,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignored",
            ],
        )
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn read(dir: &Path, rel: &str) -> Option<String> {
        std::fs::read_to_string(dir.join(rel)).ok()
    }

    /// A repository with `a.txt` committed and `*.log` ignored.
    fn repo() -> tempfile::TempDir {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().unwrap();
        git_ok(dir.path(), &["init", "-q"]);
        git_ok(dir.path(), &["config", "core.autocrlf", "false"]);
        write(dir.path(), "a.txt", "v1\n");
        write(dir.path(), ".gitignore", "*.log\n");
        git_ok(dir.path(), &["add", "-A"]);
        git_ok(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    /// One turn: snapshot, let `edit` change files, return the history.
    fn turn(dir: &Path, edit: impl FnOnce(&Path)) -> SessionHistory {
        let mut h = SessionHistory::new();
        let before = h.push_before_turn(&[msg("m")], dir).wait();
        assert!(matches!(before, TreeSnapshot::Git(_)), "{before:?}");
        edit(dir);
        h
    }

    fn undo_files(h: &mut SessionHistory, dir: &Path) -> Option<String> {
        h.undo(&[msg("m"), msg("m2")], dir).unwrap().files_note
    }

    fn redo_files(h: &mut SessionHistory, dir: &Path) -> Option<String> {
        h.redo(&[msg("m")], dir).unwrap().files_note
    }

    #[test]
    fn undo_restores_the_whole_tree_and_redo_brings_the_turn_back() {
        let dir = repo();
        let d = dir.path();
        // The user's own state before the turn: an unstaged edit, a staged
        // file, an untracked file and an ignored one.
        write(d, "a.txt", "user wip\n");
        write(d, "s.txt", "staged\n");
        git_ok(d, &["add", "s.txt"]);
        write(d, "mine.txt", "untracked\n");
        write(d, "keep.log", "ignored\n");
        let before = status(d);

        let mut h = turn(d, |d| {
            write(d, "a.txt", "turn edit\n");
            write(d, "new/deep/n.txt", "created\n");
            write(d, "name with space ü.txt", "created\n");
            std::fs::remove_file(d.join("mine.txt")).unwrap();
            write(d, "added.txt", "staged by the turn\n");
            git_ok(d, &["add", "added.txt"]);
            write(d, "build.log", "ignored, made by the turn\n");
        });
        let after = status(d);

        assert_eq!(undo_files(&mut h, d), None);
        assert_eq!(read(d, "a.txt").as_deref(), Some("user wip\n"));
        assert_eq!(read(d, "mine.txt").as_deref(), Some("untracked\n"));
        assert_eq!(read(d, "s.txt").as_deref(), Some("staged\n"));
        assert!(!d.join("new").exists(), "created directories go too");
        assert!(!d.join("name with space ü.txt").exists());
        assert!(!d.join("added.txt").exists());
        assert_eq!(
            read(d, "build.log").as_deref(),
            Some("ignored, made by the turn\n"),
            "ignored files are never touched"
        );
        std::fs::remove_file(d.join("build.log")).unwrap();
        assert_eq!(status(d), before);
        write(d, "build.log", "ignored, made by the turn\n");

        assert_eq!(redo_files(&mut h, d), None);
        assert_eq!(status(d), after);
        assert_eq!(read(d, "a.txt").as_deref(), Some("turn edit\n"));
        assert_eq!(read(d, "new/deep/n.txt").as_deref(), Some("created\n"));
        assert!(!d.join("mine.txt").exists());
    }

    /// An editor's background `git status` holds `index.lock` as the turn
    /// starts. The snapshot reads a copy of the index, so it still works.
    #[test]
    fn a_held_index_lock_does_not_stop_the_snapshot() {
        let dir = repo();
        let d = dir.path();
        write(d, "a.txt", "user wip\n");
        let lock = d.join(".git/index.lock");
        std::fs::write(&lock, "").unwrap();
        let mut h = turn(d, |d| write(d, "a.txt", "turn edit\n"));
        std::fs::remove_file(&lock).unwrap();
        assert_eq!(undo_files(&mut h, d), None);
        assert_eq!(read(d, "a.txt").as_deref(), Some("user wip\n"));
    }

    #[test]
    fn a_held_index_lock_at_undo_leaves_files_alone() {
        let dir = repo();
        let d = dir.path();
        let mut h = turn(d, |d| {
            write(d, "a.txt", "turn edit\n");
            git_ok(d, &["add", "a.txt"]);
        });
        std::fs::write(d.join(".git/index.lock"), "").unwrap();
        let note = undo_files(&mut h, d).expect("undo explains");
        assert!(
            note.starts_with("files left as they are: could not restore the staging area"),
            "{note}"
        );
        assert_eq!(read(d, "a.txt").as_deref(), Some("turn edit\n"));
    }

    #[test]
    fn a_repo_without_commits_restores_too() {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q"]);
        write(d, "b.txt", "staged\n");
        git_ok(d, &["add", "b.txt"]);
        write(d, "b.txt", "staged + unstaged\n");
        let before = status(d);
        let mut h = turn(d, |d| {
            write(d, "b.txt", "turn edit\n");
            write(d, "c.txt", "created\n");
        });
        assert_eq!(undo_files(&mut h, d), None);
        assert_eq!(read(d, "b.txt").as_deref(), Some("staged + unstaged\n"));
        assert!(!d.join("c.txt").exists());
        assert_eq!(status(d), before);
    }

    #[test]
    fn a_failed_snapshot_leaves_files_alone() {
        let dir = repo();
        let d = dir.path();
        write(d, "a.txt", "user wip\n");
        // An index git cannot read: the snapshot must fail, not guess.
        std::fs::remove_file(d.join(".git/index")).unwrap();
        std::fs::create_dir(d.join(".git/index")).unwrap();
        let mut h = SessionHistory::new();
        let before = h.push_before_turn(&[msg("m")], d).wait();
        assert!(matches!(before, TreeSnapshot::Failed(_)), "{before:?}");
        write(d, "a.txt", "turn edit\n");
        let note = undo_files(&mut h, d).expect("undo explains");
        assert!(
            note.starts_with("files left as they are: git could not snapshot"),
            "{note}"
        );
        assert_eq!(read(d, "a.txt").as_deref(), Some("turn edit\n"));
    }

    #[test]
    fn undo_refuses_after_head_moved() {
        let dir = repo();
        let d = dir.path();
        let mut h = turn(d, |d| {
            write(d, "a.txt", "committed by the turn\n");
            write(d, "new.txt", "created\n");
            git_ok(d, &["commit", "-qam", "turn"]);
        });
        let note = undo_files(&mut h, d).expect("undo explains");
        assert!(note.contains("HEAD moved"), "{note}");
        assert_eq!(read(d, "a.txt").as_deref(), Some("committed by the turn\n"));
        assert!(d.join("new.txt").exists());
    }

    /// `side` adds `side.txt`; with `conflict` both branches change `a.txt`.
    fn diverged_repo(conflict: bool) -> tempfile::TempDir {
        let dir = repo();
        let d = dir.path();
        git_ok(d, &["checkout", "-qb", "side"]);
        write(d, "side.txt", "side\n");
        if conflict {
            write(d, "a.txt", "side\n");
        }
        git_ok(d, &["add", "-A"]);
        git_ok(d, &["commit", "-qm", "side"]);
        git_ok(d, &["checkout", "-q", "-"]);
        write(d, "a.txt", "main\n");
        git_ok(d, &["commit", "-qam", "main"]);
        dir
    }

    #[test]
    fn undo_refuses_while_a_merge_is_in_progress() {
        let dir = diverged_repo(false);
        let d = dir.path();
        let mut h = turn(d, |d| write(d, "z.txt", "turn\n"));
        git_ok(d, &["merge", "-q", "--no-commit", "--no-ff", "side"]);
        let note = undo_files(&mut h, d).expect("undo explains");
        assert!(note.contains("a merge is in progress"), "{note}");
        assert!(d.join("z.txt").exists());
        assert!(
            d.join(".git/MERGE_HEAD").exists(),
            "the merge is left alone"
        );
    }

    #[test]
    fn undo_refuses_with_unmerged_paths() {
        let dir = diverged_repo(true);
        let d = dir.path();
        let mut h = turn(d, |d| write(d, "z.txt", "turn\n"));
        // CI runners have no global identity; without one `git merge` stops
        // before it starts and leaves no merge state behind.
        let merge = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["merge", "-q", "side"])
            .current_dir(d)
            .output()
            .unwrap();
        assert!(!merge.status.success(), "the merge conflicts");
        assert!(d.join(".git/MERGE_HEAD").exists(), "a conflicted merge");
        let conflicted = read(d, "a.txt").unwrap();
        let note = undo_files(&mut h, d).expect("undo explains");
        assert!(
            note.contains("could not snapshot the current files"),
            "{note}"
        );
        assert_eq!(read(d, "a.txt").unwrap(), conflicted);
        assert!(d.join("z.txt").exists());
    }

    #[test]
    fn the_users_stash_list_is_never_touched() {
        let dir = repo();
        let d = dir.path();
        write(d, "a.txt", "stashed by the user\n");
        git_ok(d, &["stash", "-q"]);
        let stash_list = git_ok(d, &["stash", "list"]);
        write(d, "a.txt", "user wip\n");
        let mut h = turn(d, |d| write(d, "a.txt", "turn\n"));
        assert_eq!(undo_files(&mut h, d), None);
        assert_eq!(redo_files(&mut h, d), None);
        assert_eq!(undo_files(&mut h, d), None);
        assert_eq!(git_ok(d, &["stash", "list"]), stash_list);
        assert_eq!(read(d, "a.txt").as_deref(), Some("user wip\n"));
        // The user's own entry still applies on top.
        git_ok(d, &["checkout", "-q", "--", "a.txt"]);
        git_ok(d, &["stash", "pop", "-q"]);
        assert_eq!(read(d, "a.txt").as_deref(), Some("stashed by the user\n"));
    }

    #[test]
    fn a_project_in_a_subdirectory_only_restores_its_own_files() {
        let dir = repo();
        let root = dir.path();
        write(root, "app/main.txt", "v1\n");
        git_ok(root, &["add", "-A"]);
        git_ok(root, &["commit", "-qm", "app"]);
        let app = root.join("app");
        let mut h = turn(&app, |app| {
            write(app, "main.txt", "turn\n");
            write(app, "made.txt", "turn\n");
        });
        // Someone edits a file outside the project after the turn.
        write(root, "a.txt", "outside\n");
        assert_eq!(undo_files(&mut h, &app), None);
        assert_eq!(read(&app, "main.txt").as_deref(), Some("v1\n"));
        assert!(!app.join("made.txt").exists());
        assert_eq!(read(root, "a.txt").as_deref(), Some("outside\n"));
    }

    /// `core.autocrlf=true` and an `eol=crlf` rule used to turn a restored LF
    /// file into CRLF. Bytes come back exactly as they were.
    #[test]
    fn line_endings_come_back_byte_for_byte() {
        let dir = repo();
        let d = dir.path();
        git_ok(d, &["config", "core.autocrlf", "true"]);
        write(d, ".gitattributes", "*.txt text eol=crlf\n");
        write(d, "lf.txt", "one\ntwo\n");
        write(d, "crlf.txt", "one\r\ntwo\r\n");
        let mut h = turn(d, |d| {
            write(d, "lf.txt", "changed\n");
            write(d, "crlf.txt", "changed\n");
        });
        assert_eq!(undo_files(&mut h, d), None);
        assert_eq!(std::fs::read(d.join("lf.txt")).unwrap(), b"one\ntwo\n");
        assert_eq!(
            std::fs::read(d.join("crlf.txt")).unwrap(),
            b"one\r\ntwo\r\n"
        );
    }
}
