//! OS-level sandbox for shell commands.
//!
//! `whycode-command-risk` classifies command *strings*. This crate is the
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
    Backend, PreparedCommand, SandboxError, SandboxOutcome, SandboxRequest, prepare, run,
};

use whycode_core::{SandboxFallback, SandboxMode, SandboxSettings};

pub fn backend_available() -> bool {
    bwrap::bwrap_path().is_some()
}

pub fn describe_backend(settings: &SandboxSettings) -> String {
    match settings.mode {
        SandboxMode::Off => "off (host shell)".to_string(),
        SandboxMode::Workspace => {
            if backend_available() {
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
mod tests {
    use super::*;
    use std::path::PathBuf;
    use whycode_core::{SandboxFallback, SandboxMode, SandboxSettings};

    #[test]
    fn off_mode_prepares_host_bash() {
        let req = SandboxRequest {
            command: "echo hi".into(),
            working_dir: PathBuf::from("/tmp"),
            settings: SandboxSettings {
                mode: SandboxMode::Off,
                network: true,
                fallback: SandboxFallback::Allow,
            },
        };
        let prepared = prepare(&req).expect("off always prepares");
        assert_eq!(prepared.backend, Backend::Host);
        assert_eq!(prepared.program, "bash");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn workspace_prefers_bwrap_when_present() {
        if !backend_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let req = SandboxRequest {
            command: "echo hi".into(),
            working_dir: dir.path().to_path_buf(),
            settings: SandboxSettings {
                mode: SandboxMode::Workspace,
                network: false,
                fallback: SandboxFallback::Deny,
            },
        };
        let prepared = prepare(&req).expect("bwrap prepare");
        assert_eq!(prepared.backend, Backend::Bubblewrap);
        assert!(
            prepared.args.iter().any(|a| a == "--unshare-net"),
            "network=false must pass --unshare-net: {:?}",
            prepared.args
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn workspace_rw_project_and_blocks_home_write() {
        if !backend_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let settings = SandboxSettings {
            mode: SandboxMode::Workspace,
            network: true,
            fallback: SandboxFallback::Deny,
        };
        let out = run(&SandboxRequest {
            command: "echo sandboxed > marker.txt && cat marker.txt".into(),
            working_dir: dir.path().to_path_buf(),
            settings: settings.clone(),
        })
        .expect("run");
        assert!(out.status.success(), "stderr={}", out.stderr_lossy());
        assert!(out.stdout_lossy().contains("sandboxed"));
        let marker = std::fs::read_to_string(dir.path().join("marker.txt")).unwrap();
        assert_eq!(marker.trim(), "sandboxed");

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let probe = home.join(format!("whycode-sandbox-probe-{}", std::process::id()));
        let _ = std::fs::remove_file(&probe);
        let out = run(&SandboxRequest {
            command: format!("echo x > {}", probe.display()),
            working_dir: dir.path().to_path_buf(),
            settings,
        })
        .expect("run");
        assert!(
            !out.status.success() || !probe.exists(),
            "home write must fail under workspace sandbox"
        );
        let _ = std::fs::remove_file(&probe);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn network_off_blocks_tcp() {
        if !backend_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = run(&SandboxRequest {
            command: "python3 -c \"import socket;s=socket.socket();s.settimeout(1);s.connect(('1.1.1.1',80))\"".into(),
            working_dir: dir.path().to_path_buf(),
            settings: SandboxSettings {
                mode: SandboxMode::Workspace,
                network: false,
                fallback: SandboxFallback::Deny,
            },
        })
        .expect("run");
        assert!(
            !out.status.success(),
            "TCP should fail with network off: {}",
            out.stderr_lossy()
        );
    }
}
