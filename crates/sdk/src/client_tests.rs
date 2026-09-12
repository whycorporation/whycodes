use super::*;
use futures::StreamExt;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn normalize_adds_scheme() {
    assert_eq!(normalize_base("127.0.0.1:3030"), "http://127.0.0.1:3030");
    assert_eq!(
        normalize_base("http://localhost:3030/"),
        "http://localhost:3030"
    );
}

#[test]
fn pop_sse_skips_keepalive() {
    let mut buf = ": ping\n\ndata: {\"ev\":\"cancelled\"}\n\n".to_string();
    assert!(pop_sse_data(&mut buf).is_none());
    let data = pop_sse_data(&mut buf).unwrap();
    assert!(data.contains("cancelled"));
}

#[test]
fn pop_sse_joins_multiline_data() {
    let mut buf = "data: one\ndata: two\n\n".to_string();
    assert_eq!(pop_sse_data(&mut buf).as_deref(), Some("one\ntwo"));
    assert!(pop_sse_data(&mut buf).is_none());
}

#[test]
fn normalize_https_and_whitespace() {
    assert_eq!(
        normalize_base("  https://example.test/v1/  "),
        "https://example.test/v1"
    );
    assert_eq!(normalize_base("localhost:9"), "http://localhost:9");
}

#[test]
fn resolve_binary_prefers_explicit_then_env() {
    let _lock = env_lock();
    let p = resolve_binary(Some(Path::new("/opt/whycodes"))).unwrap();
    assert_eq!(p, PathBuf::from("/opt/whycodes"));
    let prev = std::env::var_os("WHYCODES");
    unsafe { std::env::set_var("WHYCODES", "/env/whycodes") };
    let p = resolve_binary(None).unwrap();
    assert_eq!(p, PathBuf::from("/env/whycodes"));
    unsafe { std::env::set_var("WHYCODES", "") };
    let p = resolve_binary(None).unwrap();
    assert!(
        p.ends_with("whycodes") || p.ends_with("whycodes.exe") || p == Path::new("whycodes"),
        "{p:?}"
    );
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES", v) },
        None => unsafe { std::env::remove_var("WHYCODES") },
    }
}

#[test]
fn status_error_maps_http_codes() {
    assert_eq!(
        status_error(reqwest::StatusCode::NOT_FOUND, "x").code,
        ErrorCode::UnknownSession
    );
    assert_eq!(
        status_error(reqwest::StatusCode::BAD_REQUEST, "x").code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        status_error(reqwest::StatusCode::UNAUTHORIZED, "x").code,
        ErrorCode::Auth
    );
    assert_eq!(
        status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "x").code,
        ErrorCode::Internal
    );
    let e = SdkError::new(ErrorCode::Timeout, "slow");
    assert!(e.to_string().contains("timeout") || e.to_string().contains("slow"));
}

#[test]
fn ephemeral_port_binds() {
    let p = ephemeral_port().unwrap();
    assert!(p > 0);
}

#[test]
fn launch_options_default() {
    let o = LaunchOptions::default();
    assert!(o.inherit_logins);
    assert!(o.binary.is_none());
    assert!(o.port.is_none());
    assert!(o.home.is_none());
    assert!(!o.working_dir.as_os_str().is_empty());
    assert_eq!(
        working_dir_from(Err(std::io::Error::other("cwd"))),
        PathBuf::from(".")
    );
    assert_eq!(
        working_dir_from(Ok(PathBuf::from("/tmp"))),
        PathBuf::from("/tmp")
    );
    assert_eq!(
        schema_text_from(
            Err(serde_json::from_str::<u8>("x").unwrap_err()),
            &serde_json::json!({"n": 1})
        ),
        serde_json::json!({"n": 1}).to_string()
    );
    assert!(schema_text(&serde_json::json!({"n": 1})).contains("n"));
    assert_eq!(
        ephemeral_bind_failed(std::io::Error::other("bind")).code,
        ErrorCode::StartupFailed
    );
    assert_eq!(
        ephemeral_addr_failed(std::io::Error::other("addr")).code,
        ErrorCode::StartupFailed
    );
}

#[tokio::test]
async fn http_client_helper_wraps_reqwest_errors() {
    let err = reqwest::Client::new()
        .get("http://127.0.0.1:1/")
        .send()
        .await
        .unwrap_err();
    let wrapped = http_client_failed(err);
    assert_eq!(wrapped.code, ErrorCode::Internal);
    assert_eq!(wrapped.message, "http client");
}

#[test]
fn http_client_builds() {
    assert!(http_client().is_ok());
    assert_eq!(binary_names()[0], binary_names()[0]);
    assert!(!binary_names().is_empty());
}

#[cfg(windows)]
#[test]
fn binary_names_windows() {
    assert_eq!(binary_names(), &["whycodes.exe"]);
}

#[test]
fn resolve_binary_uses_injected_exe_and_sibling() {
    let _lock = env_lock();
    let prev_why = std::env::var_os("WHYCODES");
    let prev_exe = std::env::var_os("WHYCODES_TEST_CURRENT_EXE");
    let prev_fail = std::env::var_os("WHYCODES_TEST_CURRENT_EXE_FAIL");
    unsafe { std::env::remove_var("WHYCODES") };
    unsafe { std::env::remove_var("WHYCODES_TEST_CURRENT_EXE_FAIL") };

    let dir = tempfile::tempdir().unwrap();
    let sibling = dir.path().join(binary_names()[0]);
    std::fs::write(&sibling, b"").unwrap();
    let exe = dir.path().join("sdk-test");
    unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &exe) };
    assert_eq!(resolve_binary(None).unwrap(), sibling);

    let named = dir.path().join(binary_names()[0]);
    unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &named) };
    assert_eq!(resolve_binary(None).unwrap(), named);

    let empty_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(empty_dir.path().join("whycodes")).unwrap();
    let other = empty_dir.path().join("other-bin");
    unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &other) };
    assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));

    let no_stem = empty_dir.path().join("..");
    unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &no_stem) };
    assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let weird = empty_dir
            .path()
            .join(std::ffi::OsString::from_vec(vec![0xff]));
        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &weird) };
        assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));
    }

    unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", "") };
    let fallback = resolve_binary(None).unwrap();
    assert!(
        fallback.ends_with("whycodes") || fallback.ends_with("whycodes.exe"),
        "{fallback:?}"
    );

    unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE_FAIL", "1") };
    assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));

    match prev_why {
        Some(v) => unsafe { std::env::set_var("WHYCODES", v) },
        None => unsafe { std::env::remove_var("WHYCODES") },
    }
    match prev_exe {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_CURRENT_EXE") },
    }
    match prev_fail {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE_FAIL", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_CURRENT_EXE_FAIL") },
    }
}

#[tokio::test]
async fn event_stream_covers_poll_states() {
    struct OncePendingThenNone {
        n: u8,
    }
    impl futures::Stream for OncePendingThenNone {
        type Item = reqwest::Result<Vec<u8>>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            if self.n == 0 {
                self.n = 1;
                cx.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
            std::task::Poll::Ready(None)
        }
    }

    let mut pending = EventStream {
        bytes: Box::pin(OncePendingThenNone { n: 0 }),
        buf: String::new(),
        done: false,
    };
    assert!(pending.next().await.is_none());

    let mut already_done = EventStream {
        bytes: futures::stream::empty().boxed(),
        buf: String::new(),
        done: true,
    };
    assert!(already_done.next().await.is_none());

    let mut bad = EventStream {
        bytes: futures::stream::iter([Ok(b"data: not-json\n\n".to_vec())]).boxed(),
        buf: String::new(),
        done: false,
    };
    let err = bad.next().await.unwrap().unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);

    let mut crlf = EventStream {
        bytes: futures::stream::iter([Ok(b"data: {\"ev\":\"cancelled\"}\r\n\r\n".to_vec())])
            .boxed(),
        buf: String::new(),
        done: false,
    };
    assert!(matches!(
        crlf.next().await.unwrap().unwrap(),
        SdkEvent::Cancelled
    ));

    let mut split = EventStream {
        bytes: futures::stream::iter([
            Ok(b"data: {\"ev\":\"tur".to_vec()),
            Ok(b"n_done\",\"text\":\"x\"}\n\n".to_vec()),
        ])
        .boxed(),
        buf: String::new(),
        done: false,
    };
    match split.next().await.unwrap().unwrap() {
        SdkEvent::TurnDone { text } => assert_eq!(text, "x"),
        other => panic!("{other:?}"),
    }
    assert!(split.next().await.is_none());

    let reqwest_err = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap()
        .get("http://127.0.0.1:1/")
        .send()
        .await
        .unwrap_err();
    let mut stream_err = EventStream {
        bytes: futures::stream::iter([Err(reqwest_err)]).boxed(),
        buf: String::new(),
        done: false,
    };
    let err = stream_err.next().await.unwrap().unwrap_err();
    assert!(
        err.code == ErrorCode::Disconnected || err.code == ErrorCode::Timeout,
        "{err:?}"
    );
}

#[tokio::test]
async fn take_stderr_empty_without_child() {
    let mut none = None;
    assert!(take_stderr(&mut none).await.is_empty());
}

#[tokio::test]
async fn take_stderr_reads_child_output_and_missing_pipe() {
    let mut none_pipe = Some(cmd_exit().stderr(Stdio::null()).spawn().unwrap());
    assert!(take_stderr(&mut none_pipe).await.is_empty());

    let mut empty = Some(cmd_exit().stderr(Stdio::piped()).spawn().unwrap());
    let _ = empty.as_mut().unwrap().wait().await;
    assert!(take_stderr(&mut empty).await.is_empty());

    let mut noisy = Some(cmd_stderr_boom().stderr(Stdio::piped()).spawn().unwrap());
    let _ = noisy.as_mut().unwrap().wait().await;
    let text = take_stderr(&mut noisy).await;
    assert!(text.contains("boom"), "{text}");

    let mut live = Some(
        cmd_hang()
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap(),
    );
    assert!(take_stderr(&mut live).await.is_empty());
    let mut none: Option<Child> = None;
    assert!(take_stderr(&mut none).await.is_empty());
}

#[tokio::test]
async fn close_and_drop_kill_launched_child() {
    let live = cmd_hang().kill_on_drop(true).spawn().unwrap();
    let client =
        WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap()).with_child(live);
    client.close().await.unwrap();

    let live = cmd_hang().kill_on_drop(true).spawn().unwrap();
    drop(
        WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap()).with_child(live),
    );

    let mut dead = cmd_exit().spawn().unwrap();
    let _ = dead.wait().await;
    drop(
        WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap()).with_child(dead),
    );

    let mut dead = cmd_exit().spawn().unwrap();
    let _ = dead.wait().await;
    WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap())
        .with_child(dead)
        .close()
        .await
        .unwrap();
}

#[test]
fn prepare_launch_covers_port_home_and_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let explicit = prepare_launch(&LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dir.path().join("bin")),
        inherit_logins: true,
        home: None,
        startup_timeout: Duration::from_millis(200),
        port: Some(9),
    })
    .unwrap();
    assert_eq!(explicit.port, 9);
    assert!(explicit.home_env.is_none());
    assert!(explicit.held_home.is_none());

    let ephemeral = prepare_launch(&LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dir.path().join("bin")),
        inherit_logins: true,
        home: None,
        startup_timeout: Duration::from_millis(200),
        port: None,
    })
    .unwrap();
    assert!(ephemeral.port > 0);

    let home = dir.path().join("home");
    let isolated = prepare_launch(&LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dir.path().join("bin")),
        inherit_logins: true,
        home: Some(home.clone()),
        startup_timeout: Duration::from_millis(200),
        port: Some(11),
    })
    .unwrap();
    assert_eq!(isolated.home_env.as_deref(), Some(home.as_path()));
    assert!(home.is_dir());
    assert!(isolated.held_home.is_none());

    let temp = prepare_launch(&LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dir.path().join("bin")),
        inherit_logins: false,
        home: None,
        startup_timeout: Duration::from_millis(200),
        port: Some(12),
    })
    .unwrap();
    assert!(temp.home_env.is_some());
    assert!(temp.held_home.is_some());

    let cmd = launch_command(
        &temp,
        &LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            inherit_logins: false,
            ..Default::default()
        },
    );
    drop(cmd);
    assert_eq!(STRIPPED_LOGIN_KEYS.len(), 8);
}

#[test]
fn launch_poll_covers_timeout_exit_version_retry_and_ready() {
    let now = Instant::now();
    let timeout = Duration::from_millis(200);
    match launch_poll(
        now + timeout,
        now,
        timeout,
        "http://127.0.0.1:9",
        None,
        Err(SdkError::new(ErrorCode::Disconnected, "down")),
    ) {
        LaunchPoll::Failed(err) => {
            assert_eq!(err.code, ErrorCode::StartupTimeout);
            assert!(err.message.contains("http://127.0.0.1:9"));
        }
        other => panic!("timeout: {other:?}"),
    }

    match launch_poll(
        now,
        now + timeout,
        timeout,
        "http://127.0.0.1:9",
        Some("exit status: 7".into()),
        Ok(()),
    ) {
        LaunchPoll::Failed(err) => {
            assert_eq!(err.code, ErrorCode::StartupFailed);
            assert!(err.message.contains("exit status: 7"));
        }
        other => panic!("exit: {other:?}"),
    }

    match launch_poll(
        now,
        now + timeout,
        timeout,
        "http://127.0.0.1:9",
        None,
        Err(SdkError::new(ErrorCode::UnsupportedVersion, "proto")),
    ) {
        LaunchPoll::Failed(err) => assert_eq!(err.code, ErrorCode::UnsupportedVersion),
        other => panic!("version: {other:?}"),
    }

    assert!(matches!(
        launch_poll(
            now,
            now + timeout,
            timeout,
            "http://127.0.0.1:9",
            None,
            Err(SdkError::new(ErrorCode::Disconnected, "retry")),
        ),
        LaunchPoll::Retry
    ));
    assert!(matches!(
        launch_poll(
            now,
            now + timeout,
            timeout,
            "http://127.0.0.1:9",
            None,
            Ok(())
        ),
        LaunchPoll::Ready
    ));

    let timeout_err = SdkError::new(ErrorCode::StartupTimeout, "daemon down.");
    let with_stderr = attach_stderr(timeout_err, "stderr: waiting");
    assert!(with_stderr.message.contains("stderr: waiting"));
    let empty = attach_stderr(SdkError::new(ErrorCode::StartupFailed, "exited."), "");
    assert_eq!(empty.message, "exited.");

    assert!(poll_child_exit(None).is_none());

    assert!(missing_spawned_binary(Path::new("python")).is_none());
    assert!(missing_spawned_binary(Path::new("whycodes")).is_none());
    let relative = missing_spawned_binary(Path::new("no/such/whycodes")).unwrap();
    assert_eq!(relative.code, ErrorCode::ServeNotFound);
    let missing = missing_spawned_binary(Path::new("/no/such/whycodes")).unwrap();
    assert_eq!(missing.code, ErrorCode::ServeNotFound);
    let present = tempfile::NamedTempFile::new().unwrap();
    assert!(missing_spawned_binary(present.path()).is_none());
    let err = missing_spawned_binary(Path::new("/no/such/whycodes-binary")).unwrap();
    assert_eq!(err.code, ErrorCode::ServeNotFound);
    assert!(err.message.contains("not found"), "{err:?}");
}

#[tokio::test]
async fn launch_missing_binary_is_serve_not_found() {
    let err = match WhyCodesClient::launch(LaunchOptions {
        binary: Some(PathBuf::from("/no/such/whycodes-binary")),
        inherit_logins: false,
        startup_timeout: Duration::from_millis(200),
        ..Default::default()
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected ServeNotFound"),
    };
    assert_eq!(err.code, ErrorCode::ServeNotFound);

    let err = match WhyCodesClient::launch(LaunchOptions {
        binary: Some(PathBuf::from("whycodes-no-such-binary-on-path")),
        inherit_logins: false,
        startup_timeout: Duration::from_millis(200),
        ..Default::default()
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected ServeNotFound"),
    };
    assert_eq!(err.code, ErrorCode::ServeNotFound);

    // Armed tempdir-fail must not steal ServeNotFound (CI flake on Linux).
    let _fail = super::TestTempdirFailGuard::arm();
    let err = match WhyCodesClient::launch(LaunchOptions {
        binary: Some(PathBuf::from("/no/such/whycodes-binary")),
        inherit_logins: false,
        startup_timeout: Duration::from_millis(200),
        ..Default::default()
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected ServeNotFound"),
    };
    assert_eq!(err.code, ErrorCode::ServeNotFound);
}

#[tokio::test]
async fn launch_temp_home_create_failure_is_startup_failed() {
    let dir = tempfile::tempdir().unwrap();
    let dummy = dir.path().join("unused");
    std::fs::write(&dummy, b"").unwrap();
    let _fail = super::TestTempdirFailGuard::arm();
    let err = match WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dummy),
        inherit_logins: false,
        home: None,
        startup_timeout: Duration::from_millis(200),
        port: Some(1),
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected StartupFailed"),
    };
    assert_eq!(err.code, ErrorCode::StartupFailed, "{err:?}");
}

#[tokio::test]
async fn launch_home_create_failure_is_startup_failed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    // Present so launch does not classify a missing binary first.
    let dummy = dir.path().join("unused");
    std::fs::write(&dummy, b"").unwrap();
    let err = match WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dummy),
        inherit_logins: true,
        home: Some(file.join("nested")),
        startup_timeout: Duration::from_millis(200),
        port: Some(1),
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected StartupFailed"),
    };
    assert_eq!(err.code, ErrorCode::StartupFailed);
    assert!(err.message.contains("WHYCODES_HOME"));
}

#[tokio::test]
async fn launch_injected_tempdir_failure_is_startup_failed() {
    let dir = tempfile::tempdir().unwrap();
    let dummy = dir.path().join("unused");
    std::fs::write(&dummy, b"").unwrap();
    let _fail = super::TestTempdirFailGuard::arm();
    let err = match WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(dummy),
        inherit_logins: false,
        home: None,
        startup_timeout: Duration::from_millis(200),
        port: Some(1),
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected injected tempdir failure"),
    };
    assert_eq!(err.code, ErrorCode::StartupFailed);
    assert!(err.message.contains("temp WHYCODES_HOME"), "{err:?}");
}

#[cfg(not(windows))]
fn system_bin(name: &str) -> PathBuf {
    for candidate in [format!("/usr/bin/{name}"), format!("/bin/{name}")] {
        let path = PathBuf::from(&candidate);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(name)
}

fn cmd_exit() -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "exit", "0"]);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new("true")
    }
}

fn cmd_hang() -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "ping", "-n", "30", "127.0.0.1", ">", "NUL"]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        cmd
    }
}

fn cmd_stderr_boom() -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "echo boom 1>&2"]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo boom >&2");
        cmd
    }
}

fn launch_exit_bin(dir: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        std::fs::write(dir.join("serve"), "import sys\nsys.exit(1)\n").unwrap();
        python3()
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        system_bin("true")
    }
}

fn launch_hang_bin(dir: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        std::fs::write(dir.join("serve"), "import time\ntime.sleep(30)\n").unwrap();
        python3()
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        system_bin("yes")
    }
}

/// Spawns a real child (`true` ignores extra `serve <port>` args) so the
/// production launch loop is covered without the coverage.sh-skipped
/// fake-daemon tests.
#[tokio::test]
async fn launch_true_child_exit_is_startup_failed() {
    let dir = tempfile::tempdir().unwrap();
    let err = match WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(launch_exit_bin(dir.path())),
        inherit_logins: false,
        startup_timeout: Duration::from_secs(2),
        port: Some(1),
        ..Default::default()
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected StartupFailed"),
    };
    assert!(
        err.code == ErrorCode::StartupFailed || err.code == ErrorCode::StartupTimeout,
        "{err:?}"
    );
    assert!(
        err.message.contains("exited") || err.message.contains("did not become healthy"),
        "{err:?}"
    );
}

/// `yes` stays alive and ignores extra `serve <port>` args, so handshake
/// retries until `startup_timeout` (covers Retry loop + stderr attach).
/// `take_stderr` kills the child first so `read_to_end` cannot hang.
#[tokio::test]
async fn launch_yes_retries_until_startup_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let err = match WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(launch_hang_bin(dir.path())),
        inherit_logins: true,
        startup_timeout: Duration::from_millis(250),
        port: Some(1),
        ..Default::default()
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected StartupTimeout"),
    };
    assert_eq!(err.code, ErrorCode::StartupTimeout);
    assert!(err.message.contains("did not become healthy"), "{err:?}");
}

fn python3() -> PathBuf {
    for cmd in ["python3", "python", "py"] {
        #[cfg(windows)]
        {
            if let Ok(path) = std::env::var("PATH") {
                let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into());
                for dir in std::env::split_paths(&path) {
                    if dir.join(cmd).is_file() {
                        return PathBuf::from(cmd);
                    }
                    for ext in exts.split(';').filter(|s| !s.is_empty()) {
                        if dir.join(format!("{cmd}{ext}")).is_file() {
                            return PathBuf::from(cmd);
                        }
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            let path = system_bin(cmd);
            if path.is_file() {
                return path;
            }
        }
    }
    PathBuf::from("python3")
}

fn write_health_serve_script(dir: &std::path::Path, protocol: u32) {
    // `launch` runs `<binary> serve <port>` with `current_dir` = working_dir,
    // so a file named `serve` is the Python script python3 executes.
    std::fs::write(
        dir.join("serve"),
        format!(
            r#"
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/v1/health":
            body = json.dumps({{
                "protocol": {protocol},
                "version": "0.0.0",
                "healthy": True,
                "project": "/tmp",
                "uptime_secs": 1,
                "sessions_in_memory": 0,
            }}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()
    def log_message(self, *_args):
        pass

port = int(sys.argv[-1])
HTTPServer(("127.0.0.1", port), H).serve_forever()
"#
        ),
    )
    .unwrap();
}

/// Real spawn + handshake Ready path: `python3 serve <port>` binds
/// `/v1/health` on the requested port.
#[tokio::test]
async fn launch_python_health_server_is_ready() {
    let dir = tempfile::tempdir().unwrap();
    write_health_serve_script(dir.path(), PROTOCOL_MAJOR);
    let client = WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(python3()),
        inherit_logins: false,
        startup_timeout: Duration::from_secs(5),
        port: None,
        home: None,
    })
    .await
    .expect("python health server should become ready");
    assert!(client.base_url().starts_with("http://127.0.0.1:"));
    client.close().await.unwrap();
}

/// Same spawn loop, but `/v1/health` speaks protocol 99 so launch fails
/// with UnsupportedVersion instead of retrying.
#[tokio::test]
async fn launch_python_wrong_protocol_is_unsupported_version() {
    let dir = tempfile::tempdir().unwrap();
    write_health_serve_script(dir.path(), 99);
    let err = match WhyCodesClient::launch(LaunchOptions {
        working_dir: dir.path().to_path_buf(),
        binary: Some(python3()),
        inherit_logins: false,
        startup_timeout: Duration::from_secs(5),
        port: None,
        home: None,
    })
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected UnsupportedVersion"),
    };
    assert_eq!(err.code, ErrorCode::UnsupportedVersion);
}
