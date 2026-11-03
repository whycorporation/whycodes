//! Whole-working-tree git snapshots for `/undo` and `/redo`.
//!
//! `git stash create` sees tracked files only: a file a turn created stayed
//! after `/undo`, and `/redo` could not bring one back. A snapshot here is
//! written through a throwaway index instead: a copy of the real index (so
//! git reuses its stat cache), `git add -A`, `git write-tree`. That records
//! tracked and untracked files alike and skips ignored ones.
//!
//! A snapshot writes only loose objects. HEAD, refs, `refs/stash` (the
//! user's stash list) and the real index stay untouched, so a held
//! `index.lock` does not stop it. Restoring writes only the paths that
//! differ between two snapshots, and refuses when HEAD moved or a merge,
//! rebase, cherry-pick or revert is in progress.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// `--attr-source` at the empty tree: no `.gitattributes` rules apply.
const NO_ATTRS_SHA1: &str = "--attr-source=4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const NO_ATTRS_SHA256: &str =
    "--attr-source=6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

/// Git state files that mean an operation is half done, with its name.
const BUSY_MARKERS: [(&str, &str); 5] = [
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase"),
];

/// The working tree as recorded before a turn (or before an undo/redo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeSnapshot {
    /// The tree, staging area and HEAD at that moment.
    Git(GitTree),
    /// Not inside a git work tree; undo leaves files alone.
    NoRepo,
    /// A repository, but git could not take the snapshot (unmerged paths,
    /// a broken index). Undo must leave files alone: nothing records what
    /// they held.
    Failed(String),
}

/// Trees git wrote for one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTree {
    /// Commit HEAD named, `None` before the first commit.
    head: Option<String>,
    /// The staging area.
    index: String,
    /// The working tree: tracked and untracked files, ignored ones left out.
    work: String,
    /// A merge, rebase, cherry-pick or revert was half done.
    busy: Option<&'static str>,
    /// `--attr-source` option for this repository's hash (see [`raw_flags`]).
    no_attrs: &'static str,
}

/// Record the working tree of `dir` without changing it.
///
/// Each `git` spawn costs ~50 ms on Windows and this runs before every
/// turn, so independent calls run side by side: the untracked listing with
/// the repository probe, then the staging-area tree with `add` + the
/// working-tree tree.
pub(crate) fn snapshot(dir: &Path) -> TreeSnapshot {
    std::thread::scope(|s| {
        let untracked = s.spawn(|| git(dir, &UNTRACKED, None, None));
        let Some(probe) = probe(dir) else {
            return TreeSnapshot::NoRepo;
        };
        let untracked = untracked
            .join()
            .unwrap_or(Err(String::from("ls-files thread panicked")));
        match write_trees(dir, &probe, untracked) {
            Ok(tree) => TreeSnapshot::Git(tree),
            Err(why) => TreeSnapshot::Failed(why),
        }
    })
}

/// Untracked, not ignored files under `dir`, NUL-separated.
const UNTRACKED: [&str; 4] = ["ls-files", "-z", "--others", "--exclude-standard"];

/// Object format, index path, busy markers and HEAD, one per line; `None`
/// outside a work tree.
fn probe(dir: &Path) -> Option<String> {
    let mut args = vec!["rev-parse", "--show-object-format", "--git-path", "index"];
    for (marker, _) in BUSY_MARKERS {
        args.extend(["--git-path", marker]);
    }
    // One spawn names HEAD too. Before the first commit `HEAD` is fatal, so
    // ask again without it; outside a work tree both fail.
    args.push("HEAD");
    let probe = git(dir, &args, None, None).or_else(|_unborn| {
        args.pop();
        git(dir, &args, None, None)
    });
    probe.ok()
}

/// Write the staging area and the working tree as trees, each from its own
/// copy of the index.
fn write_trees(
    dir: &Path,
    probe: &str,
    untracked: Result<String, String>,
) -> Result<GitTree, String> {
    let lines: Vec<&str> = probe.lines().collect();
    let no_attrs = match lines.first() {
        Some(&"sha256") => NO_ATTRS_SHA256,
        _ => NO_ATTRS_SHA1,
    };
    let real_index = dir.join(lines.get(1).unwrap_or(&""));
    let busy = BUSY_MARKERS
        .iter()
        .zip(lines.iter().skip(2))
        .find(|(_, path)| dir.join(path).exists())
        .map(|((_, name), _)| *name);
    let head = lines.get(2 + BUSY_MARKERS.len()).map(ToString::to_string);

    let for_index = ScratchIndex::copy_of(&real_index)?;
    let for_work = ScratchIndex::copy_of(&real_index)?;
    let (index, work) = std::thread::scope(|s| {
        let index = s.spawn(|| git(dir, &["write-tree"], Some(&for_index.0), None));
        let work = untracked.and_then(|list| write_work_tree(dir, &for_work.0, no_attrs, &list));
        (index.join(), work)
    });
    let index = index.unwrap_or(Err(String::from("write-tree thread panicked")))?;
    let work = work?;
    Ok(GitTree {
        head,
        index,
        work,
        busy,
        no_attrs,
    })
}

/// Untracked files above this size stay out of snapshots. The scratch index
/// starts from the real one, which does not list untracked files, so git
/// would hash such a file again before every turn and keep a copy in
/// `.git/objects`. `/undo` leaves these files as they are.
pub(crate) const MAX_UNTRACKED_BYTES: u64 = 20 * 1024 * 1024;

/// `git add -A` into `index`, minus oversized `untracked` files; the tree id.
fn write_work_tree(
    dir: &Path,
    index: &Path,
    no_attrs: &'static str,
    untracked: &str,
) -> Result<String, String> {
    let skip: Vec<String> = untracked
        .split('\0')
        .filter(|rel| too_large(&dir.join(rel)))
        .map(|rel| format!(":(exclude,literal){rel}"))
        .collect();
    let mut add = raw_flags(no_attrs);
    add.extend(["add", "-A", "--", "."]);
    add.extend(skip.iter().map(String::as_str));
    git(dir, &add, Some(index), None)?;
    git(dir, &["write-tree"], Some(index), None)
}

fn too_large(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > MAX_UNTRACKED_BYTES)
}

/// Put the files of `dir` back to `target`; `now` is the tree they hold.
/// Returns why they were left as they are, or only partly restored.
pub(crate) fn restore_files(
    target: &TreeSnapshot,
    now: &TreeSnapshot,
    dir: &Path,
) -> Option<String> {
    match (target, now) {
        (TreeSnapshot::NoRepo, _) => None,
        (TreeSnapshot::Failed(why), _) => Some(format!(
            "files left as they are: git could not snapshot them at that point ({why})"
        )),
        (TreeSnapshot::Git(_), TreeSnapshot::NoRepo) => {
            Some("files left as they are: the project is no longer a git work tree".into())
        }
        (TreeSnapshot::Git(_), TreeSnapshot::Failed(why)) => Some(format!(
            "files left as they are: git could not snapshot the current files ({why})"
        )),
        (TreeSnapshot::Git(target), TreeSnapshot::Git(now)) => restore(target, now, dir).err(),
    }
}

fn restore(target: &GitTree, now: &GitTree, dir: &Path) -> Result<(), String> {
    if let Some(op) = now.busy {
        return Err(format!(
            "files left as they are: a {op} is in progress; finish or abort it first"
        ));
    }
    if target.head != now.head {
        return Err("files left as they are: HEAD moved since then \
                    (commit, checkout, merge, rebase or reset)"
            .into());
    }
    let left = |e: String| format!("files left as they are: {e}");
    let changes = changed_paths(dir, &target.work, &now.work).map_err(left)?;
    let staged = git(
        dir,
        &[
            "diff-tree",
            "-r",
            "--name-only",
            "--relative",
            &target.index,
            &now.index,
        ],
        None,
        None,
    )
    .map_err(left)?;
    if !staged.is_empty() {
        let source = format!("--source={}", target.index);
        git(
            dir,
            &["restore", "--staged", &source, "--", "."],
            None,
            None,
        )
        .map_err(|e| left(format!("could not restore the staging area ({e})")))?;
    }

    let partly = |e: String| format!("files only partly restored: {e}");
    if !changes.write.is_empty() {
        let scratch = ScratchIndex::new();
        git(dir, &["read-tree", &target.work], Some(&scratch.0), None).map_err(partly)?;
        let mut checkout = raw_flags(target.no_attrs);
        checkout.extend(["checkout-index", "-f", "-z", "--stdin"]);
        let list: String = changes.write.iter().map(|p| format!("{p}\0")).collect();
        git(dir, &checkout, Some(&scratch.0), Some(list.as_bytes())).map_err(partly)?;
    }
    for rel in &changes.delete {
        remove_created(dir, rel).map_err(partly)?;
    }
    Ok(())
}

/// Paths to write back from the target, and paths it did not have.
#[derive(Debug, Default, PartialEq, Eq)]
struct Changes {
    write: Vec<String>,
    delete: Vec<String>,
}

fn changed_paths(dir: &Path, target: &str, now: &str) -> Result<Changes, String> {
    let raw = git(
        dir,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--relative",
            target,
            now,
        ],
        None,
        None,
    )?;
    Ok(parse_raw_diff(&raw))
}

/// Parse `diff-tree -r -z` raw records: `:old new oldsha newsha S\0path\0`.
/// Submodule entries (mode 160000) are skipped: a nested repository is
/// not a file to write or delete.
fn parse_raw_diff(raw: &str) -> Changes {
    let fields: Vec<&str> = raw.split('\0').filter(|f| !f.is_empty()).collect();
    let mut changes = Changes::default();
    for [meta, path] in fields.as_chunks::<2>().0 {
        let mut meta = meta.split(' ');
        let old_mode = meta.next().unwrap_or_default();
        let new_mode = meta.next().unwrap_or_default();
        let status = meta.next_back().unwrap_or_default();
        let path = path.to_string();
        if status == "A" {
            if new_mode != "160000" {
                changes.delete.push(path);
            }
        } else if old_mode != ":160000" {
            changes.write.push(path);
        }
    }
    changes
}

/// Remove a file the target did not have, then any directories that only
/// held it, up to the project root.
fn remove_created(dir: &Path, rel: &str) -> Result<(), String> {
    let path = dir.join(rel);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove {rel}: {e}")),
    }
    let mut parent = path.parent();
    while let Some(p) = parent.filter(|p| *p != dir && p.starts_with(dir)) {
        if std::fs::remove_dir(p).is_err() {
            break; // not empty: it holds other files
        }
        parent = p.parent();
    }
    Ok(())
}

/// Options that store and write file bytes as they are: no `autocrlf`, no
/// `.gitattributes` eol or filter rules (the attribute source is the empty
/// tree). Without this, an LF file in a `core.autocrlf=true` checkout came
/// back as CRLF. Git before 2.40 has no `--attr-source`; it keeps the rest.
fn raw_flags(no_attrs: &'static str) -> Vec<&'static str> {
    flags_for(no_attrs, attr_source_supported())
}

fn flags_for(no_attrs: &'static str, attr_source: bool) -> Vec<&'static str> {
    let mut flags = vec!["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"];
    if attr_source {
        flags.insert(0, no_attrs);
    }
    flags
}

fn attr_source_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let args = [NO_ATTRS_SHA1, "version"];
        git(&std::env::temp_dir(), &args, None, None).is_ok()
    })
}

/// A private index file in the temp dir, removed on drop.
struct ScratchIndex(PathBuf);

impl ScratchIndex {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("whycodes-undo-{}-{n}.index", std::process::id());
        Self(std::env::temp_dir().join(name))
    }

    /// A scratch copy of `real`; empty (no file) when the repo has none yet.
    fn copy_of(real: &Path) -> Result<Self, String> {
        let scratch = Self::new();
        if real.exists() {
            std::fs::copy(real, &scratch.0).map_err(|e| format!("copy index: {e}"))?;
        }
        Ok(scratch)
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        for path in [self.0.clone(), self.0.with_extension("index.lock")] {
            if let Err(e) = std::fs::remove_file(&path) {
                note_cleanup(&path, &e);
            }
        }
    }
}

/// A scratch index that did not exist is fine; anything else is logged.
fn note_cleanup(path: &Path, e: &std::io::Error) {
    if e.kind() != std::io::ErrorKind::NotFound {
        tracing::debug!(path = %path.display(), error = %e, "scratch index cleanup");
    }
}

/// Run `git` in `dir`: stdout without trailing whitespace (a leading space
/// in a `-z` path list survives), or the first stderr line on failure.
/// `index` sets `GIT_INDEX_FILE`; `stdin` is fed to the process.
pub(crate) fn git(
    dir: &Path,
    args: &[&str],
    index: Option<&Path>,
    stdin: Option<&[u8]>,
) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let mut child = cmd.spawn().map_err(io_error)?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes).map_err(io_error)?;
    }
    let output = child.wait_with_output().map_err(io_error)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first = stderr.lines().next().unwrap_or("git failed").trim();
    Err(first.to_string())
}

fn io_error(e: std::io::Error) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@example.com"];
        all.extend(args);
        let out = git(dir, &all, None, None);
        assert!(out.is_ok(), "git {args:?}: {out:?}");
        out.unwrap_or_default()
    }

    #[test]
    fn a_turn_that_only_created_files_is_undone_by_deleting_them() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q"]);
        std::fs::write(d.join("kept.txt"), "kept").unwrap();
        let before = snapshot(d);
        std::fs::write(d.join("made.txt"), "made").unwrap();
        assert_eq!(restore_files(&before, &snapshot(d), d), None);
        assert!(!d.join("made.txt").exists());
        assert_eq!(std::fs::read_to_string(d.join("kept.txt")).unwrap(), "kept");
    }

    #[test]
    fn raw_diff_records_split_into_writes_and_deletes() {
        let z = "0".repeat(40);
        let raw = format!(
            ":100644 100644 {z} {z} M\0mod.txt\0\
             :000000 100644 {z} {z} A\0new.txt\0\
             :100644 000000 {z} {z} D\0gone.txt\0\
             :100644 120000 {z} {z} T\0now-link\0\
             :000000 160000 {z} {z} A\0new-submodule\0\
             :160000 160000 {z} {z} M\0bumped-submodule\0"
        );
        assert_eq!(
            parse_raw_diff(&raw),
            Changes {
                write: vec!["mod.txt".into(), "gone.txt".into(), "now-link".into()],
                delete: vec!["new.txt".into()],
            }
        );
        assert_eq!(parse_raw_diff(""), Changes::default());
    }

    #[test]
    fn flags_turn_conversion_off_with_and_without_attr_source() {
        assert_eq!(
            flags_for(NO_ATTRS_SHA1, true),
            [
                NO_ATTRS_SHA1,
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.safecrlf=false"
            ]
        );
        assert!(!flags_for(NO_ATTRS_SHA1, false).contains(&NO_ATTRS_SHA1));
        assert_eq!(raw_flags(NO_ATTRS_SHA1)[0], NO_ATTRS_SHA1, "git 2.40+ here");
    }

    #[test]
    fn restore_explains_every_snapshot_it_cannot_use() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let git_tree = TreeSnapshot::Git(GitTree {
            head: None,
            index: String::new(),
            work: String::new(),
            busy: None,
            no_attrs: NO_ATTRS_SHA1,
        });
        let failed = TreeSnapshot::Failed("boom".into());
        assert_eq!(restore_files(&TreeSnapshot::NoRepo, &git_tree, d), None);
        let note = restore_files(&failed, &git_tree, d).unwrap();
        assert!(note.contains("at that point (boom)"), "{note}");
        let note = restore_files(&git_tree, &TreeSnapshot::NoRepo, d).unwrap();
        assert!(note.contains("no longer a git work tree"), "{note}");
        let note = restore_files(&git_tree, &failed, d).unwrap();
        assert!(note.contains("the current files (boom)"), "{note}");
    }

    #[test]
    fn a_bad_tree_id_is_reported_before_any_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q"]);
        std::fs::write(d.join("f.txt"), "now").unwrap();
        let now = snapshot(d);
        let mut gone = now.clone();
        if let TreeSnapshot::Git(tree) = &mut gone {
            tree.work = "0".repeat(40);
        }
        let note = restore_files(&gone, &now, d).unwrap();
        assert!(note.starts_with("files left as they are: "), "{note}");
        assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), "now");
    }

    /// A dataset or download the user keeps untracked is not hashed into
    /// `.git/objects` before every turn, and undo leaves it alone.
    #[test]
    fn oversized_untracked_files_stay_out_of_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q"]);
        std::fs::write(d.join("f.txt"), "before").unwrap();
        let big = std::fs::File::create(d.join("data.bin")).unwrap();
        big.set_len(MAX_UNTRACKED_BYTES + 1).unwrap();
        drop(big);
        let before = snapshot(d);
        assert!(matches!(before, TreeSnapshot::Git(_)), "{before:?}");
        let blob = git_ok(d, &["hash-object", "data.bin"]);
        let stored = git(d, &["cat-file", "-e", &blob], None, None);
        assert!(stored.is_err(), "no copy in .git/objects");

        std::fs::write(d.join("f.txt"), "after").unwrap();
        assert_eq!(restore_files(&before, &snapshot(d), d), None);
        assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), "before");
        let size = std::fs::metadata(d.join("data.bin")).unwrap().len();
        assert_eq!(size, MAX_UNTRACKED_BYTES + 1, "left as it was");
    }

    #[test]
    fn a_name_that_starts_with_a_space_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q"]);
        let before = snapshot(d);
        std::fs::write(d.join(" leading.txt"), "made").unwrap();
        assert_eq!(restore_files(&before, &snapshot(d), d), None);
        assert!(!d.join(" leading.txt").exists());
    }

    /// A blob the target needs is gone (pruned): what was written stays
    /// written, and the note says the restore stopped part way.
    #[test]
    fn a_missing_blob_is_reported_as_a_partial_restore() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q"]);
        std::fs::write(d.join("f.txt"), "only in the snapshot\n").unwrap();
        let before = snapshot(d);
        let blob = git_ok(d, &["hash-object", "f.txt"]);
        let (fan, rest) = blob.split_at(2);
        std::fs::remove_file(d.join(".git/objects").join(fan).join(rest)).unwrap();
        std::fs::write(d.join("f.txt"), "changed\n").unwrap();
        let note = restore_files(&before, &snapshot(d), d).unwrap();
        assert!(note.starts_with("files only partly restored: "), "{note}");
    }

    #[test]
    fn sha256_repositories_snapshot_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_ok(d, &["init", "-q", "--object-format=sha256"]);
        std::fs::write(d.join("f.txt"), "before\n").unwrap();
        let before = snapshot(d);
        assert!(
            matches!(&before, TreeSnapshot::Git(t) if t.no_attrs == NO_ATTRS_SHA256),
            "{before:?}"
        );
        std::fs::write(d.join("f.txt"), "after\n").unwrap();
        std::fs::write(d.join("g.txt"), "new\n").unwrap();
        assert_eq!(restore_files(&before, &snapshot(d), d), None);
        assert_eq!(
            std::fs::read_to_string(d.join("f.txt")).unwrap(),
            "before\n"
        );
        assert!(!d.join("g.txt").exists());
    }

    #[test]
    fn removing_a_created_file_prunes_only_emptied_directories() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("a/b/c")).unwrap();
        std::fs::write(d.join("a/keep.txt"), "x").unwrap();
        std::fs::write(d.join("a/b/c/made.txt"), "x").unwrap();
        assert_eq!(remove_created(d, "a/b/c/made.txt"), Ok(()));
        assert!(!d.join("a/b").exists(), "emptied directories go");
        assert!(
            d.join("a/keep.txt").exists(),
            "a directory with files stays"
        );
        assert_eq!(remove_created(d, "already/gone.txt"), Ok(()));
        // A directory where a file was expected is an error, not a skip.
        std::fs::create_dir(d.join("dir.txt")).unwrap();
        let err = remove_created(d, "dir.txt").unwrap_err();
        assert!(err.starts_with("remove dir.txt: "), "{err}");
    }

    #[test]
    fn scratch_index_cleans_up_after_itself() {
        let scratch = ScratchIndex::new();
        let path = scratch.0.clone();
        std::fs::write(&path, "x").unwrap();
        std::fs::write(path.with_extension("index.lock"), "x").unwrap();
        drop(scratch);
        assert!(!path.exists());
        assert!(!path.with_extension("index.lock").exists());
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        note_cleanup(&path, &missing);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        note_cleanup(&path, &denied);
    }

    #[test]
    fn git_runner_feeds_stdin_and_reports_the_first_stderr_line() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let id = git(d, &["hash-object", "--stdin"], None, Some(b"hello\n")).unwrap();
        assert_eq!(id, "ce013625030ba8dba906f756967f9e9ca394464a");
        let err = git(d, &["no-such-command"], None, None).unwrap_err();
        assert!(err.contains("no-such-command"), "{err}");
        let missing = d.join("missing");
        assert!(git(&missing, &["version"], None, None).is_err());
    }
}
