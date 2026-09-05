//! Host shell used when bubblewrap is off or unavailable.
//!
//! Unix runs `bash -c`. Windows never launches WSL: Git Bash if present,
//! otherwise `cmd.exe /C`. `C:\Windows\System32\bash.exe` is the WSL stub.

#[cfg(any(windows, test))]
use std::ffi::OsStr;
#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

use super::policy::{Backend, PreparedCommand};

/// Plain host shell (no namespace isolation).
pub fn prepare_host(
    command: &str,
    working_dir: PathBuf,
    warning: Option<String>,
) -> PreparedCommand {
    let invocation = host_invocation(command);
    PreparedCommand {
        program: invocation.program,
        args: invocation.args,
        working_dir,
        backend: Backend::Host,
        warning: merge_warnings(warning, invocation.warning),
    }
}

struct HostInvocation {
    program: String,
    args: Vec<String>,
    warning: Option<String>,
}

fn host_invocation(command: &str) -> HostInvocation {
    #[cfg(windows)]
    {
        windows_host_invocation(command)
    }
    #[cfg(not(windows))]
    {
        HostInvocation {
            program: "bash".into(),
            args: vec!["-c".into(), command.to_string()],
            warning: None,
        }
    }
}

#[cfg(windows)]
fn windows_host_invocation(command: &str) -> HostInvocation {
    let override_shell =
        std::env::var_os("WHYCODES_SHELL").map(|v| v.to_string_lossy().into_owned());
    let native_bash = find_native_bash(
        std::env::var_os("PATH").as_deref(),
        &well_known_git_bash_paths(
            std::env::var_os("ProgramFiles").map(PathBuf::from),
            std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            std::env::var_os("USERPROFILE").map(PathBuf::from),
        ),
    );
    let comspec = std::env::var_os("ComSpec").map(|v| v.to_string_lossy().into_owned());
    let shell =
        resolve_windows_host_shell(override_shell.as_deref(), native_bash, comspec.as_deref());
    let mut args = shell.args_prefix;
    args.push(command.to_string());
    HostInvocation {
        program: shell.program,
        args,
        warning: shell.warning,
    }
}

#[cfg(any(windows, test))]
pub(crate) struct WindowsHostShell {
    pub program: String,
    pub args_prefix: Vec<String>,
    pub warning: Option<String>,
}

/// Pick a Windows shell that will not start WSL2.
#[cfg(any(windows, test))]
pub(crate) fn resolve_windows_host_shell(
    override_shell: Option<&str>,
    native_bash: Option<PathBuf>,
    comspec: Option<&str>,
) -> WindowsHostShell {
    if let Some(raw) = override_shell.map(str::trim).filter(|s| !s.is_empty()) {
        let path = Path::new(raw);
        if !is_wsl_stub(path) {
            return windows_shell_from_program(raw, None);
        }
    }
    if let Some(bash) = native_bash {
        return WindowsHostShell {
            program: bash.to_string_lossy().into_owned(),
            args_prefix: vec!["-c".into()],
            warning: None,
        };
    }
    let cmd = comspec
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("cmd.exe");
    WindowsHostShell {
        program: cmd.to_string(),
        args_prefix: vec!["/C".into()],
        warning: Some(
            "no Git Bash on this Windows host; using cmd.exe. \
             Unix-style commands may fail. Install Git for Windows \
             (https://git-scm.com) or set WHYCODES_SHELL."
                .into(),
        ),
    }
}

#[cfg(any(windows, test))]
fn windows_file_name(path: &str) -> String {
    path.replace('/', "\\")
        .rsplit('\\')
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}

#[cfg(any(windows, test))]
pub(crate) fn windows_shell_from_program(
    program: &str,
    warning: Option<String>,
) -> WindowsHostShell {
    let name = windows_file_name(program);
    let args_prefix = if name == "cmd.exe" || name == "cmd" {
        vec!["/C".into()]
    } else if name == "powershell.exe"
        || name == "powershell"
        || name == "pwsh.exe"
        || name == "pwsh"
    {
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
        ]
    } else {
        vec!["-c".into()]
    };
    WindowsHostShell {
        program: program.to_string(),
        args_prefix,
        warning,
    }
}

/// True for the WSL launcher (`System32\bash.exe`) and `wsl.exe`.
///
/// Path separators are normalized so unit tests on Unix still match Windows
/// strings (`C:\Windows\System32\bash.exe`).
#[cfg(any(windows, test))]
pub(crate) fn is_wsl_stub(path: &Path) -> bool {
    let n = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let file = n.rsplit('\\').next().unwrap_or("");
    if file == "wsl.exe" || file == "wsl" {
        return true;
    }
    if file != "bash.exe" && file != "bash" {
        return false;
    }
    n.contains("\\system32\\") || n.contains("\\syswow64\\") || n.contains("\\windowsapps\\")
}

/// First non-WSL `bash.exe` on PATH, then extra candidate files.
#[cfg(any(windows, test))]
pub(crate) fn find_native_bash(path_var: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path) = path_var {
        for dir in std::env::split_paths(path) {
            for name in ["bash.exe", "bash"] {
                let candidate = dir.join(name);
                if candidate.is_file() && !is_wsl_stub(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    extra
        .iter()
        .find(|p| p.is_file() && !is_wsl_stub(p))
        .cloned()
}

#[cfg(any(windows, test))]
pub(crate) fn well_known_git_bash_paths(
    program_files: Option<PathBuf>,
    program_files_x86: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    user_profile: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut push_root = |root: Option<PathBuf>| {
        if let Some(root) = root {
            v.push(root.join("Git").join("bin").join("bash.exe"));
            v.push(root.join("Git").join("usr").join("bin").join("bash.exe"));
        }
    };
    push_root(program_files);
    push_root(program_files_x86);
    if let Some(local) = local_app_data {
        v.push(
            local
                .join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }
    if let Some(home) = user_profile {
        v.push(
            home.join("scoop")
                .join("apps")
                .join("git")
                .join("current")
                .join("bin")
                .join("bash.exe"),
        );
    }
    v
}

pub(crate) fn merge_warnings(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(mut left), Some(right)) => {
            left.push_str("; ");
            left.push_str(&right);
            Some(left)
        }
    }
}
