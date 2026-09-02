//! Project-local `serve` lock (pid, port, started_at).
//!
//! A second `whycodes serve` should not surface a raw `Address already in use`.
//! Stale locks (dead PID, 24h age, clock skew) are removed; a live holder is
//! either taken over (TTY) or reported (CI / `--no-takeover`).

use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// PID recycle window: a lock older than this is stale even if the pid exists.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// `started_at` more than this in the future is clock skew → treat as stale.
const CLOCK_SKEW: Duration = Duration::from_secs(5 * 60);

const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ServeLock {
    pub pid: u32,
    pub port: u16,
    /// Unix seconds.
    pub started_at: u64,
    pub interactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
}

#[derive(Debug)]
pub(crate) enum LockDecision {
    /// No lock, or we removed a stale one.
    Free,
    /// Holder is alive; caller must not bind.
    Blocked(ServeLock),
    /// User asked to take over; holder should be signalled.
    Takeover(ServeLock),
}

/// `.whycodes/serve.lock` under the project working directory.
pub(crate) fn lock_path(project_dir: &Path) -> PathBuf {
    whycodes_core::paths::project_dir(project_dir).join("serve.lock")
}

pub(crate) fn read_lock(path: &Path) -> Option<ServeLock> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn write_lock(path: &Path, lock: &ServeLock) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("lock.tmp");
    let json = serde_json::to_vec_pretty(lock)
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn remove_lock(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != ErrorKind::NotFound
    {
        tracing::debug!(error = %err, path = %path.display(), "serve lock remove failed");
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn is_stale(lock: &ServeLock, now: u64, pid_live: impl Fn(u32) -> PidProbe) -> bool {
    match pid_live(lock.pid) {
        PidProbe::Dead => true,
        PidProbe::Alive => {
            if lock.started_at > now.saturating_add(CLOCK_SKEW.as_secs()) {
                return true;
            }
            now.saturating_sub(lock.started_at) > MAX_AGE.as_secs()
        }
        PidProbe::Denied => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PidProbe {
    Alive,
    Dead,
    /// `EPERM` / access denied — treat as alive, do not clobber.
    Denied,
}

pub(crate) fn pid_alive(pid: u32) -> PidProbe {
    probe_pid(pid)
}

#[cfg(unix)]
fn probe_pid(pid: u32) -> PidProbe {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let rc = unsafe { kill(pid as i32, 0) };
    if rc == 0 {
        return PidProbe::Alive;
    }
    let err = std::io::Error::last_os_error();
    // EPERM is 1 on Linux/macOS/BSD. Treat as alive — do not clobber a
    // lock we cannot signal.
    const EPERM: i32 = 1;
    match err.raw_os_error() {
        Some(EPERM) => PidProbe::Denied,
        _ => PidProbe::Dead,
    }
}

#[cfg(windows)]
fn probe_pid(pid: u32) -> PidProbe {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return PidProbe::Dead;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        if ok && code == STILL_ACTIVE {
            PidProbe::Alive
        } else {
            PidProbe::Dead
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn probe_pid(_pid: u32) -> PidProbe {
    PidProbe::Dead
}

pub(crate) struct ServeLockGuard {
    path: PathBuf,
}

impl ServeLockGuard {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ServeLockGuard {
    fn drop(&mut self) {
        remove_lock(&self.path);
    }
}

/// Inspect / replace the lock. May prompt on a TTY.
pub(crate) fn acquire_lock(
    project_dir: &Path,
    port: u16,
    no_takeover: bool,
) -> anyhow::Result<(ServeLockGuard, Option<ServeLock>)> {
    let path = lock_path(project_dir);
    let decision = decide_lock(&path, port, no_takeover, stdin_can_prompt())?;
    match decision {
        LockDecision::Free => {
            let lock = current_lock(port);
            write_lock(&path, &lock)?;
            Ok((ServeLockGuard { path }, None))
        }
        LockDecision::Blocked(holder) => {
            anyhow::bail!(blocked_message(&holder));
        }
        LockDecision::Takeover(holder) => Ok((ServeLockGuard { path }, Some(holder))),
    }
}

pub(crate) fn commit_lock(guard: &ServeLockGuard, port: u16) -> std::io::Result<()> {
    write_lock(guard.path(), &current_lock(port))
}

fn current_lock(port: u16) -> ServeLock {
    ServeLock {
        pid: std::process::id(),
        port,
        started_at: now_secs(),
        interactive: stdin_can_prompt(),
        parent_pid: None,
    }
}

fn stdin_can_prompt() -> bool {
    std::io::stdin().is_terminal() && std::env::var_os("CI").is_none()
}

pub(crate) fn decide_lock(
    path: &Path,
    port: u16,
    no_takeover: bool,
    can_prompt: bool,
) -> anyhow::Result<LockDecision> {
    let Some(holder) = read_lock(path) else {
        return Ok(LockDecision::Free);
    };
    if is_stale(&holder, now_secs(), pid_alive) {
        remove_lock(path);
        return Ok(LockDecision::Free);
    }
    if no_takeover || !can_prompt {
        return Ok(LockDecision::Blocked(holder));
    }
    prompt_takeover(path, port, holder)
}

fn prompt_takeover(path: &Path, port: u16, holder: ServeLock) -> anyhow::Result<LockDecision> {
    let items = ["Take over", "Abort", "Start anyway"];
    let choice = dialoguer::Select::new()
        .with_prompt(format!(
            "whycodes serve already running (pid {}, http://127.0.0.1:{})",
            holder.pid, holder.port
        ))
        .items(items)
        .default(0)
        .interact();
    match choice {
        Ok(0) => Ok(LockDecision::Takeover(holder)),
        Ok(2) => {
            if holder.port == port {
                anyhow::bail!(
                    "port {} is still held by pid {}. Choose Take over, or pass a different port.",
                    port,
                    holder.pid
                );
            }
            // Different port: replace the lock with ours after bind (caller writes).
            remove_lock(path);
            Ok(LockDecision::Free)
        }
        _ => anyhow::bail!("aborted: serve already running (pid {})", holder.pid),
    }
}

pub(crate) fn blocked_message(holder: &ServeLock) -> String {
    format!(
        "whycodes serve already running (pid {}, http://127.0.0.1:{})\n\
         Take over from a TTY, or stop that process. Scripts: this is a non-zero exit.",
        holder.pid, holder.port
    )
}

pub(crate) fn connect_hint(project_dir: &Path) -> Option<String> {
    let lock = read_lock(&lock_path(project_dir))?;
    if is_stale(&lock, now_secs(), pid_alive) {
        return None;
    }
    Some(format!(
        "A serve lock exists for pid {} on port {} (http://127.0.0.1:{}).\n\
         Connect:  whycodes connect 127.0.0.1:{}\n\
         Take over from a TTY: whycodes serve",
        lock.pid, lock.port, lock.port, lock.port
    ))
}

pub(crate) fn signal_term(pid: u32) -> std::io::Result<()> {
    signal_pid(pid, SIGTERM)
}

pub(crate) fn signal_kill(pid: u32) -> std::io::Result<()> {
    signal_pid(pid, SIGKILL)
}

#[cfg(unix)]
fn signal_pid(pid: u32, sig: i32) -> std::io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let rc = unsafe { kill(pid as i32, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn signal_pid(pid: u32, _sig: i32) -> std::io::Result<()> {
    const PROCESS_TERMINATE: u32 = 0x0001;
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn TerminateProcess(handle: *mut std::ffi::c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let ok = TerminateProcess(handle, 1) != 0;
        CloseHandle(handle);
        if ok {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn signal_pid(_pid: u32, _sig: i32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "cannot signal process on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_when_pid_dead() {
        let lock = ServeLock {
            pid: 1,
            port: 3030,
            started_at: now_secs(),
            interactive: false,
            parent_pid: None,
        };
        assert!(is_stale(&lock, now_secs(), |_| PidProbe::Dead));
        assert!(!is_stale(&lock, now_secs(), |_| PidProbe::Alive));
        assert!(!is_stale(&lock, now_secs(), |_| PidProbe::Denied));
    }

    #[test]
    fn stale_when_older_than_max_age() {
        let lock = ServeLock {
            pid: 1,
            port: 3030,
            started_at: 10,
            interactive: false,
            parent_pid: None,
        };
        let now = 10 + MAX_AGE.as_secs() + 1;
        assert!(is_stale(&lock, now, |_| PidProbe::Alive));
    }

    #[test]
    fn stale_when_started_in_the_future() {
        let now = 1_700_000_000;
        let lock = ServeLock {
            pid: 1,
            port: 3030,
            started_at: now + CLOCK_SKEW.as_secs() + 1,
            interactive: false,
            parent_pid: None,
        };
        assert!(is_stale(&lock, now, |_| PidProbe::Alive));
    }

    #[test]
    fn roundtrip_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.lock");
        let lock = ServeLock {
            pid: 42,
            port: 3030,
            started_at: 99,
            interactive: true,
            parent_pid: Some(1),
        };
        write_lock(&path, &lock).unwrap();
        assert_eq!(read_lock(&path), Some(lock));
        remove_lock(&path);
        assert!(read_lock(&path).is_none());
    }

    #[test]
    fn decide_lock_free_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.lock");
        match decide_lock(&path, 3030, true, false).unwrap() {
            LockDecision::Free => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decide_lock_blocked_without_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serve.lock");
        let lock = ServeLock {
            pid: std::process::id(),
            port: 3030,
            started_at: now_secs(),
            interactive: false,
            parent_pid: None,
        };
        write_lock(&path, &lock).unwrap();
        match decide_lock(&path, 3030, true, false).unwrap() {
            LockDecision::Blocked(h) => assert_eq!(h.pid, lock.pid),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn self_pid_is_alive() {
        assert_eq!(pid_alive(std::process::id()), PidProbe::Alive);
    }

    #[test]
    fn lock_path_is_under_whycodes() {
        let p = lock_path(Path::new("/tmp/proj"));
        assert!(p.ends_with(".whycodes/serve.lock") || p.ends_with(".whycodes\\serve.lock"));
    }

    #[test]
    fn blocked_message_names_pid_and_url() {
        let msg = blocked_message(&ServeLock {
            pid: 9,
            port: 4040,
            started_at: 0,
            interactive: false,
            parent_pid: None,
        });
        assert!(msg.contains("pid 9"), "{msg}");
        assert!(msg.contains("127.0.0.1:4040"), "{msg}");
    }
}
