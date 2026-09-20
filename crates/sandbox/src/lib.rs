//! OS-level sandbox for shell commands.
//!
//! `whycodes-command-risk` classifies command *strings*. This crate is the
//! second lock: when enabled, shell runs inside a restricted filesystem (and
//! optionally network) namespace so blast radius is limited even if the string
//! gate misses.
//!
//! Linux uses bubblewrap (`bwrap`). Other platforms follow `SandboxFallback`.
//! This is defence in depth, not a multi-tenant security boundary.

mod bwrap;
mod host;
mod policy;

pub use policy::{
    Backend, PreparedCommand, SandboxError, SandboxOutcome, SandboxRequest, kill_pid_group,
    prepare, prepare_with, run, run_timeout,
};

use whycodes_core::{SandboxFallback, SandboxMode, SandboxSettings};

pub fn backend_available() -> bool {
    bwrap::bwrap_path().is_some()
}

pub fn describe_backend(settings: &SandboxSettings) -> String {
    describe_backend_with(settings, backend_available())
}

fn describe_backend_with(settings: &SandboxSettings, bwrap_available: bool) -> String {
    let fs = settings.filesystem.as_str();
    match settings.mode {
        SandboxMode::Off => format!("off (host shell, filesystem {fs})"),
        SandboxMode::Workspace => {
            if bwrap_available {
                if settings.network {
                    format!("workspace (bwrap, network on, filesystem {fs})")
                } else {
                    format!("workspace (bwrap, network off, filesystem {fs})")
                }
            } else {
                match settings.fallback {
                    SandboxFallback::Allow => {
                        format!(
                            "workspace requested, bwrap missing → host (fallback allow, filesystem {fs})"
                        )
                    }
                    SandboxFallback::Deny => {
                        format!("workspace requested, bwrap missing → deny (filesystem {fs})")
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
