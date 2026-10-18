//! The project scratch directory as seen by shell commands.
//!
//! Every command the agent's shell tools start gets `WHYCODES_SCRATCH`, the
//! absolute `.whycodes/scratch/` of its working directory, so a script can
//! keep a file the user or a later step will read without guessing a path.
//! `TMPDIR` and friends are left alone: build and test intermediates stay in
//! the OS temp dir.
//!
//! The first command for a scratch dir in a process also prunes it on a
//! background thread (age limit, then size cap).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Name of the variable every agent shell command gets.
pub const SCRATCH_ENV: &str = "WHYCODES_SCRATCH";

/// Scratch entries untouched for this long are removed.
pub const SCRATCH_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// After the age pass, the oldest entries go until the dir is under this.
pub const SCRATCH_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// Scratch dirs already pruned by this process.
static PRUNED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Environment for a shell started in `working_dir`. Creates the scratch dir
/// when the working dir exists; a failure is logged and the command still
/// runs.
pub(crate) fn scratch_env(working_dir: &Path) -> Vec<(String, OsString)> {
    let dir = whycodes_core::project_scratch_dir(&absolute(working_dir));
    if working_dir.is_dir() {
        match std::fs::create_dir_all(&dir) {
            Ok(()) => prune_once(&dir),
            Err(e) => not_created(&dir, &e),
        }
    }
    vec![(SCRATCH_ENV.to_string(), dir.into_os_string())]
}

fn not_created(dir: &Path, e: &std::io::Error) {
    tracing::debug!(error = %e, path = %dir.display(), "scratch dir not created");
}

fn absolute(path: &Path) -> PathBuf {
    match std::path::absolute(path) {
        Ok(abs) => abs,
        Err(e) => {
            tracing::debug!(error = %e, "scratch path left relative");
            path.to_path_buf()
        }
    }
}

/// Prune `dir` on a background thread the first time this process sees it.
fn prune_once(dir: &Path) {
    let mut pruned = PRUNED.lock().unwrap_or_else(|e| e.into_inner());
    if pruned.insert(dir.to_path_buf()) {
        let dir = dir.to_path_buf();
        std::thread::spawn(move || prune_now(&dir));
    }
}

fn prune_now(dir: &Path) -> usize {
    prune_scratch_dir(dir, SystemTime::now(), SCRATCH_MAX_AGE, SCRATCH_MAX_BYTES)
}

/// One top-level scratch entry: its newest mtime and file bytes, recursive.
pub(crate) struct Entry {
    pub path: PathBuf,
    pub is_file: bool,
    pub newest: SystemTime,
    pub bytes: u64,
}

/// Remove top-level entries of `dir` whose newest mtime is older than
/// `max_age`, then the oldest of the rest until their total is at most
/// `max_bytes`. Symlinks count as themselves: never followed, never
/// measured through, and removing one leaves its target alone. Returns how
/// many entries were removed.
pub(crate) fn prune_scratch_dir(
    dir: &Path,
    now: SystemTime,
    max_age: Duration,
    max_bytes: u64,
) -> usize {
    let mut entries: Vec<Entry> = list(dir)
        .into_iter()
        .map(|(path, meta)| measure(path, &meta))
        .collect();
    let cutoff = now.checked_sub(max_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let before = entries.len();
    entries.retain(|e| !(e.newest < cutoff && remove_entry(e)));
    let mut removed = before - entries.len();
    entries.sort_by_key(|e| e.newest);
    let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
    for e in &entries {
        if total <= max_bytes {
            break;
        }
        if remove_entry(e) {
            total -= e.bytes;
            removed += 1;
        }
    }
    removed
}

/// Children of `dir` with their own (not followed) metadata.
fn list(dir: &Path) -> Vec<(PathBuf, Metadata)> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) => {
            tracing::debug!(error = %e, path = %dir.display(), "scratch prune skipped");
            return Vec::new();
        }
    };
    read.filter_map(Result::ok)
        .filter_map(|d| d.metadata().map(|m| (d.path(), m)).ok())
        .collect()
}

pub(crate) fn measure(path: PathBuf, meta: &Metadata) -> Entry {
    let mut newest = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let mut bytes = if meta.is_file() { meta.len() } else { 0 };
    if meta.is_dir() {
        for (child, child_meta) in list(&path) {
            let inner = measure(child, &child_meta);
            newest = newest.max(inner.newest);
            bytes += inner.bytes;
        }
    }
    Entry {
        path,
        is_file: meta.is_file(),
        newest,
        bytes,
    }
}

/// Files go with `remove_file`; directories and symlinks with
/// `remove_dir_all`, which deletes a symlink itself and never follows it.
pub(crate) fn remove_entry(e: &Entry) -> bool {
    let result = if e.is_file {
        std::fs::remove_file(&e.path)
    } else {
        std::fs::remove_dir_all(&e.path)
    };
    match result {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(error = %err, path = %e.path.display(), "scratch entry kept");
            false
        }
    }
}

#[cfg(test)]
#[path = "scratch_tests.rs"]
mod tests;
