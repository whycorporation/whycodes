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
    /// Unix `kill(pid, 0)` only; Windows `OpenProcess` never yields this.
    #[cfg_attr(not(unix), allow(dead_code))]
    Denied,
}

pub(crate) fn pid_alive(pid: u32) -> PidProbe {
    probe_pid(pid)
}

#[cfg(unix)]
fn probe_pid(pid: u32) -> PidProbe {
    let Some(tid) = unix_kill_pid(pid) else {
        return PidProbe::Dead;
    };
    // A zombie still answers kill(pid, 0) but is not holding a port.
    if proc_is_zombie(tid) {
        return PidProbe::Dead;
    }
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let rc = unsafe { kill(tid, 0) };
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

/// `/proc/<pid>/stat` state `Z` — Linux only. Other unix: not a zombie.
#[cfg(target_os = "linux")]
fn proc_is_zombie(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    proc_stat_is_zombie(&stat)
}

/// `pid (comm) state ...`; comm may contain `)`.
#[cfg(any(test, target_os = "linux"))]
fn proc_stat_is_zombie(stat: &str) -> bool {
    let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
        return false;
    };
    after.split_whitespace().next() == Some("Z")
}

#[cfg(all(unix, not(target_os = "linux")))]
fn proc_is_zombie(_pid: i32) -> bool {
    false
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
        parent_pid: current_parent_pid(),
    }
}

fn current_parent_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(std::os::unix::process::parent_id())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn stdin_can_prompt() -> bool {
    #[cfg(test)]
    if std::env::var_os("WHYCODES_TEST_TAKEOVER").is_some() {
        return true;
    }
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
    let choice = takeover_prompt_choice(&holder);
    apply_takeover_choice(path, port, holder, choice)
}

#[cfg(test)]
fn takeover_prompt_choice(_holder: &ServeLock) -> Result<usize, ()> {
    match std::env::var("WHYCODES_TEST_TAKEOVER").ok().as_deref() {
        Some("0") => Ok(0),
        Some("2") => Ok(2),
        _ => Err(()),
    }
}

#[cfg(not(test))]
fn takeover_prompt_choice(holder: &ServeLock) -> Result<usize, ()> {
    let items = ["Take over", "Abort", "Start anyway"];
    dialoguer::Select::new()
        .with_prompt(format!(
            "whycodes serve already running (pid {}, http://127.0.0.1:{})",
            holder.pid, holder.port
        ))
        .items(items)
        .default(0)
        .interact()
        .map_err(|_| ())
}

pub(crate) fn apply_takeover_choice(
    path: &Path,
    port: u16,
    holder: ServeLock,
    choice: Result<usize, ()>,
) -> anyhow::Result<LockDecision> {
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

/// `kill(2)` treats pid <= 0 as broadcast: 0 = process group, -1 = every
/// process we can signal, < -1 = that process group. Casting a `u32` pid
/// with `as i32` turns 0 and values above `i32::MAX` (e.g. `u32::MAX`) into
/// those. Never pass them through.
#[cfg(unix)]
fn unix_kill_pid(pid: u32) -> Option<i32> {
    i32::try_from(pid).ok().filter(|&tid| tid > 0)
}

#[cfg(unix)]
fn signal_pid(pid: u32, sig: i32) -> std::io::Result<()> {
    let Some(tid) = unix_kill_pid(pid) else {
        return Err(std::io::Error::new(
            ErrorKind::NotFound,
            "pid is not a single process (0 / overflow would broadcast)",
        ));
    };
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let rc = unsafe { kill(tid, sig) };
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
#[path = "lockfile_tests.rs"]
mod tests;
