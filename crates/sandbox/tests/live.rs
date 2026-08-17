//! Live bubblewrap exec. Lives outside `src/` so a missing `bwrap` on the
//! CI runner does not leave `lib.rs` test bodies uncovered (the 100% crate
//! floor counts in-crate `#[cfg(test)]` lines).

#![cfg(target_os = "linux")]

use std::path::PathBuf;

use whycode_core::{SandboxFallback, SandboxMode, SandboxSettings};
use whycode_sandbox::{Backend, SandboxRequest, backend_available, prepare, run};

fn skip_without_bwrap() -> bool {
    if backend_available() {
        return false;
    }
    eprintln!("skipping: bwrap is not installed");
    true
}

#[test]
fn workspace_prefers_bwrap_when_present() {
    if skip_without_bwrap() {
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
fn workspace_rw_project_and_blocks_home_write() {
    if skip_without_bwrap() {
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
    let stderr = out.stderr_lossy();
    assert!(out.status.success(), "stderr={stderr}");
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
fn network_off_blocks_tcp() {
    if skip_without_bwrap() {
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
    let stderr = out.stderr_lossy();
    assert!(
        !out.status.success(),
        "TCP should fail with network off: {stderr}"
    );
}
