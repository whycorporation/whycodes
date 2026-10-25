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
mod clean;
mod host;
mod policy;
#[cfg(windows)]
mod process_tree;
mod scratch;

pub use host::{EMBEDDED_SHELL_NAME, embedded_shell, host_shell_name, set_embedded_shell};
pub use policy::{
    Backend, PreparedCommand, SandboxError, SandboxOutcome, SandboxRequest, first_sighting,
    kill_pid_group, prepare, prepare_with, run, run_timeout,
};
#[cfg(windows)]
pub use process_tree::ProcessTree;
pub use scratch::{SCRATCH_ENV, SCRATCH_MAX_AGE, SCRATCH_MAX_BYTES};

use whycodes_core::{SandboxFallback, SandboxMode, SandboxSettings};

pub fn backend_available() -> bool {
    bwrap::bwrap_path().is_some()
}

pub fn describe_backend(settings: &SandboxSettings) -> String {
    describe_backend_with(settings, backend_available())
}

fn describe_backend_with(settings: &SandboxSettings, bwrap_available: bool) -> String {
    match settings.mode {
        SandboxMode::Off => "off (host shell)".to_string(),
        SandboxMode::Workspace => {
            if bwrap_available {
                if settings.network {
                    "workspace (bwrap, network on)".to_string()
                } else {
                    "workspace (bwrap, network off)".to_string()
                }
            } else {
                match settings.fallback {
                    SandboxFallback::Allow => {
                        "workspace requested, bwrap missing → host (fallback allow)".to_string()
                    }
                    SandboxFallback::Deny => {
                        "workspace requested, bwrap missing → deny".to_string()
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
