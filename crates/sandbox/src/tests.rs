//! Unit tests. Kept in a sibling file so the crate's 100% llvm-cov floor
//! measures production `lib.rs` / `bwrap.rs` / `policy.rs` / `host.rs` only.
//! Host-dependent branches in this file must not fail CI.

use super::*;
use std::path::PathBuf;
use std::time::Duration;
use whycodes_core::{SandboxFallback, SandboxMode, SandboxSettings};

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

/// Program name stored on `PreparedCommand`. Need not exist — prepare
/// never execs it.
fn stub_bwrap() -> &'static std::path::Path {
    std::path::Path::new("/usr/bin/bwrap")
}

#[test]
fn prepare_bwrap_bin_unshare_net_when_network_off() {
    let dir = tempfile::tempdir().unwrap();
    let prepared =
        crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "echo hi", dir.path(), false)
            .expect("prepare");
    assert_eq!(prepared.backend, Backend::Bubblewrap);
    assert_eq!(prepared.program, stub_bwrap().to_string_lossy());
    assert!(
        prepared.args.iter().any(|a| a == "--unshare-net"),
        "network=false must pass --unshare-net: {:?}",
        prepared.args
    );
}

#[test]
fn describe_backend_covers_every_mode() {
    let off = SandboxSettings {
        mode: SandboxMode::Off,
        network: true,
        fallback: SandboxFallback::Allow,
    };
    assert!(describe_backend(&off).contains("off"));

    let ws_net = SandboxSettings {
        mode: SandboxMode::Workspace,
        network: true,
        fallback: SandboxFallback::Allow,
    };
    let ws_nonet = SandboxSettings {
        mode: SandboxMode::Workspace,
        network: false,
        fallback: SandboxFallback::Deny,
    };
    assert!(describe_backend_with(&ws_net, true).contains("network on"));
    assert!(describe_backend_with(&ws_nonet, true).contains("network off"));
    assert!(describe_backend_with(&ws_net, false).contains("fallback allow"));
    assert!(describe_backend_with(&ws_nonet, false).contains("deny"));
    // Exercise the public wrapper against the real host.
    let live = describe_backend(&ws_net);
    assert!(!live.is_empty());
}

#[test]
fn workspace_without_bwrap_respects_fallback() {
    let req_deny = SandboxRequest {
        command: "true".into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Workspace,
            network: false,
            fallback: SandboxFallback::Deny,
        },
    };
    assert!(matches!(
        prepare_with(&req_deny, false),
        Err(SandboxError::Unavailable(_))
    ));

    let req_allow = SandboxRequest {
        command: "true".into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Workspace,
            network: false,
            fallback: SandboxFallback::Allow,
        },
    };
    let prepared = prepare_with(&req_allow, false).expect("allow fallback");
    assert_eq!(prepared.backend, Backend::Host);
    assert!(prepared.warning.is_some());
}

#[test]
fn find_bwrap_in_none_path_uses_fallbacks() {
    let none = crate::bwrap::find_bwrap_in(None, &["/no/bwrap"]);
    assert!(none.is_none());
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("bwrap");
    std::fs::write(&fake, b"").unwrap();
    let hit = crate::bwrap::find_bwrap_in(None, &[fake.to_str().unwrap()]);
    assert_eq!(hit.as_deref(), Some(fake.as_path()));
}

#[test]
fn display_content_combines_streams_and_warning() {
    fn outcome(stdout: &str, stderr: &str, code: i32, warning: Option<&str>) -> SandboxOutcome {
        use std::os::unix::process::ExitStatusExt;
        SandboxOutcome {
            backend: Backend::Host,
            warning: warning.map(str::to_string),
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    let (empty, ok) = outcome("", "", 0, None).display_content();
    assert!(ok);
    assert!(empty.contains("exit code: 0"), "{empty}");

    let (warn_only, _) = outcome("", "", 0, Some("only-warn")).display_content();
    assert!(warn_only.contains("[sandbox] only-warn"), "{warn_only}");

    let mut empty = String::new();
    crate::policy::append_sandbox_warning_for_test(&mut empty, "lone");
    assert_eq!(empty, "[sandbox] lone");
    crate::policy::append_sandbox_warning_for_test(&mut empty, "two");
    assert!(empty.contains('\n'), "{empty}");

    let (out_only, _) = outcome("hello", "", 0, None).display_content();
    assert_eq!(out_only, "hello");

    let (err_only, fail) = outcome("", "boom", 1, None).display_content();
    assert!(!fail);
    assert!(err_only.contains("[stderr]"), "{err_only}");
    assert!(err_only.contains("boom"), "{err_only}");

    let (both, _) = outcome("out", "err", 0, Some("warn")).display_content();
    assert!(both.contains("out") && both.contains("[stderr]") && both.contains("[sandbox] warn"));
}

#[test]
fn run_off_mode_captures_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&SandboxRequest {
        command: "echo captured".into(),
        working_dir: dir.path().to_path_buf(),
        settings: SandboxSettings {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    })
    .expect("run");
    assert!(out.status.success());
    assert!(out.stdout_lossy().contains("captured"));
    assert!(out.stderr_lossy().is_empty());
}

#[test]
fn missing_working_dir_still_prepares() {
    let missing = PathBuf::from("/no/such/whycodes-sandbox-cwd");
    let req = SandboxRequest {
        command: "true".into(),
        working_dir: missing.clone(),
        settings: SandboxSettings {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    };
    let prepared = prepare(&req).expect("missing cwd is allowed");
    assert_eq!(prepared.working_dir, missing);
}

#[test]
fn prepare_bwrap_skips_missing_cwd_and_optional_binds() {
    let missing = tempfile::tempdir().unwrap().path().join("not-created");
    let prepared = crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "echo hi", &missing, true)
        .expect("prepare");
    assert_eq!(prepared.backend, Backend::Bubblewrap);
    assert!(!prepared.args.iter().any(|a| a == "--unshare-net"));
    assert!(
        !prepared
            .args
            .windows(3)
            .any(|w| w[0] == "--bind" && w[1] == missing.to_string_lossy())
    );
}

#[test]
fn prepare_bwrap_binds_ssh_agent_and_home_cache() {
    let _lock = env_lock();
    let home = tempfile::tempdir().unwrap();
    let cargo = home.path().join(".cargo");
    std::fs::create_dir_all(&cargo).unwrap();
    let sock_dir = home.path().join("ssh");
    std::fs::create_dir_all(&sock_dir).unwrap();
    let sock = sock_dir.join("agent.sock");
    std::fs::write(&sock, b"").unwrap();

    let prev_home = std::env::var_os("HOME");
    let prev_sock = std::env::var_os("SSH_AUTH_SOCK");
    unsafe { std::env::set_var("HOME", home.path()) };
    unsafe { std::env::set_var("SSH_AUTH_SOCK", &sock) };
    let dir = tempfile::tempdir().unwrap();
    let prepared = crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), false);
    restore_os("HOME", prev_home);
    restore_os("SSH_AUTH_SOCK", prev_sock);
    let prepared = prepared.expect("prepare");
    let cargo_s = cargo.to_string_lossy();
    assert!(
        prepared
            .args
            .windows(3)
            .any(|w| w[0] == "--bind-try" && w[1] == cargo_s),
        "{:?}",
        prepared.args
    );
    let sock_parent = sock_dir.to_string_lossy();
    assert!(
        prepared
            .args
            .windows(3)
            .any(|w| w[0] == "--bind-try" && w[1] == sock_parent),
        "{:?}",
        prepared.args
    );
}

#[test]
fn prepare_delegates_to_prepare_with_real_availability() {
    let req = SandboxRequest {
        command: "true".into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Workspace,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    };
    let got = prepare(&req).expect("prepare");
    let bwrap = got.backend == Backend::Bubblewrap;
    let host_fallback = got.backend == Backend::Host && got.warning.is_some();
    assert!(
        bwrap || host_fallback,
        "backend={:?} warning={:?}",
        got.backend,
        got.warning
    );
}

#[test]
fn prepare_with_forced_bwrap_hits_prepare_bwrap() {
    let req = SandboxRequest {
        command: "true".into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Workspace,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    };
    let got = prepare_with(&req, true);
    let is_bwrap = got.as_ref().ok().map(|p| p.backend) == Some(Backend::Bubblewrap);
    let is_unavail = matches!(got, Err(SandboxError::Unavailable(_)));
    assert!(is_bwrap || is_unavail, "{got:?}");
}

#[test]
fn canonicalize_missing_path_is_bad_working_dir() {
    let err =
        crate::policy::canonicalize_or_bad(std::path::Path::new("/no/such/whycodes-sandbox-canon"));
    assert!(matches!(err, Err(SandboxError::BadWorkingDir(_))));
}

#[test]
fn canonicalize_existing_working_dir() {
    let dir = tempfile::tempdir().unwrap();
    let req = SandboxRequest {
        command: "true".into(),
        working_dir: dir.path().to_path_buf(),
        settings: SandboxSettings {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    };
    let prepared = prepare(&req).expect("ok");
    assert!(prepared.working_dir.is_absolute());
}

#[test]
fn find_bwrap_path_and_fallback_and_none() {
    let found = crate::bwrap::find_bwrap();
    assert_eq!(found.is_some(), backend_available());

    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("bwrap");
    std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&fake).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&fake, p).unwrap();
    }
    let via_path =
        crate::bwrap::find_bwrap_in(Some(dir.path().as_os_str().to_os_string()), &["/no/bwrap"]);
    assert_eq!(via_path.as_deref(), Some(fake.as_path()));

    let none = crate::bwrap::find_bwrap_in(Some("/no/such/bin".into()), &["/no/bwrap"]);
    assert!(none.is_none());

    let fallback_file = dir.path().join("fallback-bwrap");
    std::fs::write(&fallback_file, b"").unwrap();
    let via_fallback = crate::bwrap::find_bwrap_in(
        Some("/no/such/bin".into()),
        &[fallback_file.to_str().unwrap()],
    );
    assert_eq!(via_fallback.as_deref(), Some(fallback_file.as_path()));
}

#[test]
fn prepare_bwrap_without_home_and_root_auth_sock() {
    let _lock = env_lock();
    let prev_home = std::env::var_os("HOME");
    let prev_sock = std::env::var_os("SSH_AUTH_SOCK");
    unsafe { std::env::remove_var("HOME") };
    unsafe { std::env::set_var("SSH_AUTH_SOCK", "/") };
    let dir = tempfile::tempdir().unwrap();
    let prepared = crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), true);
    restore_os("HOME", prev_home);
    restore_os("SSH_AUTH_SOCK", prev_sock);
    prepared.expect("prepare without HOME");

    let prev_home = std::env::var_os("HOME");
    let prev_sock = std::env::var_os("SSH_AUTH_SOCK");
    unsafe { std::env::set_var("HOME", tempfile::tempdir().unwrap().path()) };
    unsafe { std::env::set_var("SSH_AUTH_SOCK", "/no/such/whycodes-ssh-dir/agent.sock") };
    let dir = tempfile::tempdir().unwrap();
    let prepared = crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), true);
    restore_os("HOME", prev_home);
    restore_os("SSH_AUTH_SOCK", prev_sock);
    prepared.expect("prepare with missing ssh parent");

    // HOME set, SSH_AUTH_SOCK unset — the `if let Ok(auth_sock)` miss
    // path. Self-hosted CI often has an agent socket in the environment,
    // so other tests never take this branch and llvm-cov flags the
    // closing brace of that `if let` (bwrap.rs) as uncovered.
    let prev_home = std::env::var_os("HOME");
    let prev_sock = std::env::var_os("SSH_AUTH_SOCK");
    unsafe { std::env::set_var("HOME", tempfile::tempdir().unwrap().path()) };
    unsafe { std::env::remove_var("SSH_AUTH_SOCK") };
    let dir = tempfile::tempdir().unwrap();
    let prepared = crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), true);
    restore_os("HOME", prev_home);
    restore_os("SSH_AUTH_SOCK", prev_sock);
    prepared.expect("prepare without SSH_AUTH_SOCK");
}

fn home_or_tmp(home: Option<std::ffi::OsString>) -> PathBuf {
    home.map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[test]
fn home_or_tmp_none_uses_tmp() {
    assert_eq!(home_or_tmp(None), PathBuf::from("/tmp"));
    assert_eq!(
        home_or_tmp(Some(std::ffi::OsString::from("/home/x"))),
        PathBuf::from("/home/x")
    );
}

#[test]
fn prepare_bwrap_bin_none_is_unavailable() {
    let err = crate::bwrap::prepare_bwrap_bin(None, "true", std::path::Path::new("/tmp"), true);
    assert!(matches!(err, Err(SandboxError::Unavailable(_))));
}

#[test]
fn run_with_unavailable_bwrap_denies() {
    let req = SandboxRequest {
        command: "true".into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Workspace,
            network: false,
            fallback: SandboxFallback::Deny,
        },
    };
    assert!(matches!(
        crate::policy::run_with(&req, false),
        Err(SandboxError::Unavailable(_))
    ));
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn restore_os_both_branches() {
    let prev = std::env::var_os("WHYCODES_SANDBOX_COV");
    restore_os("WHYCODES_SANDBOX_COV", Some(std::ffi::OsString::from("1")));
    restore_os("WHYCODES_SANDBOX_COV", None);
    restore_os("WHYCODES_SANDBOX_COV", prev);
}

#[test]
fn env_lock_recovers_from_poison() {
    let _ = std::thread::spawn(|| {
        let _guard = env_lock();
        panic!("poison the sandbox env lock");
    })
    .join();
    let _guard = env_lock();
}

fn restore_os(key: &str, prev: Option<std::ffi::OsString>) {
    match prev {
        Some(v) => unsafe { std::env::set_var(key, v) },
        None => unsafe { std::env::remove_var(key) },
    }
}

#[test]
fn sandbox_error_display() {
    let u = SandboxError::Unavailable("nope".into());
    assert_eq!(u.to_string(), "nope");
    let bad = crate::policy::bad_working_dir_for_test(
        std::path::Path::new("/x"),
        std::io::Error::other("e"),
    );
    assert!(bad.to_string().contains("invalid"));
    let io = SandboxError::from(std::io::Error::other("e"));
    assert!(io.to_string().contains("I/O"));
    let t = SandboxError::TimedOut(3);
    assert!(t.to_string().contains("timed out"));
}

#[test]
fn spawn_capture_missing_program_is_io_error() {
    let err = crate::policy::spawn_capture(&PreparedCommand {
        program: "/no/such/whycodes-sandbox-bin".into(),
        args: vec![],
        working_dir: PathBuf::from("/tmp"),
        backend: Backend::Host,
        warning: None,
    });
    assert!(matches!(err, Err(SandboxError::Io(_))));
}

#[test]
fn run_timeout_kills_sleep() {
    let req = SandboxRequest {
        command: "sleep 30".into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    };
    let started = std::time::Instant::now();
    let err = crate::policy::run_timeout(&req, Some(std::time::Duration::from_secs(1)));
    assert!(
        matches!(err, Err(SandboxError::TimedOut(_))),
        "expected timeout, got {err:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "timeout should not wait for sleep 30"
    );
}

fn host_req(command: &str) -> SandboxRequest {
    SandboxRequest {
        command: command.into(),
        working_dir: PathBuf::from("/tmp"),
        settings: SandboxSettings {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    }
}

#[test]
fn run_timeout_fast_command_collects_output() {
    let out = crate::policy::run_timeout(&host_req("echo covered"), Some(Duration::from_secs(5)))
        .expect("echo should finish before the timeout");
    assert!(out.status.success());
    assert!(
        out.stdout_lossy().contains("covered"),
        "stdout={}",
        out.stdout_lossy()
    );
}

#[test]
fn run_without_timeout_uses_blocking_wait() {
    let out = crate::run(&host_req("printf hi")).expect("run");
    assert!(out.status.success());
    assert_eq!(out.stdout_lossy(), "hi");
}

#[test]
fn zero_timeout_falls_back_to_blocking_wait() {
    let out = crate::policy::run_timeout(&host_req("echo z"), Some(Duration::ZERO)).expect("zero");
    assert!(out.status.success());
    assert!(out.stdout_lossy().contains('z'));
}

#[test]
fn wait_child_timeout_without_pipes_and_kill_unused_pid() {
    use std::process::{Command, Stdio};

    let mut child = Command::new("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn true");
    let result = crate::policy::wait_child_timeout(&mut child, Duration::from_secs(2)).unwrap();
    assert!(matches!(result, crate::policy::WaitResult::Done(_)));

    crate::kill_pid_group(u32::MAX);
}

#[test]
fn ignore_io_ok_and_err() {
    crate::policy::ignore_io(Ok(()), "ok");
    crate::policy::ignore_io::<()>(Err(std::io::Error::other("boom")), "err");
}

#[cfg(unix)]
#[test]
fn own_process_group_sets_pgid() {
    crate::policy::own_process_group().expect("setpgid");
}
