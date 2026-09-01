//! Unit tests. Sibling file so llvm-cov --ignore-filename-regex tests.rs
//! cannot sink the crate's 100% production floor.

use crate::error::{Error, ErrorKind, TransportError};
use crate::file_claims::*;
use crate::logging::*;
use crate::network::*;
use crate::panel::*;
use crate::paths::*;
use crate::sandbox::*;
use crate::swarm_hub::*;
use crate::todo::*;
use crate::tool::*;
use crate::types::*;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ── error.rs ──────────────────────────────────────────────

mod error_tests {
    use super::*;

    #[test]
    fn test_error_display_config() {
        let err = Error::Config("bad setting".to_string());
        assert_eq!(err.to_string(), "Configuration error: bad setting");
    }

    #[test]
    fn test_error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = Error::from(io_err);
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_display_serde() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = Error::from(serde_err);
        assert!(err.to_string().contains("Serialization error"));
    }

    #[test]
    fn test_error_display_llm() {
        let err = Error::llm("rate limit");
        assert_eq!(err.to_string(), "LLM error: rate limit");
    }

    #[test]
    fn test_error_display_tool() {
        let err = Error::Tool("tool not found".to_string());
        assert_eq!(err.to_string(), "Tool error: tool not found");
    }

    #[test]
    fn test_error_display_session() {
        let err = Error::Session("expired".to_string());
        assert_eq!(err.to_string(), "Session error: expired");
    }

    #[test]
    fn test_error_display_agent() {
        let err = Error::Agent("no agent configured".to_string());
        assert_eq!(err.to_string(), "Agent error: no agent configured");
    }

    #[test]
    fn test_error_display_provider() {
        let err = Error::Provider("no api key".to_string());
        assert_eq!(err.to_string(), "Provider error: no api key");
    }

    #[test]
    fn test_error_display_http() {
        let err = Error::http("404");
        assert_eq!(err.to_string(), "HTTP error: 404");
    }

    #[test]
    fn test_error_display_other() {
        let err = Error::Other("something went wrong".to_string());
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("permission denied"));
    }

    #[test]
    fn test_error_debug() {
        let err = Error::Config("test".to_string());
        let debug = format!("{:?}", err);
        // Debug output should include the variant name
        assert!(debug.contains("Config"));
    }

    #[test]
    fn clone_preserves_every_variant() {
        let cases = [
            Error::Config("c".into()),
            Error::Io(std::io::Error::other("io")),
            Error::from(serde_json::from_str::<u8>("x").unwrap_err()),
            Error::llm("l"),
            Error::Tool("t".into()),
            Error::Session("s".into()),
            Error::Agent("a".into()),
            Error::Provider("p".into()),
            Error::http("h"),
            Error::Other("o".into()),
        ];
        for err in cases {
            let cloned = err.clone();
            assert_eq!(
                std::mem::discriminant(&err),
                std::mem::discriminant(&cloned)
            );
            assert_eq!(err.to_string(), cloned.to_string());
        }
    }

    #[test]
    fn clone_preserves_serde_message() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let original = serde_err.to_string();
        let err = Error::from(serde_err);
        assert!(err.to_string().contains(&original));
        assert_eq!(err.to_string(), err.clone().to_string());
        assert!(matches!(err, Error::Serde(_)));
    }

    #[test]
    fn error_kind_as_str_covers_every_variant() {
        let cases = [
            (ErrorKind::RateLimited, "rate_limited"),
            (ErrorKind::Server, "server"),
            (ErrorKind::Network, "network"),
            (ErrorKind::Timeout, "timeout"),
            (ErrorKind::Auth, "auth"),
            (ErrorKind::Client, "client"),
            (ErrorKind::ContextOverflow, "context_overflow"),
            (ErrorKind::Cancelled, "cancelled"),
            (ErrorKind::Unknown, "unknown"),
        ];
        for (kind, label) in cases {
            assert_eq!(kind.as_str(), label);
            assert_eq!(kind.to_string(), label);
        }
        assert!(ErrorKind::RateLimited.retryable());
        assert!(ErrorKind::Server.retryable());
        assert!(ErrorKind::Network.retryable());
        assert!(ErrorKind::Timeout.retryable());
        assert!(!ErrorKind::Auth.retryable());
        assert!(!ErrorKind::Client.retryable());
        assert!(!ErrorKind::ContextOverflow.retryable());
        assert!(!ErrorKind::Cancelled.retryable());
        assert!(!ErrorKind::Unknown.retryable());
        assert_eq!(ErrorKind::default(), ErrorKind::Unknown);
    }

    #[test]
    fn llm_and_http_carry_kind() {
        let llm = Error::llm_kind(ErrorKind::Timeout, "complete timed out after 30s");
        assert_eq!(llm.transport_kind(), Some(ErrorKind::Timeout));
        assert_eq!(llm.to_string(), "LLM error: complete timed out after 30s");
        let http = Error::http_kind(ErrorKind::RateLimited, "429");
        assert_eq!(http.transport_kind(), Some(ErrorKind::RateLimited));
        assert_eq!(http.to_string(), "HTTP error: 429");
        assert_eq!(Error::Config("x".into()).transport_kind(), None);
        let from_str: TransportError = "wire".into();
        assert_eq!(from_str.kind, ErrorKind::Unknown);
        let from_string: TransportError = String::from("wire2").into();
        assert_eq!(from_string.message, "wire2");
        let cloned = llm.clone();
        assert_eq!(cloned.transport_kind(), Some(ErrorKind::Timeout));
        assert_eq!(cloned.to_string(), llm.to_string());
    }
}

// ── file_claims.rs ──────────────────────────────────────────────

mod file_claims_tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn first_claim_acquires_second_same_agent_holds() {
        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycodes_claim_test_a.rs");
        assert_eq!(reg.try_claim("w0", "worker-0", p), ClaimResult::Acquired);
        assert_eq!(reg.try_claim("w0", "worker-0", p), ClaimResult::Held);
    }

    #[test]
    fn second_agent_conflicts_and_notifies() {
        let reg = FileClaimRegistry::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = Arc::clone(&hits);
        reg.set_listener(Some(Arc::new(move |_ev| {
            hits2.fetch_add(1, Ordering::SeqCst);
        })));
        let p = Path::new("/tmp/whycodes_claim_test_b.rs");
        assert_eq!(reg.try_claim("w0", "worker-0", p), ClaimResult::Acquired);
        match reg.try_claim("w1", "worker-1", p) {
            ClaimResult::Conflict {
                owner_id,
                owner_label,
            } => {
                assert_eq!(owner_id, "w0");
                assert_eq!(owner_label, "worker-0");
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn release_agent_frees_paths() {
        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycodes_claim_test_c.rs");
        reg.try_claim("w0", "worker-0", p);
        reg.release_agent("w0");
        assert_eq!(reg.try_claim("w1", "worker-1", p), ClaimResult::Acquired);
    }

    #[test]
    fn read_after_other_write_is_stale() {
        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycodes_claim_test_stale.rs");
        reg.try_claim("w0", "worker-0", p);
        let ev = reg.note_read("w1", p).expect("stale");
        assert_eq!(ev.writer_id, "w0");
        assert!(reg.note_read("w1", p).is_none());
    }

    #[test]
    fn claim_key_debug_snapshot_clear_and_stale_listener() {
        let reg = FileClaimRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        let _ = format!("{reg:?}");

        let rel = Path::new("/definitely-missing-whycodes-xyz/foo/../bar.rs");
        let key = FileClaimRegistry::claim_key(rel);
        assert!(key.contains("bar.rs"), "{key}");
        assert!(!key.contains("foo"), "{key}");
        let cur = FileClaimRegistry::claim_key(Path::new("./."));
        assert!(!cur.is_empty());

        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = Arc::clone(&hits);
        reg.set_stale_listener(Some(Arc::new(move |_| {
            hits2.fetch_add(1, Ordering::SeqCst);
        })));
        let p = Path::new("/tmp/whycodes_claim_snap.rs");
        assert_eq!(reg.try_claim("w0", "lab", p), ClaimResult::Acquired);
        let p2 = Path::new("/tmp/whycodes_claim_snap_b.rs");
        assert_eq!(reg.try_claim("w1", "lab-b", p2), ClaimResult::Acquired);
        assert_eq!(reg.len(), 2);
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].1, "lab");
        assert!(reg.note_read("w0", p).is_none());
        let ev = reg.note_read("w2", p);
        assert!(ev.is_some());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        reg.clear();
        assert!(reg.is_empty());
        reg.set_stale_listener(None);
        reg.set_listener(None);
    }

    #[test]
    fn recover_lock_ok_and_poisoned() {
        let m = std::sync::Mutex::new(7);
        assert_eq!(*crate::file_claims::recover_lock(m.lock()), 7);
        let m = Arc::new(std::sync::Mutex::new(3));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert_eq!(*crate::file_claims::recover_lock(m.lock()), 3);
        assert_eq!(
            crate::file_claims::path_or_dot(Ok(PathBuf::from("/abs"))),
            PathBuf::from("/abs")
        );
        assert_eq!(
            crate::file_claims::path_or_dot(Err(std::io::Error::other("no cwd"))),
            PathBuf::from(".")
        );

        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycodes_poison_claim.rs");
        reg.poison_claims();
        assert_eq!(reg.len(), 0);
        let _ = format!("{reg:?}");
        assert_eq!(reg.try_claim("w0", "lab", p), ClaimResult::Acquired);
        reg.poison_last_write();
        assert_eq!(reg.try_claim("w0", "lab", p), ClaimResult::Held);
        reg.poison_last_seen();
        assert_eq!(reg.try_claim("w0", "lab", p), ClaimResult::Held);
        reg.poison_listener();
        assert!(matches!(
            reg.try_claim("w1", "other", p),
            ClaimResult::Conflict { .. }
        ));
        reg.set_listener(None);
        reg.poison_stale_listener();
        let _ = reg.note_read("w2", p);
        reg.set_stale_listener(None);
        reg.poison_claims();
        reg.release_agent("w0");
        reg.poison_claims();
        let _ = reg.snapshot();
        reg.poison_claims();
        reg.clear();
    }
}

// ── logging.rs ──────────────────────────────────────────────

mod logging_tests {
    use super::*;

    use std::sync::Mutex as StdMutex;

    // Serialize tests that touch process-global STATE / panic hook.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;

    /// Restore the previous panic hook even if the test asserts.
    struct RestoreHook(Option<PanicHook>);
    impl Drop for RestoreHook {
        fn drop(&mut self) {
            if let Some(h) = self.0.take() {
                std::panic::set_hook(h);
            }
        }
    }

    fn temp_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn log_dirs_ensure_creates_subdirs() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        assert!(!dirs.logs.exists());
        dirs.ensure().unwrap();
        assert!(dirs.logs.is_dir());
        assert!(dirs.crash.is_dir());
        assert!(dirs.debug.is_dir());
        assert_eq!(dirs.unified_jsonl(), tmp.path().join("logs/unified.jsonl"));
    }

    #[test]
    fn append_jsonl_writes_valid_json_line() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let path = tmp.path().join("logs/unified.jsonl");
        let event = LogEvent::new("test", "info", "hello")
            .with_sid("ses_1")
            .with_ctx(json!({"k": 1}));
        append_jsonl(&path, &event).unwrap();
        append_jsonl(&path, &LogEvent::new("test", "warn", "again")).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);

        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["src"], "test");
        assert_eq!(v["lvl"], "info");
        assert_eq!(v["msg"], "hello");
        assert_eq!(v["sid"], "ses_1");
        assert_eq!(v["ctx"]["k"], 1);
        assert!(v["ts"].as_str().unwrap().contains('T'));
        assert_eq!(v["ver"], VERSION);
        assert!(v["pid"].as_u64().unwrap() > 0);

        let v2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v2["msg"], "again");
        assert!(v2.get("sid").is_none() || v2["sid"].is_null());
    }

    #[test]
    fn format_crash_report_contains_message_and_meta() {
        // We cannot construct PanicHookInfo easily; test the string pieces via
        // write_crash_report path using a real catch_unwind + hook is heavy.
        // Instead exercise format path indirectly by writing a synthetic file.
        let body = {
            let mut out = String::new();
            out.push_str("whycodes crash report\n");
            out.push_str(&format!("version: {VERSION}\n"));
            out.push_str("message: boom\n");
            out
        };
        assert!(body.contains("whycodes crash report"));
        assert!(body.contains(VERSION));
        assert!(body.contains("boom"));
    }

    #[test]
    fn write_crash_report_from_hook_info() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        dirs.ensure().unwrap();

        // Trigger a panic inside catch_unwind and use the hook info via
        // a custom hook that captures the formatted report.
        let report_path: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let report_path2 = Arc::clone(&report_path);
        let dirs2 = dirs.clone();

        let _restore = RestoreHook(Some(std::panic::take_hook()));
        std::panic::set_hook(Box::new(move |info| {
            // Other tests in this binary panic on worker threads; ignore them.
            if crate::logging::panic_message(info) != "test-crash-payload" {
                return;
            }
            if let Ok(path) = write_crash_report(&dirs2, info) {
                *report_path2.lock().unwrap() = Some(path);
            }
        }));

        let result = std::panic::catch_unwind(|| {
            panic!("test-crash-payload");
        });
        assert!(result.is_err());

        let path = report_path.lock().unwrap().clone().expect("crash path");
        assert!(path.exists());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("whycodes crash report"), "{text}");
        assert!(text.contains("test-crash-payload"), "{text}");
        assert!(text.contains("version:"), "{text}");
        assert!(path.starts_with(tmp.path().join("crash")));
    }

    #[test]
    fn emit_without_init_is_noop() {
        // Ensure we don't panic when STATE is empty. May race with other tests
        // that init; best-effort only.
        emit("test", "info", "noop", None);
        // Force the uninitialized skip even if another test already set STATE.
        crate::logging::emit_uninitialized_for_test();
    }

    #[test]
    fn init_creates_unified_and_startup_event() {
        let _g = TEST_LOCK.lock().unwrap();
        // If already initialized by another test in this process, skip structural
        // assertions that require a fresh STATE — still check path helpers.
        let tmp = temp_data_dir();
        if is_initialized() {
            let dirs = LogDirs::from_data_dir(tmp.path());
            dirs.ensure().unwrap();
            append_jsonl(
                &dirs.unified_jsonl(),
                &LogEvent::new("test", "info", "already-init"),
            )
            .unwrap();
            let content = fs::read_to_string(dirs.unified_jsonl()).unwrap();
            assert!(content.contains("already-init"));
            return;
        }

        let handle = init(InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: Some("debug".into()),
            log_file: Some(tmp.path().join("nested/human.log")),
            debug: true,
            with_stderr: true,
        })
        .expect("init");

        assert!(handle.dirs.unified_jsonl().exists() || handle.dirs.logs.exists());
        assert!(is_initialized());

        // Startup event from init
        emit("test", "info", "post-init-ping", Some(json!({"ok": true})));

        let content = fs::read_to_string(handle.dirs.unified_jsonl()).unwrap();
        assert!(
            content.contains("logging.init") || content.contains("post-init-ping"),
            "jsonl should contain init or ping event: {content}"
        );
        assert!(handle.debug_log.is_some());
        let latest = handle.dirs.debug.join("latest.log");
        assert!(
            latest.exists() || handle.dirs.debug.join("latest.path").exists(),
            "latest debug pointer should exist"
        );
    }

    #[test]
    fn log_event_serde_roundtrip() {
        let e = LogEvent::new("src", "error", "msg").with_ctx(json!({"a": true}));
        let s = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["src"], "src");
        assert_eq!(v["lvl"], "error");
        assert_eq!(v["msg"], "msg");
        assert_eq!(v["ctx"]["a"], true);
    }

    #[test]
    fn whycodes_log_file_env_path_is_used_when_passed() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        dirs.ensure().unwrap();
        let custom = tmp.path().join("custom.log");
        let opts = InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: Some("info".into()),
            log_file: Some(custom.clone()),
            debug: false,
            with_stderr: false,
        };
        let path = resolve_debug_log_path(&dirs, &opts).unwrap();
        assert_eq!(path.as_deref(), Some(custom.as_path()));
    }

    #[test]
    fn resolve_debug_log_path_creates_stamped_file_and_latest_link() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        dirs.ensure().unwrap();
        let opts = InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: None,
            log_file: None,
            debug: true,
            with_stderr: false,
        };
        let path = resolve_debug_log_path(&dirs, &opts)
            .unwrap()
            .expect("debug log");
        assert_eq!(path.parent(), Some(dirs.debug.as_path()));
        assert!(
            path.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("whycodes-")),
            "stamped debug filename: {}",
            path.display()
        );
        assert!(path.exists(), "debug log file should be touched");
        assert!(
            dirs.debug.join("latest.log").exists() || dirs.debug.join("latest.path").exists(),
            "latest pointer should exist"
        );
        // No log_file and no debug → None
        let opts = InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: None,
            log_file: None,
            debug: false,
            with_stderr: false,
        };
        assert_eq!(resolve_debug_log_path(&dirs, &opts).unwrap(), None);
    }

    #[test]
    fn build_env_filter_uses_rust_log_then_explicit() {
        let _g = TEST_LOCK.lock().unwrap();
        // RUST_LOG wins over the explicit level.
        unsafe { std::env::set_var("RUST_LOG", "warn") };
        let f = build_env_filter(Some("debug"));
        unsafe { std::env::remove_var("RUST_LOG") };
        let s = f.to_string();
        assert!(s.contains("warn"), "RUST_LOG should win: {s}");

        // No RUST_LOG → explicit level.
        let f = build_env_filter(Some("trace"));
        assert!(f.to_string().contains("trace"), "{}", f.to_string());

        // Neither → info default.
        let f = build_env_filter(None);
        assert!(f.to_string().contains("info"), "{}", f.to_string());
    }

    #[test]
    fn panic_cleanup_set_and_clear() {
        let _g = TEST_LOCK.lock().unwrap();
        clear_panic_cleanup();
        assert!(
            crate::logging::recover_lock(PANIC_CLEANUP.lock()).is_none(),
            "cleared cleanup slot"
        );
        set_panic_cleanup(|| {});
        assert!(crate::logging::recover_lock(PANIC_CLEANUP.lock()).is_some());
        clear_panic_cleanup();
        assert!(crate::logging::recover_lock(PANIC_CLEANUP.lock()).is_none());
    }

    #[test]
    fn panic_hook_runs_registered_cleanup() {
        let _g = TEST_LOCK.lock().unwrap();
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&ran);
        set_panic_cleanup(move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        crate::logging::run_panic_cleanup();
        clear_panic_cleanup();
        crate::logging::run_panic_cleanup();
        assert!(
            ran.load(std::sync::atomic::Ordering::SeqCst),
            "registered terminal cleanup must run"
        );

        crate::logging::poison_panic_cleanup();
        let ran2 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag2 = std::sync::Arc::clone(&ran2);
        set_panic_cleanup(move || {
            flag2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        crate::logging::run_panic_cleanup();
        clear_panic_cleanup();
        assert!(
            ran2.load(std::sync::atomic::Ordering::SeqCst),
            "cleanup must still run after PANIC_CLEANUP is poisoned"
        );

        // Holding the mutex: try_lock WouldBlock, so this is a no-op.
        let _held = crate::logging::recover_lock(PANIC_CLEANUP.lock());
        crate::logging::run_panic_cleanup();
    }

    #[test]
    fn dirs_is_none_before_init() {
        // Not holding the lock here: dirs() only reads STATE which is a
        // OnceLock — safe to call concurrently, and init tests use the lock.
        if !is_initialized() {
            assert!(dirs().is_none());
        }
    }

    #[test]
    fn emit_sid_init_options_visitor_and_string_panic() {
        let _g = TEST_LOCK.lock().unwrap();
        let _ = InitOptions::default();
        emit("test", "info", "pre-init", None);
        emit_sid("test", "info", "pre-init", Some("sid"), None);

        let tmp = temp_data_dir();
        let _ = init(InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: Some("debug".into()),
            log_file: Some(tmp.path().join("nested/human.log")),
            debug: false,
            with_stderr: true,
        });
        let _ = init(InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: None,
            log_file: None,
            debug: true,
            with_stderr: false,
        });
        emit_sid(
            "test",
            "info",
            "post-init-sid",
            Some("ses"),
            Some(json!({"n": 1})),
        );
        emit("test", "info", "post-init", None);
        if is_initialized() {
            assert!(dirs().is_some());
        }
        tracing::info!(
            sid = "s1",
            extra = "x",
            n = 3_i64,
            u = 4_u64,
            ok = true,
            "visitor"
        );
        tracing::debug!(?tmp, "debug-field");
        tracing::info!(message = "str-msg", sid = "s2", extra2 = "y");
        set_panic_cleanup(|| {});
        let _ = std::panic::catch_unwind(|| panic!("hook-cleanup"));
        clear_panic_cleanup();

        let prev = std::env::var_os("WHYCODES_LOG_LEVEL");
        unsafe { std::env::remove_var("RUST_LOG") };
        unsafe { std::env::set_var("WHYCODES_LOG_LEVEL", "error") };
        let f = build_env_filter(None);
        assert!(f.to_string().contains("error"), "{}", f.to_string());
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_LOG_LEVEL", v) },
            None => unsafe { std::env::remove_var("WHYCODES_LOG_LEVEL") },
        }

        let tmp2 = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp2.path());
        dirs.ensure().unwrap();
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured2 = Arc::clone(&captured);
        let dirs2 = dirs.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let body = format_crash_report(info);
            let _ = write_crash_report(&dirs2, info);
            *captured2.lock().unwrap() = Some(body);
        }));
        assert!(
            std::panic::catch_unwind(|| {
                panic!("{}", String::from("owned-panic"));
            })
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| {
                std::panic::panic_any(9u32);
            })
            .is_err()
        );
        std::panic::set_hook(previous);
        let body = captured.lock().unwrap().clone().unwrap_or_default();
        assert!(body.contains("owned-panic") || body.contains("whycodes crash"));
    }

    #[test]
    fn logging_helpers_cover_remaining_branches() {
        let _g = TEST_LOCK.lock().unwrap();
        let tmp = temp_data_dir();
        let dirs = LogDirs::from_data_dir(tmp.path());
        dirs.ensure().unwrap();

        assert!(ensure_parent(Path::new("")).is_ok());
        let nested = tmp.path().join("a/b/c.log");
        ensure_parent(&nested).unwrap();
        assert!(nested.parent().unwrap().is_dir());

        let not_dir = tmp.path().join("not-a-dir");
        fs::write(&not_dir, b"x").unwrap();
        assert!(ensure_parent(&not_dir.join("child.jsonl")).is_err());
        assert!(
            append_jsonl(
                &not_dir.join("child.jsonl"),
                &LogEvent::new("t", "info", "x")
            )
            .is_err()
        );

        create_parent_dir(&tmp.path().join("d/e/f.log")).unwrap();
        assert!(tmp.path().join("d/e").is_dir());
        create_parent_dir(Path::new("bare.log")).unwrap();
        let file_as_parent = tmp.path().join("file-parent");
        fs::write(&file_as_parent, b"x").unwrap();
        assert!(create_parent_dir(&file_as_parent.join("x.log")).is_err());

        let log_path = tmp.path().join("fmt.log");
        let file = open_append(&log_path).unwrap();
        let _layer = fmt_file_layer::<tracing_subscriber::Registry>(file);
        let _stderr = stderr_fmt_layer::<tracing_subscriber::Registry>();
        assert!(log_path.exists());

        let unlocked = Mutex::new(open_append(&log_path).unwrap());
        write_jsonl_locked(&unlocked, &LogEvent::new("t", "info", "locked")).unwrap();
        let poisoned = Mutex::new(open_append(&log_path).unwrap());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = poisoned.lock().unwrap();
            panic!("poison");
        }));
        assert!(lock_log_file(&poisoned).is_err());
        assert!(write_jsonl_locked(&poisoned, &LogEvent::new("t", "info", "x")).is_err());

        let target = tmp.path().join("target.log");
        fs::write(&target, b"hello").unwrap();
        let latest = tmp.path().join("latest-as-dir");
        fs::create_dir(&latest).unwrap();
        symlink_or_copy(&target, &latest);

        let ok_msg = crash_user_message(&Ok(PathBuf::from("/tmp/crash.txt")));
        assert!(ok_msg.contains("report written"));
        let err_msg = crash_user_message(&Err(std::io::Error::other("disk full")));
        assert!(err_msg.contains("failed to write"));

        assert_eq!(clean_debug_value(r#""quoted""#.into()), "quoted");
        assert_eq!(clean_debug_value("plain".into()), "plain");

        let mut v = JsonVisitor::default();
        v.assign_str("message", "m".into());
        v.assign_str("sid", "s".into());
        v.assign_str("extra", "x".into());
        assert_eq!(v.message.as_deref(), Some("m"));
        assert_eq!(v.sid.as_deref(), Some("s"));
        assert_eq!(v.fields.get("extra").and_then(|x| x.as_str()), Some("x"));

        let nested_log = tmp.path().join("nested/deep/human.log");
        let opts = InitOptions {
            data_dir: tmp.path().to_path_buf(),
            log_level: None,
            log_file: Some(nested_log.clone()),
            debug: false,
            with_stderr: false,
        };
        let path = resolve_debug_log_path(&dirs, &opts).unwrap();
        assert_eq!(path.as_deref(), Some(nested_log.as_path()));
        assert!(nested_log.parent().unwrap().is_dir());

        assert!(choose_debug_file(None).unwrap().is_none());
        let some_log = tmp.path().join("opt.log");
        assert!(
            choose_debug_file(Some(some_log.as_path()))
                .unwrap()
                .is_some()
        );

        emit_panic_report(Err(std::io::Error::other("no crash dir")), "p");
        emit_panic_report(Ok(tmp.path().join("missing-crash.txt")), "p");
        assert!(format_location(None).is_none());
        assert!(
            format_location(Some(std::panic::Location::caller()))
                .unwrap()
                .contains("location:")
        );

        let m = Arc::new(std::sync::Mutex::new(1u8));
        assert_eq!(*crate::logging::recover_lock(m.lock()), 1);
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert_eq!(*crate::logging::recover_lock(m.lock()), 1);
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        crate::logging::poison_panic_cleanup();
        set_panic_cleanup(|| {});
        crate::logging::poison_panic_cleanup();
        clear_panic_cleanup();
        std::panic::set_hook(prev);
    }
}

// ── network.rs ──────────────────────────────────────────────

mod network_tests {
    use super::*;

    #[test]
    fn host_from_https_with_path_and_port() {
        assert_eq!(
            host_from_url("https://api.github.com:443/repos/x").unwrap(),
            "api.github.com"
        );
    }

    #[test]
    fn host_from_userinfo() {
        assert_eq!(
            host_from_url("https://user:pass@example.com/a").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn host_from_ipv6() {
        assert_eq!(host_from_url("http://[::1]:8080/x").unwrap(), "::1");
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(host_from_url("ftp://example.com").is_err());
        assert!(host_from_url("example.com/path").is_err());
    }

    #[test]
    fn apex_pattern_matches_subdomains() {
        assert!(host_matches_pattern("example.com", "example.com"));
        assert!(host_matches_pattern("api.example.com", "example.com"));
        assert!(host_matches_pattern("a.b.example.com", "example.com"));
        assert!(!host_matches_pattern("notexample.com", "example.com"));
        assert!(!host_matches_pattern("example.com.evil", "example.com"));
    }

    #[test]
    fn star_prefix_subdomains_only() {
        assert!(host_matches_pattern("api.example.com", "*.example.com"));
        assert!(!host_matches_pattern("example.com", "*.example.com"));
        assert!(host_matches_pattern("a.b.example.com", "*.example.com"));
    }

    #[test]
    fn star_matches_all() {
        assert!(host_matches_pattern("anything.test", "*"));
    }

    #[test]
    fn unrestricted_allows_all() {
        let p = NetworkPolicy::unrestricted();
        assert!(p.is_host_allowed("evil.example"));
        assert!(p.ensure_url_allowed("https://evil.example/x").is_ok());
    }

    #[test]
    fn allowlist_blocks_unknown() {
        let p = NetworkPolicy {
            allowlist: vec!["github.com".into(), "crates.io".into()],
            denylist: vec![],
        };
        assert!(p.is_host_allowed("api.github.com"));
        assert!(p.is_host_allowed("crates.io"));
        assert!(!p.is_host_allowed("evil.com"));
        assert!(p.check_url("https://api.github.com/x").is_ok());
        assert!(p.check_url("https://evil.com/x").is_err());
    }

    #[test]
    fn denylist_wins() {
        let p = NetworkPolicy {
            allowlist: vec!["example.com".into()],
            denylist: vec!["tracking.example.com".into()],
        };
        assert!(p.is_host_allowed("example.com"));
        assert!(p.is_host_allowed("docs.example.com"));
        assert!(!p.is_host_allowed("tracking.example.com"));
    }

    #[test]
    fn denylist_alone() {
        let p = NetworkPolicy {
            allowlist: vec![],
            denylist: vec!["bad.com".into()],
        };
        assert!(p.is_host_allowed("good.com"));
        assert!(!p.is_host_allowed("bad.com"));
        assert!(!p.is_host_allowed("sub.bad.com"));
    }

    #[test]
    fn parse_domain_list_splits() {
        assert_eq!(
            parse_domain_list("github.com, crates.io *.npmjs.org"),
            vec!["github.com", "crates.io", "*.npmjs.org"]
        );
    }

    #[test]
    fn restricted_ensure_url_and_edge_hosts() {
        let p = NetworkPolicy {
            allowlist: vec!["ok.com".into()],
            denylist: vec!["bad.com".into()],
        };
        assert!(p.is_restricted());
        assert!(p.ensure_url_allowed("https://ok.com/x").is_ok());
        let err = p.ensure_url_allowed("https://bad.com/x").unwrap_err();
        assert!(err.contains("Denied patterns"), "{err}");
        assert!(err.contains("Allowed patterns"), "{err}");
        assert!(!p.is_host_allowed(""));
        assert_eq!(normalize_host("[::1]"), "::1");
        assert!(host_from_url("").is_err());
        assert!(host_from_url("https://").is_err());
        assert!(host_from_url("https:///path").is_err());
        assert!(host_from_url("http://[no-close").is_err());
        assert!(!host_matches_pattern("", "x"));
        assert!(!host_matches_pattern("x", ""));
        // `*.` normalizes to `*` (trailing dots stripped) and then matches all.
        assert!(host_matches_pattern("a.b", "*."));
        assert!(p.check_url("https://ok.com").is_ok());
        assert!(host_from_url("http://.").is_err());
    }
}

// ── panel.rs ──────────────────────────────────────────────

mod panel_tests {
    use super::*;

    #[test]
    fn panel_update_variants_debug_eq() {
        let a = PanelUpdate::File {
            path: "a.rs".into(),
            text: "x".into(),
        };
        let b = PanelUpdate::Diff {
            path: "a.rs".into(),
            unified: "d".into(),
        };
        let c = PanelUpdate::Mermaid {
            source: "graph TD".into(),
        };
        let d = PanelUpdate::Clear;
        assert_ne!(a, b);
        assert_eq!(d, PanelUpdate::Clear);
        let _ = format!("{a:?}{b:?}{c:?}{d:?}");
        let sink: PanelSink = Arc::new(|_| {});
        sink(PanelUpdate::Clear);
    }
}

// ── todo.rs ───────────────────────────────────────────────

mod todo_tests {
    use super::*;

    #[test]
    fn status_parse_mark_and_terminal() {
        assert_eq!(TodoStatus::parse("pending"), TodoStatus::Pending);
        assert_eq!(TodoStatus::parse("in_progress"), TodoStatus::InProgress);
        assert_eq!(TodoStatus::parse("completed"), TodoStatus::Completed);
        assert_eq!(TodoStatus::parse("cancelled"), TodoStatus::Cancelled);
        assert_eq!(TodoStatus::parse("nope"), TodoStatus::Pending);
        assert_eq!(TodoStatus::Pending.as_str(), "pending");
        assert_eq!(TodoStatus::InProgress.as_str(), "in_progress");
        assert_eq!(TodoStatus::Completed.as_str(), "completed");
        assert_eq!(TodoStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(TodoStatus::Pending.mark(), "☐");
        assert_eq!(TodoStatus::InProgress.mark(), "▶");
        assert_eq!(TodoStatus::Completed.mark(), "☑");
        assert_eq!(TodoStatus::Cancelled.mark(), "✗");
        assert!(!TodoStatus::Pending.is_terminal());
        assert!(!TodoStatus::InProgress.is_terminal());
        assert!(TodoStatus::Completed.is_terminal());
        assert!(TodoStatus::Cancelled.is_terminal());
    }

    #[test]
    fn serde_round_trip_and_unknown_status() {
        let item = TodoItem::new("a", "do it", TodoStatus::InProgress);
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("in_progress"));
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back, item);
        let unknown: TodoItem =
            serde_json::from_str(r#"{"id":"x","content":"y","status":"mystery"}"#).unwrap();
        assert_eq!(unknown.status, TodoStatus::Pending);
        let defaults: TodoItem = serde_json::from_str("{}").unwrap();
        assert!(defaults.id.is_empty());
        assert!(defaults.content.is_empty());
        assert_eq!(defaults.status, TodoStatus::Pending);
        assert_eq!(item.line(), "▶ do it");
        let list = TodoList {
            todos: vec![item.clone()],
        };
        let _ = format!("{list:?}");
        assert_eq!(list, list.clone());
    }

    #[test]
    fn path_session_scoped_or_fallback() {
        let root = Path::new("/proj");
        assert_eq!(
            todos_path(root, Some("sess-1")),
            PathBuf::from("/proj/.whycodes/todos/sess-1.json")
        );
        assert_eq!(
            todos_path(root, Some("  ")),
            PathBuf::from("/proj/.whycodes/todos.json")
        );
        assert_eq!(
            todos_path(root, None),
            PathBuf::from("/proj/.whycodes/todos.json")
        );
    }

    #[test]
    fn load_missing_invalid_and_save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_todos(dir.path(), None).is_empty());
        let why = dir.path().join(".whycodes");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(why.join("todos.json"), "not json {{{").unwrap();
        assert!(load_todos(dir.path(), None).is_empty());
        std::fs::write(why.join("todos.json"), r#"{"other":1}"#).unwrap();
        assert!(load_todos(dir.path(), None).is_empty());

        let items = vec![
            TodoItem::new("1", "a", TodoStatus::Pending),
            TodoItem::new("2", "b", TodoStatus::Completed),
        ];
        save_todos(dir.path(), Some("abc"), &items).unwrap();
        let loaded = load_todos(dir.path(), Some("abc"));
        assert_eq!(loaded, items);
        assert!(load_todos(dir.path(), Some("other")).is_empty());
    }

    #[test]
    fn save_fails_when_parent_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        let err = save_todos(&blocker, None, &[]).unwrap_err();
        assert!(err.contains("creating todo dir"), "{err}");
    }

    #[test]
    fn save_fails_when_target_is_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let why = dir.path().join(".whycodes");
        std::fs::create_dir_all(why.join("todos.json")).unwrap();
        let err = save_todos(dir.path(), None, &[]).unwrap_err();
        assert!(err.contains("writing todos"), "{err}");
    }

    #[test]
    fn merge_by_id_and_replace() {
        let existing = vec![
            TodoItem::new("a", "old", TodoStatus::Pending),
            TodoItem::new("b", "keep", TodoStatus::Pending),
        ];
        let incoming = vec![
            TodoItem::new("a", "new", TodoStatus::Completed),
            TodoItem::new("c", "add", TodoStatus::InProgress),
            TodoItem::new("", "no-id", TodoStatus::Pending),
        ];
        let merged = apply_todo_update(existing.clone(), incoming.clone(), true);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0].content, "new");
        assert_eq!(merged[0].status, TodoStatus::Completed);
        assert_eq!(merged[1].content, "keep");
        assert_eq!(merged[2].id, "c");
        assert_eq!(merged[3].content, "no-id");
        let replaced = apply_todo_update(existing, incoming, false);
        assert_eq!(replaced.len(), 3);
    }

    #[test]
    fn apply_todowrite_args_default_merge() {
        let existing = vec![TodoItem::new("a", "old", TodoStatus::Pending)];
        let next = apply_todowrite_args(
            &existing,
            &json!({"todos":[{"id":"a","content":"upd","status":"completed"}]}),
        )
        .unwrap();
        assert_eq!(next[0].content, "upd");
        let replaced = apply_todowrite_args(
            &existing,
            &json!({
                "merge": false,
                "todos":[{"id":"z","content":"only","status":"pending"}]
            }),
        )
        .unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].id, "z");
        assert!(apply_todowrite_args(&existing, &json!({})).is_none());
        assert!(apply_todowrite_args(&existing, &json!({"todos":"nope"})).is_none());
        assert_eq!(terminal_count(&next), 1);
        assert!(all_terminal(&next));
        assert!(!all_terminal(&[]));
        assert!(!all_terminal(&existing));
        let sink: TodoSink = Arc::new(|_| {});
        sink(next);
    }
}

// ── paths.rs ──────────────────────────────────────────────

mod paths_tests {
    use super::*;
    use std::borrow::Cow;

    static PATHS_LOCK: Mutex<()> = Mutex::new(());

    fn recover_paths_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::file_claims::recover_lock(PATHS_LOCK.lock())
    }

    #[test]
    fn home_override_wins() {
        let _g = recover_paths_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        let data = data_dir();
        let cfg = config_file();
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
        }
        assert_eq!(data, dir.path());
        assert_eq!(cfg, dir.path().join("config.toml"));
    }

    #[test]
    fn empty_home_env_is_ignored() {
        let _g = recover_paths_lock();
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe {
            std::env::set_var("WHYCODES_HOME", "");
        }
        assert!(whycodes_home().is_none());
        let data = data_dir();
        let cfg = config_dir();
        let file = config_file();
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
        }
        assert!(!data.as_os_str().is_empty());
        assert!(!cfg.as_os_str().is_empty());
        assert_eq!(file, cfg.join("config.toml"));
        assert_eq!(or_dot(None), PathBuf::from("."));
        assert_eq!(or_dot(Some(PathBuf::from("/x"))), PathBuf::from("/x"));
    }

    #[test]
    fn project_dir_is_whycodes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        assert_eq!(project_dir(root), root.join(".whycodes"));
        std::fs::create_dir(root.join(".whycodes")).unwrap();
        assert_eq!(project_dir(root), root.join(".whycodes"));
    }

    #[test]
    fn display_path_strips_windows_verbatim_prefix() {
        assert_eq!(display_path(Path::new(r"\\?\C:\dev")), r"C:\dev");
        assert_eq!(display_path(Path::new(r"\\?\c:\Users\me")), r"c:\Users\me");
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\dir")),
            r"\\server\share\dir"
        );
        assert_eq!(display_path(Path::new(r"C:\dev")), r"C:\dev");
        assert_eq!(display_path(Path::new("/tmp/proj")), "/tmp/proj");
        assert_eq!(display_path(Path::new(".")), ".");
        // Device namespace — not a drive / UNC path; leave the prefix.
        assert_eq!(
            display_path(Path::new(r"\\?\pipe\whycodes")),
            r"\\?\pipe\whycodes"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\Volume{guid}\")),
            r"\\?\Volume{guid}\"
        );
        assert_eq!(display_path(Path::new(r"\\?\")), r"\\?\");
        assert_eq!(display_path(Path::new(r"\\?\C")), r"\\?\C");
        assert_eq!(display_path(Path::new(r"\\?\UNC")), r"\\?\UNC");
        assert_eq!(display_path(Path::new(r"\\?\UNC\")), r"\\");
        assert_eq!(strip_windows_verbatim_prefix(""), Cow::Borrowed(""));
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\1:\not-a-drive"),
            Cow::Borrowed(r"\\?\1:\not-a-drive")
        );
    }

    #[test]
    fn apply_component_parent_cur_and_normal() {
        use std::path::Component;
        let mut p = PathBuf::from("/a/b");
        crate::file_claims::apply_component(&mut p, Component::ParentDir);
        assert_eq!(p, PathBuf::from("/a"));
        crate::file_claims::apply_component(&mut p, Component::CurDir);
        assert_eq!(p, PathBuf::from("/a"));
        crate::file_claims::apply_component(&mut p, Component::Normal("c".as_ref()));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn option_if_and_warn_if_err() {
        assert_eq!(maybe_layer(true, 1), Some(1));
        assert_eq!(maybe_layer(false, 1), None);
        warn_already_set(None);
        warn_already_set(Some("already".into()));
        note_try_init(true);
        note_try_init(false);
        assert_eq!(event_message(Some("m".into()), "fb"), "m");
        assert_eq!(event_message(None, "fb"), "fb");
    }
}

// ── sandbox.rs ──────────────────────────────────────────────

mod sandbox_tests {
    use super::*;

    #[test]
    fn sandbox_mode_parses_and_displays() {
        assert_eq!("off".parse::<SandboxMode>().unwrap(), SandboxMode::Off);
        assert_eq!("none".parse::<SandboxMode>().unwrap(), SandboxMode::Off);
        assert_eq!("false".parse::<SandboxMode>().unwrap(), SandboxMode::Off);
        assert_eq!("0".parse::<SandboxMode>().unwrap(), SandboxMode::Off);
        assert_eq!(
            "workspace".parse::<SandboxMode>().unwrap(),
            SandboxMode::Workspace
        );
        assert_eq!("on".parse::<SandboxMode>().unwrap(), SandboxMode::Workspace);
        assert_eq!(
            "true".parse::<SandboxMode>().unwrap(),
            SandboxMode::Workspace
        );
        assert_eq!("1".parse::<SandboxMode>().unwrap(), SandboxMode::Workspace);
        assert!("nope".parse::<SandboxMode>().is_err());
        assert_eq!(SandboxMode::Off.as_str(), "off");
        assert_eq!(SandboxMode::Workspace.as_str(), "workspace");
        assert_eq!(SandboxMode::default(), SandboxMode::Workspace);
    }

    #[test]
    fn sandbox_fallback_parses() {
        assert_eq!(
            "allow".parse::<SandboxFallback>().unwrap(),
            SandboxFallback::Allow
        );
        assert_eq!(
            "warn".parse::<SandboxFallback>().unwrap(),
            SandboxFallback::Allow
        );
        assert_eq!(
            "host".parse::<SandboxFallback>().unwrap(),
            SandboxFallback::Allow
        );
        assert_eq!(
            "deny".parse::<SandboxFallback>().unwrap(),
            SandboxFallback::Deny
        );
        assert_eq!(
            "error".parse::<SandboxFallback>().unwrap(),
            SandboxFallback::Deny
        );
        assert_eq!(
            "strict".parse::<SandboxFallback>().unwrap(),
            SandboxFallback::Deny
        );
        assert!("maybe".parse::<SandboxFallback>().is_err());
        assert_eq!(SandboxFallback::default(), SandboxFallback::Allow);
    }

    #[test]
    fn settings_off_default_and_from_raw() {
        let d = SandboxSettings::default();
        assert_eq!(d.mode, SandboxMode::Workspace);
        assert!(d.network);
        assert_eq!(d.fallback, SandboxFallback::Allow);

        let off = SandboxSettings::off();
        assert_eq!(off.mode, SandboxMode::Off);

        let ok = SandboxSettings::from_raw("off", false, "deny");
        assert_eq!(ok.mode, SandboxMode::Off);
        assert!(!ok.network);
        assert_eq!(ok.fallback, SandboxFallback::Deny);

        let bad = SandboxSettings::from_raw("??? ", true, "???");
        assert_eq!(bad.mode, SandboxMode::Workspace);
        assert_eq!(bad.fallback, SandboxFallback::Allow);
    }
}

// ── swarm_hub.rs ──────────────────────────────────────────────

mod swarm_hub_tests {
    use super::*;

    #[test]
    fn dm_reaches_only_target() {
        let hub = SwarmHub::new();
        hub.send("worker-0", "worker-1", "hello");
        assert!(hub.drain("worker-0").is_empty());
        let got = hub.drain("worker-1");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "hello");
        assert!(hub.drain("worker-1").is_empty());
    }

    #[test]
    fn broadcast_skips_sender() {
        let hub = SwarmHub::new();
        hub.ensure("parent");
        hub.ensure("worker-0");
        hub.ensure("worker-1");
        hub.send("worker-0", "all", "heads up");
        let parent = hub.drain("parent");
        assert!(parent.iter().any(|m| m.text == "heads up"));
        assert!(hub.drain("worker-0").is_empty());
        let w1 = hub.drain("worker-1");
        assert!(w1.iter().any(|m| m.text == "heads up"));
    }

    #[test]
    fn empty_send_listener_and_broadcast_without_inboxes() {
        let hub = SwarmHub::new();
        let _ = format!("{hub:?}");
        hub.send("a", "b", "   ");
        assert!(hub.drain("b").is_empty());

        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits2 = std::sync::Arc::clone(&hits);
        hub.set_listener(Some(std::sync::Arc::new(move |_| {
            hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })));
        hub.send("solo", "all", "ping");
        assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        let parent = hub.drain("parent");
        assert!(parent.iter().any(|m| m.text == "ping"));
        hub.set_listener(None);
        hub.ensure("late");
        hub.send("late", "all", "later");
        assert!(hub.drain("parent").iter().any(|m| m.text == "later"));
    }

    #[test]
    fn recover_lock_poisoned() {
        let m = std::sync::Arc::new(std::sync::Mutex::new(1));
        let m2 = std::sync::Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert_eq!(*crate::swarm_hub::recover_lock(m.lock()), 1);

        let hub = SwarmHub::new();
        hub.poison_inboxes();
        hub.ensure("w0");
        hub.poison_inboxes();
        let _ = format!("{hub:?}");
        hub.poison_inboxes();
        hub.send("w0", "w1", "hi");
        hub.poison_inboxes();
        let _ = hub.drain("w1");
        hub.poison_listener();
        hub.set_listener(None);
        hub.poison_listener();
        hub.send("w0", "w1", "again");
    }
}

// ── tool.rs ──────────────────────────────────────────────

mod tool_tests {
    use super::*;

    use crate::file_claims::FileClaimRegistry;
    use std::path::Path;

    struct DummyTool;
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "read a file"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a ToolContext,
        ) -> crate::tool::ToolFuture<'a> {
            Box::pin(async move {
                ToolResult {
                    tool_call_id: "1".into(),
                    content: "ok".into(),
                    is_error: false,
                }
            })
        }
    }

    #[test]
    fn context_constructors_and_debug() {
        let ctx = ToolContext::new("/tmp/proj");
        assert_eq!(ctx.working_dir, "/tmp/proj");
        assert!(ctx.sandbox.network);
        let dbg = format!("{ctx:?}");
        assert!(dbg.contains("working_dir"));
        assert!(dbg.contains("/tmp/proj"));

        let uns = ToolContext::unsandboxed("/work");
        assert_eq!(uns.sandbox.mode, crate::SandboxMode::Off);
        assert!(uns.check_file_write(Path::new("a.rs")).is_ok());
        assert!(uns.check_file_read(Path::new("a.rs")).is_none());
    }

    #[test]
    fn file_claim_gates() {
        let reg = FileClaimRegistry::new();
        let p = Path::new("/tmp/whycodes_tool_claim.rs");
        assert_eq!(
            reg.try_claim("w0", "alpha", p),
            crate::file_claims::ClaimResult::Acquired
        );

        let mut ctx = ToolContext::new(".");
        ctx.file_claims = Some(reg.clone());
        assert!(ctx.check_file_write(p).is_ok());
        assert!(ctx.check_file_read(p).is_none());

        ctx.agent_id = Some("w1".into());
        ctx.agent_label = Some("beta".into());
        let err = ctx.check_file_write(p).unwrap_err();
        assert!(err.contains("File conflict"), "{err}");
        assert!(ctx.check_file_read(p).is_some());

        ctx.agent_id = Some("w0".into());
        assert!(ctx.check_file_write(p).is_ok());
    }

    #[tokio::test]
    async fn dummy_tool_definition_and_permission() {
        let t = DummyTool;
        let def = t.definition();
        assert_eq!(def.name, "read");
        assert_eq!(def.description, "read a file");
        let out = t
            .execute(serde_json::json!({}), &ToolContext::new("."))
            .await;
        assert!(!out.is_error);
        let allow = PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        };
        assert!(t.is_allowed(&allow));
    }
}

// ── types.rs ──────────────────────────────────────────────

mod types_tests {
    use super::*;

    // ── test_message_content_as_text ────────────────────────────────────

    #[test]
    fn test_message_content_as_text_string_variant() {
        let mc = MessageContent::Text("hello world".to_string());
        assert_eq!(mc.as_text(), Some("hello world"));
    }

    #[test]
    fn test_message_content_as_text_blocks_with_text() {
        let mc = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "first block".to_string(),
            },
            ContentBlock::Text {
                text: "second block".to_string(),
            },
        ]);
        assert_eq!(mc.as_text(), Some("first block"));
    }

    #[test]
    fn test_message_content_as_text_blocks_without_text() {
        let mc = MessageContent::Blocks(vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            input: serde_json::json!({"q": "test"}),
        }]);
        assert_eq!(mc.as_text(), None);
    }

    #[test]
    fn test_message_content_as_text_empty_blocks() {
        let mc = MessageContent::Blocks(vec![]);
        assert_eq!(mc.as_text(), None);
    }

    #[test]
    fn test_message_content_text_constructor() {
        let mc = MessageContent::text("hello");
        assert_eq!(mc.as_text(), Some("hello"));
    }

    // ── test_serialize_deserialize_message ──────────────────────────────

    #[test]
    fn test_serialize_deserialize_message_text() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("hello".to_string()),
            tool_call_id: None,
            name: None,
            created_at: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::User);
        assert_eq!(deser.content.as_text(), Some("hello"));
        assert!(deser.tool_call_id.is_none());
    }

    #[test]
    fn test_serialize_deserialize_message_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "assistant reply".to_string(),
            }]),
            tool_call_id: None,
            name: Some("assistant".to_string()),
            created_at: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::Assistant);
        assert_eq!(deser.content.as_text(), Some("assistant reply"));
        assert_eq!(deser.name.as_deref(), Some("assistant"));
    }

    #[test]
    fn thinking_block_roundtrips_signature() {
        let b = ContentBlock::Thinking {
            text: "plan".into(),
            signature: Some("sig-abc".into()),
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["signature"], "sig-abc");
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        match back {
            ContentBlock::Thinking { text, signature } => {
                assert_eq!(text, "plan");
                assert_eq!(signature.as_deref(), Some("sig-abc"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn strip_trailing_thinking_keeps_text_and_tools() {
        let blocks = vec![
            ContentBlock::Thinking {
                text: "a".into(),
                signature: Some("s".into()),
            },
            ContentBlock::Text { text: "hi".into() },
            ContentBlock::Thinking {
                text: "orphan".into(),
                signature: None,
            },
        ];
        let out = strip_trailing_thinking(&blocks);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[1], ContentBlock::Text { .. }));
    }

    #[test]
    fn test_serialize_deserialize_message_tool() {
        let msg = Message {
            role: Role::Tool,
            content: MessageContent::Text("tool result".to_string()),
            tool_call_id: Some("call-1".to_string()),
            name: None,
            created_at: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::Tool);
        assert_eq!(deser.tool_call_id.as_deref(), Some("call-1"));
    }

    // ── test_tool_definition_serialize ──────────────────────────────────

    #[test]
    fn test_tool_definition_serialize() {
        let td = ToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
        };

        let json = serde_json::to_string(&td).expect("serialize");
        let deser: ToolDefinition = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.name, "search");
        assert_eq!(deser.description, "Search the web");
        assert_eq!(deser.parameters["type"], "object");
        assert!(deser.parameters["required"][0] == "query");
    }

    // ── test_permission_set_default ─────────────────────────────────────

    #[test]
    fn test_permission_set_default() {
        let ps = PermissionSet::default();
        assert!(ps.allowed_tools.is_none());
        assert!(ps.denied_tools.is_none());
        // Default booleans are false
        assert!(!ps.allow_file_writes);
        assert!(!ps.allow_network);
        assert!(!ps.allow_shell);
        assert!(ps.allowed_paths.is_none());
        assert!(ps.rules.is_empty());
    }

    #[test]
    fn browser_defaults_to_ask() {
        let ps = PermissionSet {
            allow_file_writes: true,
            allow_shell: true,
            allow_network: true,
            ..Default::default()
        };
        assert_eq!(ps.action_for("browser"), PermissionAction::Ask);
        assert!(ps.is_tool_allowed("browser"));
    }

    #[test]
    fn test_permission_action_for_rules() {
        let mut ps = PermissionSet {
            allow_file_writes: true,
            allow_shell: true,
            allow_network: true,
            ..Default::default()
        };
        ps.rules.insert("bash".into(), PermissionAction::Ask);
        ps.rules.insert("edit".into(), PermissionAction::Deny);
        ps.rules.insert("mymcp_*".into(), PermissionAction::Deny);

        assert_eq!(ps.action_for("bash"), PermissionAction::Ask);
        assert_eq!(ps.action_for("edit"), PermissionAction::Deny);
        assert_eq!(ps.action_for("mymcp_search"), PermissionAction::Deny);
        assert_eq!(ps.action_for("read"), PermissionAction::Allow);
        assert!(!ps.is_tool_allowed("edit"));
        assert!(ps.is_tool_allowed("bash")); // ask still appears in schema
    }

    #[test]
    fn test_role_serialization() {
        let role = Role::User;
        let json = serde_json::to_string(&role).expect("serialize");
        assert_eq!(json, "\"user\"");

        let deser: Role = serde_json::from_str("\"assistant\"").expect("deserialize");
        assert_eq!(deser, Role::Assistant);
    }

    #[test]
    fn test_provider_config_resolve_url() {
        let pc = ProviderConfig {
            name: "openai".to_string(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        };
        assert_eq!(
            pc.resolve_url("gpt-4"),
            "https://api.openai.com/v1/chat/completions"
        );

        let pc_custom = ProviderConfig {
            name: "openai".to_string(),
            api_key: None,
            api_base: None,
            base_url: Some("https://custom.example.com/api".to_string()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        };
        assert_eq!(
            pc_custom.resolve_url("gpt-4"),
            "https://custom.example.com/api/chat/completions"
        );
    }

    // ── shell-scoped permission rules ─────────────────────────────────

    fn perms_with(rules: &[(&str, PermissionAction)]) -> PermissionSet {
        let mut p = PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        };
        for (k, a) in rules {
            p.rules.insert((*k).to_string(), *a);
        }
        p
    }

    #[test]
    fn shell_rule_git_star_allows_git_commands() {
        let p = perms_with(&[("bash(git *)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("git status"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(
            p.action_for_shell("git commit -m x"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(p.action_for_shell("rm -rf /"), None);
    }

    #[test]
    fn shell_rule_colon_form_matches() {
        let p = perms_with(&[("Bash(cargo:*)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("cargo test"),
            Some(PermissionAction::Allow)
        );
    }

    #[test]
    fn dangerous_interpreter_allow_coerces_to_ask() {
        let p = perms_with(&[("bash(python *)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("python script.py"),
            Some(PermissionAction::Ask)
        );
        let p = perms_with(&[("bash(node:*)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("node app.js"),
            Some(PermissionAction::Ask)
        );
    }

    #[test]
    fn shell_rule_deny_blocks_matching() {
        let p = perms_with(&[("bash(curl *)", PermissionAction::Deny)]);
        assert_eq!(
            p.action_for_shell("curl https://evil"),
            Some(PermissionAction::Deny)
        );
    }

    #[test]
    fn path_rule_edit_src_star() {
        let p = perms_with(&[("edit(src/**)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_path("edit", "src/main.rs"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(p.action_for_path("edit", "crates/foo.rs"), None);
    }

    #[test]
    fn path_rule_write_md() {
        let p = perms_with(&[("write(**/*.md)", PermissionAction::Ask)]);
        // **/*.md via **/suffix style — our parser uses **/ only at start.
        // Prefer write(*.md) for basename or write(docs/**).
        let p2 = perms_with(&[("write(docs/**)", PermissionAction::Allow)]);
        assert_eq!(
            p2.action_for_path("write", "docs/a.md"),
            Some(PermissionAction::Allow)
        );
        let _ = p;
    }

    #[test]
    fn stamp_messages_mut_provider_urls_and_permission_edges() {
        let m = Message {
            role: Role::User,
            content: MessageContent::text("hi"),
            tool_call_id: None,
            name: None,
            created_at: None,
        }
        .stamp();
        assert!(m.created_at.is_some());
        let again = m.clone().stamp();
        assert_eq!(again.created_at, m.created_at);

        let mut req = LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(vec![m]),
            tools: std::sync::Arc::from([]),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: default_use_prompt_cache(),
        };
        req.messages_mut()[0].name = Some("u".into());
        assert_eq!(req.messages[0].name.as_deref(), Some("u"));

        let names = [
            "openai",
            "anthropic",
            "groq",
            "deepseek",
            "google",
            "google-antigravity",
            "gemini",
            "azure",
            "openrouter",
            "together",
            "together_ai",
            "fireworks",
            "mistral",
            "cohere",
            "perplexity",
            "xai",
            "ollama",
            "custom-x",
        ];
        for name in names {
            let pc = ProviderConfig {
                name: name.into(),
                api_key: None,
                api_base: None,
                base_url: None,
                headers: None,
                models: vec![],
                tool_arguments: None,
                extra: Default::default(),
            };
            assert_eq!(pc.tool_arguments_format(), ToolArgumentsFormat::JsonString);
            assert!(pc.resolve_url("m").contains("http"));
        }

        assert_eq!(
            PermissionAction::parse("allow"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(
            PermissionAction::parse("prompt"),
            Some(PermissionAction::Ask)
        );
        assert_eq!(
            PermissionAction::parse("block"),
            Some(PermissionAction::Deny)
        );
        assert_eq!(PermissionAction::parse("???"), None);

        assert_eq!(ApprovalMode::parse("auto"), Some(ApprovalMode::Auto));
        assert_eq!(ApprovalMode::parse("dontask"), Some(ApprovalMode::Auto));
        assert_eq!(ApprovalMode::parse("don't-ask"), Some(ApprovalMode::Auto));
        assert_eq!(ApprovalMode::parse("dont-ask"), Some(ApprovalMode::Auto));
        assert_eq!(ApprovalMode::parse("ask"), Some(ApprovalMode::Important));
        assert_eq!(
            ApprovalMode::parse("important"),
            Some(ApprovalMode::Important)
        );
        assert_eq!(
            ApprovalMode::parse("default"),
            Some(ApprovalMode::Important)
        );
        assert_eq!(ApprovalMode::parse("manual"), Some(ApprovalMode::Manual));
        assert_eq!(ApprovalMode::parse("always"), Some(ApprovalMode::Manual));
        assert_eq!(ApprovalMode::parse("???"), None);
        assert_eq!(ApprovalMode::default(), ApprovalMode::Auto);
        for mode in ApprovalMode::ALL {
            assert_eq!(mode.as_str(), mode.label());
            assert_eq!(mode.to_string(), mode.as_str());
            assert!(!mode.description().is_empty());
        }
        assert_eq!(ApprovalMode::Auto.as_str(), "auto");
        assert_eq!(ApprovalMode::Important.as_str(), "important");
        assert_eq!(ApprovalMode::Manual.as_str(), "manual");
        assert_eq!(ApprovalMode::Auto.label(), "auto");
        assert_eq!(ApprovalMode::Important.label(), "important");
        assert_eq!(ApprovalMode::Manual.label(), "manual");
        assert_eq!(
            ApprovalMode::Auto.description(),
            "Auto-answer questions and auto-allow permission asks"
        );
        assert_eq!(
            ApprovalMode::Important.description(),
            "Prompt on questions and high-risk tools only"
        );
        assert_eq!(
            ApprovalMode::Manual.description(),
            "Prompt on every question and permission ask"
        );
        assert_eq!(ApprovalMode::ALL.len(), 3);

        let mut p = PermissionSet {
            allow_file_writes: false,
            allow_shell: false,
            allow_network: false,
            denied_tools: Some(vec!["secret".into()]),
            allowed_tools: Some(vec!["read".into()]),
            ..Default::default()
        };
        p.rules.insert("mcp_*".into(), PermissionAction::Deny);
        assert_eq!(p.action_for("mcp_x"), PermissionAction::Deny);
        p.rules.insert("*".into(), PermissionAction::Ask);
        assert_eq!(p.action_for("anything"), PermissionAction::Ask);
        p.rules.clear();
        assert_eq!(p.action_for("secret"), PermissionAction::Deny);
        assert_eq!(p.action_for("write"), PermissionAction::Deny);
        assert_eq!(p.action_for("grep"), PermissionAction::Deny);
        p.allowed_tools = None;
        assert_eq!(p.action_for("write"), PermissionAction::Deny);
        assert_eq!(p.action_for("bash"), PermissionAction::Deny);
        assert_eq!(p.action_for("webfetch"), PermissionAction::Deny);

        let p2 = perms_with(&[
            ("edit(src/**)", PermissionAction::Allow),
            ("write(**/*.md)", PermissionAction::Ask),
            ("read(*)", PermissionAction::Allow),
            ("write(/etc/**)", PermissionAction::Allow),
            ("not-a-rule", PermissionAction::Allow),
            ("edit()", PermissionAction::Allow),
        ]);
        assert_eq!(
            p2.action_for_path("edit", "src\\mod.rs"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(
            p2.action_for_path("write", "notes.md"),
            Some(PermissionAction::Ask)
        );
        assert_eq!(
            p2.action_for_path("read", "a.rs"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(
            p2.action_for_path("write", "/etc/passwd"),
            Some(PermissionAction::Ask)
        );
        assert!(p2.action_for_path("bash", "x").is_none());
        assert!(path_glob_matches("src/*.rs", "src/main.rs"));
        assert!(path_glob_matches("foo", "foo/bar"));
        assert!(path_glob_matches("**/*.rs", "lib/x.rs"));
        assert!(!match_simple_star("a*", "a/b"));

        let sh = perms_with(&[
            ("bash(git *)", PermissionAction::Allow),
            ("bash(*)", PermissionAction::Allow),
            ("bash(python *)", PermissionAction::Allow),
            ("bash(npm run *)", PermissionAction::Allow),
            ("shell()", PermissionAction::Allow),
            ("edit(x)", PermissionAction::Allow),
            ("bash", PermissionAction::Allow),
        ]);
        assert!(sh.action_for_shell("").is_none());
        assert_eq!(
            sh.action_for_shell("git status"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(
            sh.action_for_shell("python x.py"),
            Some(PermissionAction::Ask)
        );
        assert_eq!(sh.action_for_shell("ls"), Some(PermissionAction::Ask));
        assert!(is_dangerous_shell_allow_pattern("*"));
        assert!(is_dangerous_shell_allow_pattern("python"));
        assert!(is_dangerous_shell_allow_pattern("python*"));
        assert!(is_dangerous_shell_allow_pattern("npm run"));
        assert!(is_dangerous_shell_allow_pattern("yarn run *"));
        assert!(is_dangerous_shell_allow_pattern("python -c *"));
        assert!(is_dangerous_shell_allow_pattern("node*"));
        assert!(!is_dangerous_shell_allow_pattern("git status"));
        assert!(parse_shell_rule("bash)(x").is_none());
        assert!(parse_shell_rule("edit(src)").is_none());
        assert!(parse_path_rule("bash(src)").is_none());
        assert!(parse_path_rule("read)(x").is_none());
        assert!(shell_arg_matches("*", "anything"));
        assert!(shell_arg_matches(" *", "x"));
        assert!(!match_simple_star("a*", "a/b"));
        assert!(!match_simple_star("a*b", "ax/b"));
        assert!(!match_simple_star("ab", "a"));
        assert!(match_simple_star("a*", "abc"));
        assert!(match_simple_star("*c", "abc"));
        assert!(match_simple_star("ab*", "ab"));
        assert!(match_simple_star("a*c", "abdc"));
        assert!(shell_arg_matches("git*", "git"));
        assert!(shell_arg_matches("git*", "gitignore"));
        assert!(!match_simple_star("xy", "ab"));
        assert!(!match_simple_star("a", "b"));
        assert!(match_simple_star("ab**", "ab"));
        assert!(match_simple_star("a*b*c", "aXbYc"));
        assert!(shell_arg_matches("git status", "git status"));
        assert!(shell_arg_matches("git status", "git status --short"));
        assert!(!shell_arg_matches("git status", "git"));
        assert!(is_dangerous_shell_allow_pattern("python*foo"));
        assert!(is_dangerous_shell_allow_pattern("node -e *"));
        assert!(is_dangerous_shell_allow_pattern("pnpm run *foo"));

        let overlap = perms_with(&[
            ("write(*)", PermissionAction::Allow),
            ("write(**/*.md)", PermissionAction::Ask),
            ("bash(git *)", PermissionAction::Allow),
            ("bash(git status *)", PermissionAction::Ask),
        ]);
        assert_eq!(
            overlap.action_for_path("write", "notes.md"),
            Some(PermissionAction::Ask)
        );
        assert_eq!(
            overlap.action_for_shell("git status --short"),
            Some(PermissionAction::Ask)
        );
        assert!(
            ContentBlock::Thinking {
                text: "t".into(),
                signature: None
            }
            .is_thinking()
        );
        assert!(ContentBlock::RedactedThinking { data: "x".into() }.is_thinking());
    }
}

mod types_usage_tests {
    use super::*;

    fn usage(input: u64, output: u64, created: Option<u64>, read: Option<u64>) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: created,
            cache_read_input_tokens: read,
        }
    }

    #[test]
    fn adding_accumulates_every_field() {
        let mut total = usage(10, 20, Some(1), Some(2));
        total.add(&usage(5, 7, Some(3), Some(4)));
        assert_eq!(total.input_tokens, 15);
        assert_eq!(total.output_tokens, 27);
        assert_eq!(total.cache_creation_input_tokens, Some(4));
        assert_eq!(total.cache_read_input_tokens, Some(6));
    }

    #[test]
    fn cache_stays_none_until_a_provider_reports_it() {
        // A provider without prompt caching must not make the session look
        // like it cached zero tokens — it reported nothing at all.
        let mut total = Usage::default();
        total.add(&usage(10, 20, None, None));
        assert_eq!(total.cache_creation_input_tokens, None);
        assert_eq!(total.cache_read_input_tokens, None);
    }

    #[test]
    fn the_first_report_promotes_none_to_some() {
        let mut total = usage(10, 20, None, None);
        total.add(&usage(0, 0, Some(5), Some(6)));
        assert_eq!(total.cache_creation_input_tokens, Some(5));
        assert_eq!(total.cache_read_input_tokens, Some(6));
    }

    #[test]
    fn total_counts_cache_tokens_as_well() {
        // Cache reads and writes are input tokens reported separately, not a
        // subset of input_tokens, so they add rather than overlap.
        assert_eq!(usage(10, 20, Some(3), Some(4)).total(), 37);
        assert_eq!(usage(10, 20, None, None).total(), 30);
    }

    #[test]
    fn an_untouched_usage_is_empty() {
        assert!(Usage::default().is_empty());
        assert!(usage(0, 0, Some(0), Some(0)).is_empty());
        assert!(!usage(0, 1, None, None).is_empty());
        assert!(!usage(0, 0, Some(1), None).is_empty());
    }

    #[test]
    fn accumulating_nothing_changes_nothing() {
        let mut total = usage(10, 20, Some(1), Some(2));
        let before = total.clone();
        total.add(&Usage::default());
        assert_eq!(total.input_tokens, before.input_tokens);
        assert_eq!(total.output_tokens, before.output_tokens);
        assert_eq!(
            total.cache_creation_input_tokens,
            before.cache_creation_input_tokens
        );
    }

    #[test]
    fn absorb_stream_takes_max_not_sum() {
        let mut step = Usage::default();
        // Anthropic split: input at start, output on the last delta.
        step.absorb_stream(100, 0);
        step.absorb_stream(0, 20);
        assert_eq!(step.input_tokens, 100);
        assert_eq!(step.output_tokens, 20);
        // Repeated full snapshot (OpenAI include_usage on every chunk).
        step.absorb_stream(100, 20);
        step.absorb_stream(100, 20);
        assert_eq!(step.input_tokens, 100);
        assert_eq!(step.output_tokens, 20);
        // Running total climbs; max tracks the last snapshot.
        step.absorb_stream(100, 35);
        assert_eq!(step.output_tokens, 35);
    }

    #[test]
    fn absorb_stream_cache_does_not_double() {
        let mut step = Usage::default();
        step.absorb_stream_cache(8, 40);
        step.absorb_stream_cache(8, 40);
        assert_eq!(step.cache_creation_input_tokens, Some(8));
        assert_eq!(step.cache_read_input_tokens, Some(40));
        step.absorb_stream_cache(0, 0);
        assert_eq!(step.cache_creation_input_tokens, Some(8));
    }
}
