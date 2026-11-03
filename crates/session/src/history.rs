//! Session undo/redo with optional git file restore.

use std::collections::VecDeque;
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
    Running(JoinHandle<TreeSnapshot>),
    Done(TreeSnapshot),
}

/// What `git stash create` recorded of the working tree before a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeSnapshot {
    /// Uncommitted changes, saved as this stash commit.
    Stash(String),
    /// Nothing uncommitted, so `git checkout -- .` puts the tree back.
    Clean,
    /// Not inside a git work tree; undo leaves files alone, as before.
    NoRepo,
    /// A repository, but git could not take the snapshot (a held
    /// `index.lock`, no first commit yet). Undo must leave files alone:
    /// `git checkout -- .` would throw away the uncommitted work it missed.
    Failed(String),
}

impl StashSlot {
    fn spawn(project_path: &Path) -> Self {
        let dir = project_path.to_path_buf();
        Self::with(Stash::Running(std::thread::spawn(move || {
            git_stash_create(&dir)
        })))
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
    /// Absolute paths of untracked files created during the turn
    new_files: Vec<PathBuf>,
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

    /// Capture current messages before a user turn and start `git stash
    /// create` on a background thread. Wait on the returned slot before the
    /// turn can edit files, so the stash shows the tree the user had.
    pub fn push_before_turn(&mut self, messages: &[Message], project_path: &Path) -> StashSlot {
        let stash = StashSlot::spawn(project_path);
        self.record_turn(messages, stash.clone());
        stash
    }

    fn record_turn(&mut self, messages: &[Message], stash: StashSlot) {
        self.redo_stack.clear();
        let (store, len) = self.capture(messages);
        self.push_undo(HistorySnapshot {
            store,
            len,
            stash,
            new_files: Vec::new(), // filled after turn via mark_new_files if needed
        });
        self.gc();
    }

    /// Record untracked files created during the last turn (for cleanup on undo).
    pub fn mark_new_files(&mut self, files: Vec<PathBuf>) {
        if let Some(snap) = self.undo_stack.back_mut() {
            snap.new_files = files;
        }
    }

    /// Pop undo: returns previous messages. Restores git working tree if possible.
    pub fn undo(&mut self, current_messages: &[Message], project_path: &Path) -> Option<Restored> {
        let snap = self.undo_stack.pop_back()?;
        // Copy out before `capture` may reuse the log past this prefix.
        let messages = self.messages_of(&snap);
        // Save current state for redo
        let (store, len) = self.capture(current_messages);
        self.redo_stack.push(HistorySnapshot {
            store,
            len,
            stash: StashSlot::done(git_stash_create(project_path)),
            new_files: Vec::new(),
        });
        self.gc();

        let files_note = restore_tree(&snap.stash.wait(), project_path);

        for f in &snap.new_files {
            let _ = std::fs::remove_file(f);
        }

        Some(Restored {
            messages,
            files_note,
        })
    }

    /// Pop redo: returns messages after redo. Restores git if possible.
    pub fn redo(&mut self, current_messages: &[Message], project_path: &Path) -> Option<Restored> {
        let snap = self.redo_stack.pop()?;
        let messages = self.messages_of(&snap);
        let (store, len) = self.capture(current_messages);
        self.push_undo(HistorySnapshot {
            store,
            len,
            stash: StashSlot::done(git_stash_create(project_path)),
            new_files: Vec::new(),
        });
        self.gc();

        let files_note = restore_tree(&snap.stash.wait(), project_path);
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

/// Snapshot the working tree as a stash commit without modifying it.
///
/// One spawn per turn: a `rev-parse --is-inside-work-tree` probe runs only
/// when `git stash create` fails, to tell "not a repo" from a real failure.
/// The probe used to run every turn (~40 ms on Windows).
fn git_stash_create(project_path: &Path) -> TreeSnapshot {
    match git(project_path, &["stash", "create"]) {
        Ok(hash) if hash.is_empty() => TreeSnapshot::Clean,
        Ok(hash) => TreeSnapshot::Stash(hash),
        Err(why) => {
            if git(project_path, &["rev-parse", "--is-inside-work-tree"]).is_ok() {
                TreeSnapshot::Failed(why)
            } else {
                TreeSnapshot::NoRepo
            }
        }
    }
}

/// Put the working tree back to `snapshot`. Returns why it was left as it
/// is, when it was.
fn restore_tree(snapshot: &TreeSnapshot, project_path: &Path) -> Option<String> {
    let args = match snapshot {
        TreeSnapshot::Stash(stash) => vec!["checkout", stash.as_str(), "--", "."],
        TreeSnapshot::Clean => vec!["checkout", "--", "."],
        TreeSnapshot::NoRepo => return None,
        TreeSnapshot::Failed(why) => {
            return Some(format!(
                "files left as they are: git could not snapshot them before the turn ({why})"
            ));
        }
    };
    let why = git(project_path, &args).err()?;
    Some(format!("files not restored: git checkout failed ({why})"))
}

/// Run `git` in `dir`: trimmed stdout, or the first stderr line on failure.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").args(args).current_dir(dir).output();
    let output = output.map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .next()
        .unwrap_or("git failed")
        .trim()
        .to_string())
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
        assert!(
            matches!(stash, TreeSnapshot::Stash(_)),
            "dirty tree yields a stash ref"
        );
        std::fs::write(&f, "v3").expect("write v3");

        let restored = h.undo(&[msg("m"), msg("m2")], dir.path());
        assert_eq!(restored.expect("undo").files_note, None);
        let content = std::fs::read_to_string(&f).unwrap_or_default();
        assert_eq!(content, "v2", "undo checks out the pre-turn tree");
    }

    #[test]
    fn stash_slot_wait_survives_a_poisoned_lock() {
        let slot = StashSlot::done(TreeSnapshot::Stash("kept".into()));
        let held = slot.clone();
        let poisoned = std::thread::spawn(move || {
            let _guard = held.0.lock().expect("first lock");
            panic!("poison the slot");
        })
        .join();
        assert!(poisoned.is_err());
        assert_eq!(slot.wait(), TreeSnapshot::Stash("kept".into()));
    }

    #[test]
    fn stash_slot_waits_once_and_keeps_the_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = SessionHistory::new();
        let slot = h.push_before_turn(&[msg("m")], dir.path());
        // Outside a repo the background `git` yields no ref; a second wait
        // reads the stored result instead of joining again.
        assert_eq!(slot.wait(), TreeSnapshot::NoRepo);
        assert_eq!(slot.wait(), TreeSnapshot::NoRepo);
        let done = StashSlot::done(TreeSnapshot::Stash("abc".into()));
        assert_eq!(done.clone().wait(), TreeSnapshot::Stash("abc".into()));
        assert_eq!(done.wait(), TreeSnapshot::Stash("abc".into()));
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
        assert_eq!(restored.messages[0].content.as_text(), Some("after"));
        assert_eq!(restored.files_note, None);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
    }

    #[test]
    fn git_helpers_handle_disappearing_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir);
        assert_eq!(git_stash_create(&path), TreeSnapshot::NoRepo);
        assert!(git_changed_files(&path).is_empty());

        let mut history = SessionHistory::new();
        history.push_before_turn(&[msg("before")], &path);
        assert_eq!(
            history.undo(&[msg("after")], &path).unwrap().messages.len(),
            1
        );
        assert_eq!(
            history
                .redo(&[msg("before")], &path)
                .unwrap()
                .messages
                .len(),
            1
        );
    }

    #[test]
    fn stash_creation_failure_in_damaged_repo_is_a_failure() {
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
        assert!(matches!(
            git_stash_create(dir.path()),
            TreeSnapshot::Failed(_)
        ));
    }

    fn committed_repo() -> tempfile::TempDir {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().unwrap();
        assert!(run_git(dir.path(), &["init", "-q"]));
        std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
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
        dir
    }

    /// An editor's background `git status` holds `index.lock` as the turn
    /// starts. Undo used to read the failed snapshot as a clean tree and run
    /// `git checkout -- .`, wiping the user's uncommitted edit.
    #[test]
    fn undo_keeps_uncommitted_work_when_the_snapshot_failed() {
        let dir = committed_repo();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "user wip").unwrap();
        let lock = dir.path().join(".git/index.lock");
        std::fs::write(&lock, "").unwrap();

        let mut h = SessionHistory::new();
        let snapshot = h.push_before_turn(&[msg("m")], dir.path()).wait();
        assert!(matches!(snapshot, TreeSnapshot::Failed(_)), "{snapshot:?}");
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(&file, "user wip + turn edit").unwrap();

        let undone = h.undo(&[msg("m"), msg("m2")], dir.path()).unwrap();
        assert_eq!(texts(&undone.messages), ["m"]);
        let note = undone.files_note.expect("undo says files stayed");
        assert!(note.starts_with("files left as they are"), "{note}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "user wip + turn edit",
            "nothing was checked out over the user's work"
        );
    }

    /// `git stash create` needs a first commit; before it, the fallback
    /// `git checkout -- .` reset files to the index.
    #[test]
    fn undo_keeps_files_in_a_repo_without_commits() {
        assert!(git_available(), "git is required by the session test suite");
        let dir = tempfile::tempdir().unwrap();
        assert!(run_git(dir.path(), &["init", "-q"]));
        let file = dir.path().join("b.txt");
        std::fs::write(&file, "staged").unwrap();
        assert!(run_git(dir.path(), &["add", "b.txt"]));
        std::fs::write(&file, "staged + unstaged").unwrap();

        let mut h = SessionHistory::new();
        h.push_before_turn(&[msg("m")], dir.path()).wait();
        let undone = h.undo(&[msg("m"), msg("m2")], dir.path()).unwrap();
        assert!(undone.files_note.is_some());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "staged + unstaged");
    }

    #[test]
    fn clean_tree_snapshot_restores_head_on_undo() {
        let dir = committed_repo();
        let file = dir.path().join("a.txt");
        let mut h = SessionHistory::new();
        let snapshot = h.push_before_turn(&[msg("m")], dir.path()).wait();
        assert_eq!(snapshot, TreeSnapshot::Clean);
        std::fs::write(&file, "turn edit").unwrap();
        let undone = h.undo(&[msg("m"), msg("m2")], dir.path()).unwrap();
        assert_eq!(undone.files_note, None);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
    }

    #[test]
    fn failed_checkout_is_reported_not_swallowed() {
        let dir = committed_repo();
        let missing = TreeSnapshot::Stash("0".repeat(40));
        let note = restore_tree(&missing, dir.path()).expect("checkout fails");
        assert!(
            note.starts_with("files not restored: git checkout failed ("),
            "{note}"
        );
    }

    #[test]
    fn panicked_snapshot_thread_counts_as_failed() {
        let slot = StashSlot::with(Stash::Running(std::thread::spawn(|| -> TreeSnapshot {
            panic!("git thread died")
        })));
        assert!(matches!(slot.wait(), TreeSnapshot::Failed(_)));
    }
}
