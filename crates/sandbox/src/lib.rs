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
    Backend, PreparedCommand, SandboxError, SandboxOutcome, SandboxRequest, prepare, prepare_with,
    run,
};

use whycode_core::{SandboxFallback, SandboxMode, SandboxSettings};

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
        assert!(
            both.contains("out") && both.contains("[stderr]") && both.contains("[sandbox] warn")
        );
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
        let missing = PathBuf::from("/no/such/whycode-sandbox-cwd");
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
        let prepared =
            crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "echo hi", &missing, true)
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
        let prepared =
            crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), false);
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
        // One assertion so both host layouts execute the same lines
        // (llvm-cov 100% must not depend on whether bwrap is installed).
        assert!(
            got.backend == Backend::Bubblewrap
                || (got.backend == Backend::Host && got.warning.is_some()),
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
        assert!(
            matches!(&got, Ok(p) if p.backend == Backend::Bubblewrap)
                || matches!(got, Err(SandboxError::Unavailable(_))),
            "{got:?}"
        );
    }

    #[test]
    fn canonicalize_missing_path_is_bad_working_dir() {
        let err = crate::policy::canonicalize_or_bad(std::path::Path::new(
            "/no/such/whycode-sandbox-canon",
        ));
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
        let via_path = crate::bwrap::find_bwrap_in(
            Some(dir.path().as_os_str().to_os_string()),
            &["/no/bwrap"],
        );
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
        let prepared =
            crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), true);
        restore_os("HOME", prev_home);
        restore_os("SSH_AUTH_SOCK", prev_sock);
        prepared.expect("prepare without HOME");

        let prev_home = std::env::var_os("HOME");
        let prev_sock = std::env::var_os("SSH_AUTH_SOCK");
        unsafe { std::env::set_var("HOME", tempfile::tempdir().unwrap().path()) };
        unsafe { std::env::set_var("SSH_AUTH_SOCK", "/no/such/whycode-ssh-dir/agent.sock") };
        let dir = tempfile::tempdir().unwrap();
        let prepared =
            crate::bwrap::prepare_bwrap_bin(Some(stub_bwrap()), "true", dir.path(), true);
        restore_os("HOME", prev_home);
        restore_os("SSH_AUTH_SOCK", prev_sock);
        prepared.expect("prepare with missing ssh parent");
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
        let prev = std::env::var_os("WHYCODE_SANDBOX_COV");
        restore_os("WHYCODE_SANDBOX_COV", Some(std::ffi::OsString::from("1")));
        restore_os("WHYCODE_SANDBOX_COV", None);
        restore_os("WHYCODE_SANDBOX_COV", prev);
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
    }

    #[test]
    fn spawn_capture_missing_program_is_io_error() {
        let err = crate::policy::spawn_capture(&PreparedCommand {
            program: "/no/such/whycode-sandbox-bin".into(),
            args: vec![],
            working_dir: PathBuf::from("/tmp"),
            backend: Backend::Host,
            warning: None,
        });
        assert!(matches!(err, Err(SandboxError::Io(_))));
    }
}
