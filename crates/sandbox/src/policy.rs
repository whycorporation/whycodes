use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
    run_with(request, bwrap::bwrap_path().is_some())
}

pub(crate) fn run_with(
    request: &SandboxRequest,
    bwrap_available: bool,
) -> Result<SandboxOutcome, SandboxError> {
    let prepared = prepare_with(request, bwrap_available)?;
    let output = spawn_capture(&prepared)?;
    Ok(SandboxOutcome {
        backend: prepared.backend,
        warning: prepared.warning,
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub(crate) fn spawn_capture(prepared: &PreparedCommand) -> Result<Output, SandboxError> {
    let mut cmd = Command::new(&prepared.program);
    cmd.args(&prepared.args)
        .current_dir(&prepared.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(cmd.output()?)
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
