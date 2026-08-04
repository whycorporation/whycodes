//! Bubblewrap (`bwrap`) backend for Linux.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::policy::{Backend, PreparedCommand, SandboxError};

static BWRAP: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn bwrap_path() -> Option<&'static Path> {
    BWRAP.get_or_init(which_bwrap).as_deref()
}

fn which_bwrap() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("bwrap");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for p in ["/usr/bin/bwrap", "/bin/bwrap"] {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    None
}

const HOME_RW_BINDS: &[&str] = &[
    ".cargo",
    ".rustup",
    ".npm",
    ".cache",
    ".local/share/pnpm",
    ".local/share/uv",
    ".local/state",
    "go/pkg",
    ".bun",
    ".yarn",
];

pub fn prepare_bwrap(
    command: &str,
    working_dir: &Path,
    network: bool,
) -> Result<PreparedCommand, SandboxError> {
    let bwrap =
        bwrap_path().ok_or_else(|| SandboxError::Unavailable("bwrap not found on PATH".into()))?;

    let cwd = working_dir.to_path_buf();
    let cwd_str = cwd.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::with_capacity(64);

    push(&mut args, ["--ro-bind", "/", "/"]);
    push(&mut args, ["--dev", "/dev"]);
    push(&mut args, ["--proc", "/proc"]);
    push(&mut args, ["--tmpfs", "/tmp"]);

    if cwd.exists() {
        push(&mut args, ["--bind", &cwd_str, &cwd_str]);
    } else {
        tracing::debug!(
            path = %cwd_str,
            "sandbox workspace path does not exist yet; skipping RW bind"
        );
    }

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for rel in HOME_RW_BINDS {
            let p = home.join(rel);
            if p.exists() {
                let s = p.to_string_lossy();
                push(&mut args, ["--bind-try", &s, &s]);
            }
        }
        if let Ok(auth_sock) = std::env::var("SSH_AUTH_SOCK") {
            let sock = PathBuf::from(&auth_sock);
            if let Some(parent) = sock.parent()
                && parent.exists()
            {
                let s = parent.to_string_lossy();
                push(&mut args, ["--bind-try", &s, &s]);
            }
        }
    }

    if !network {
        args.push("--unshare-net".into());
    }

    push(&mut args, ["--die-with-parent"]);
    push(&mut args, ["--new-session"]);
    push(&mut args, ["--chdir", &cwd_str]);

    args.push("--".into());
    args.push("bash".into());
    args.push("-c".into());
    args.push(command.to_string());

    Ok(PreparedCommand {
        program: bwrap.to_string_lossy().into_owned(),
        args,
        working_dir: cwd,
        backend: Backend::Bubblewrap,
        warning: None,
    })
}

fn push(args: &mut Vec<String>, items: impl IntoIterator<Item = impl AsRef<str>>) {
    for item in items {
        args.push(item.as_ref().to_string());
    }
}
