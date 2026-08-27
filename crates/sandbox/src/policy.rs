use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use thiserror::Error;
use whycodes_core::{SandboxFallback, SandboxMode, SandboxSettings};

use crate::{bwrap, host};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Host,
    Bubblewrap,
}

#[derive(Debug, Clone)]
pub struct SandboxRequest {
    pub command: String,
    pub working_dir: PathBuf,
    pub settings: SandboxSettings,
}

#[derive(Debug, Clone)]
pub struct PreparedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub backend: Backend,
    pub warning: Option<String>,
}

#[derive(Debug)]
pub struct SandboxOutcome {
    pub backend: Backend,
    pub warning: Option<String>,
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SandboxOutcome {
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    pub fn display_content(&self) -> (String, bool) {
        let stdout = self.stdout_lossy();
        let stderr = self.stderr_lossy();
        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("[stderr]\n");
            result.push_str(&stderr);
        }
        if result.is_empty() {
            result = format!(
                "Command executed successfully (exit code: {})",
                self.status.code().unwrap_or(0)
            );
        }
        if let Some(ref w) = self.warning {
            append_sandbox_warning(&mut result, w);
        }
        (result, self.status.success())
    }
}

fn append_sandbox_warning(result: &mut String, warning: &str) {
    if !result.is_empty() {
        result.push('\n');
    }
    result.push_str("[sandbox] ");
    result.push_str(warning);
}

#[cfg(test)]
pub(crate) fn append_sandbox_warning_for_test(result: &mut String, warning: &str) {
    append_sandbox_warning(result, warning);
}

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("{0}")]
    Unavailable(String),
    #[error("sandbox I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("working directory is invalid: {0}")]
    BadWorkingDir(String),
    #[error("command timed out after {0} seconds")]
    TimedOut(u64),
}

pub fn prepare(request: &SandboxRequest) -> Result<PreparedCommand, SandboxError> {
    prepare_with(request, bwrap::bwrap_path().is_some())
}

/// Same as [`prepare`] but the caller supplies whether bwrap is present.
/// Tests use this to hit both fallback branches on a host that has bwrap.
pub fn prepare_with(
    request: &SandboxRequest,
    bwrap_available: bool,
) -> Result<PreparedCommand, SandboxError> {
    let cwd = resolve_working_dir(&request.working_dir)?;

    match request.settings.mode {
        SandboxMode::Off => Ok(host::prepare_host(&request.command, cwd, None)),
        SandboxMode::Workspace => {
            if bwrap_available {
                bwrap::prepare_bwrap(&request.command, &cwd, request.settings.network)
            } else {
                let msg = "sandbox=workspace requires bubblewrap (`bwrap`) on Linux; \
                           it is not available on this host"
                    .to_string();
                match request.settings.fallback {
                    SandboxFallback::Deny => Err(SandboxError::Unavailable(msg)),
                    SandboxFallback::Allow => {
                        tracing::warn!("{msg}; running on host (sandbox_fallback=allow)");
                        Ok(host::prepare_host(
                            &request.command,
                            cwd,
                            Some(format!("{msg}; ran on host (sandbox_fallback=allow)")),
                        ))
                    }
                }
            }
        }
    }
}

pub fn run(request: &SandboxRequest) -> Result<SandboxOutcome, SandboxError> {
    run_timeout(request, None)
}

/// Like [`run`] but kill the process group if `timeout` elapses.
///
/// Dropping a `spawn_blocking` future does **not** stop `Command::output()`.
/// The timeout must live inside this spawn so the child (and its group) die.
pub fn run_timeout(
    request: &SandboxRequest,
    timeout: Option<Duration>,
) -> Result<SandboxOutcome, SandboxError> {
    run_with_timeout(request, bwrap::bwrap_path().is_some(), timeout)
}

#[cfg(test)]
pub(crate) fn run_with(
    request: &SandboxRequest,
    bwrap_available: bool,
) -> Result<SandboxOutcome, SandboxError> {
    run_with_timeout(request, bwrap_available, None)
}

pub(crate) fn run_with_timeout(
    request: &SandboxRequest,
    bwrap_available: bool,
    timeout: Option<Duration>,
) -> Result<SandboxOutcome, SandboxError> {
    let prepared = prepare_with(request, bwrap_available)?;
    let output = spawn_capture_timeout(&prepared, timeout)?;
    Ok(SandboxOutcome {
        backend: prepared.backend,
        warning: prepared.warning,
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(test)]
pub(crate) fn spawn_capture(prepared: &PreparedCommand) -> Result<Output, SandboxError> {
    spawn_capture_timeout(prepared, None)
}

pub(crate) fn spawn_capture_timeout(
    prepared: &PreparedCommand,
    timeout: Option<Duration>,
) -> Result<Output, SandboxError> {
    let mut cmd = Command::new(&prepared.program);
    cmd.args(&prepared.args)
        .current_dir(&prepared.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_new_process_group(&mut cmd);
    let child = cmd.spawn()?;
    let Some(limit) = timeout.filter(|d| !d.is_zero()) else {
        return Ok(child.wait_with_output()?);
    };
    let mut child = child;
    match wait_child_timeout(&mut child, limit)? {
        WaitResult::Done(output) => Ok(output),
        WaitResult::TimedOut(secs) => {
            kill_process_group(&mut child);
            ignore_io(child.wait(), "wait after timeout kill");
            Err(SandboxError::TimedOut(secs))
        }
    }
}

pub(crate) enum WaitResult {
    Done(Output),
    TimedOut(u64),
}

pub(crate) fn wait_child_timeout(
    child: &mut Child,
    limit: Duration,
) -> Result<WaitResult, SandboxError> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx_out, rx_out) = mpsc::channel();
    let (tx_err, rx_err) = mpsc::channel();
    let has_out = stdout.is_some();
    let has_err = stderr.is_some();
    if let Some(mut out) = stdout {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            ignore_io(
                std::io::Read::read_to_end(&mut out, &mut buf),
                "drain stdout",
            );
            if tx_out.send(buf).is_err() {
                tracing::debug!("stdout receiver dropped");
            }
        });
    }
    if let Some(mut err) = stderr {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            ignore_io(
                std::io::Read::read_to_end(&mut err, &mut buf),
                "drain stderr",
            );
            if tx_err.send(buf).is_err() {
                tracing::debug!("stderr receiver dropped");
            }
        });
    }

    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout = if has_out {
                    rx_out.recv().unwrap_or_default()
                } else {
                    Vec::new()
                };
                let stderr = if has_err {
                    rx_err.recv().unwrap_or_default()
                } else {
                    Vec::new()
                };
                return Ok(WaitResult::Done(Output {
                    status,
                    stdout,
                    stderr,
                }));
            }
            None => {
                if Instant::now() >= deadline {
                    return Ok(WaitResult::TimedOut(limit.as_secs().max(1)));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Log and swallow I/O that is already on a best-effort path (timeout kill,
/// pipe drain). Extracted so the 100% line floor can hit the error arm.
pub(crate) fn ignore_io<T>(result: std::io::Result<T>, what: &'static str) {
    if let Err(e) = result {
        tracing::debug!(error = %e, context = what, "sandbox io");
    }
}

/// `setpgid(0, 0)` so timeout can SIGKILL bash + grandchildren.
///
/// Pulled out of `pre_exec`: llvm-cov does not attribute the forked child
/// closure, which would fail the crate's 100% line floor.
#[cfg(unix)]
pub(crate) fn own_process_group() -> std::io::Result<()> {
    unsafe {
        libc::setpgid(0, 0);
    }
    Ok(())
}

fn configure_new_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(own_process_group);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

/// Kill `pid` and every process in its group (Unix). No-op on other platforms
/// besides what the caller does with `Child::kill`.
pub fn kill_pid_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn kill_process_group(child: &mut Child) {
    kill_pid_group(child.id());
    ignore_io(child.kill(), "kill child after timeout");
}

fn resolve_working_dir(path: &Path) -> Result<PathBuf, SandboxError> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }
    canonicalize_or_bad(path)
}

pub(crate) fn canonicalize_or_bad(path: &Path) -> Result<PathBuf, SandboxError> {
    std::fs::canonicalize(path).map_err(|e| bad_working_dir(path, e))
}

fn bad_working_dir(path: &Path, e: std::io::Error) -> SandboxError {
    SandboxError::BadWorkingDir(format!("{}: {e}", path.display()))
}

#[cfg(test)]
pub(crate) fn bad_working_dir_for_test(path: &Path, e: std::io::Error) -> SandboxError {
    bad_working_dir(path, e)
}
