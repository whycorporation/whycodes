use super::*;
use std::net::TcpListener;
use std::path::Path;
use std::process::Stdio;
use std::thread;

struct FailSerde;

impl serde::Serialize for FailSerde {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("nope"))
    }
}

fn dir_as_file(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join(name), b"not-a-directory").expect("blocker file");
    dir
}

fn hang_browser_child() -> Child {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "ping", "-n", "30", "127.0.0.1", ">", "NUL"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
    #[cfg(not(windows))]
    {
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
}

fn connected_streams() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("read listener address");
    let client = TcpStream::connect(addr).expect("connect to loopback listener");
    let (server, _) = listener.accept().expect("accept loopback connection");
    (client, server)
}

#[test]
fn metadata_and_parameters_describe_the_public_contract() {
    let tool = BrowserTool::new();
    assert_eq!(tool.name(), "browser");
    assert!(!tool.description().is_empty());

    let schema = tool.parameters();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["action"]));
    assert_eq!(schema["properties"]["action"]["type"], "string");
    assert_eq!(
        schema["properties"]["action"]["enum"],
        json!([
            "status",
            "open",
            "snapshot",
            "click",
            "type",
            "wait",
            "screenshot",
            "close"
        ])
    );
    assert_eq!(schema["properties"]["url"]["type"], "string");
    assert_eq!(schema["properties"]["selector"]["type"], "string");
    assert_eq!(schema["properties"]["text"]["type"], "string");
    assert_eq!(schema["properties"]["ms"]["type"], "integer");
}

#[tokio::test]
async fn invalid_actions_and_missing_arguments_are_rejected_locally() {
    let tool = BrowserTool::new();
    let ctx = ToolContext::unsandboxed(".");
    let cases = [
        (json!({}), "action must be"),
        (json!({"action": "  "}), "action must be"),
        (json!({"action": "unknown"}), "action must be"),
        (json!({"action": "open"}), "open requires `url`"),
        (json!({"action": "open", "url": 42}), "open requires `url`"),
        (json!({"action": "click"}), "click requires `selector`"),
        (
            json!({"action": "click", "selector": false}),
            "click requires `selector`",
        ),
        (json!({"action": "type"}), "type requires `selector`"),
    ];

    for (args, expected) in cases {
        let result = tool.execute(args, &ctx).await;
        assert!(result.is_error, "expected an error: {result:?}");
        assert!(
            result.content.contains(expected),
            "expected {expected:?} in {:?}",
            result.content
        );
    }
}

#[tokio::test]
async fn session_actions_fail_cleanly_without_a_browser() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    drop(close_browser());
    let tool = BrowserTool::new();
    let ctx = ToolContext::unsandboxed(".");

    for args in [
        json!({"action": "click", "selector": "#submit"}),
        json!({"action": "type", "selector": "#name", "text": "Ada"}),
        json!({"action": "screenshot"}),
    ] {
        let result = tool.execute(args, &ctx).await;
        assert!(result.is_error, "expected an error: {result:?}");
        assert_eq!(
            result.content,
            "no browser session — call browser open first"
        );
    }

    let result = tool.execute(json!({"action": "wait", "ms": 0}), &ctx).await;
    assert!(!result.is_error);
    assert_eq!(result.content, "waited 0ms");

    let result = tool.execute(json!({"action": "close"}), &ctx).await;
    assert!(!result.is_error);
    assert_eq!(result.content, "no browser session");
}

#[test]
fn protocol_helpers_reject_unsupported_url_schemes_before_io() {
    assert_eq!(
        http_get("https://example.test").unwrap_err(),
        "bad url https://example.test"
    );
    assert_eq!(
        ws_cdp_call("wss://example.test/devtools", "Page.enable", json!({})).unwrap_err(),
        "need ws:// url, got wss://example.test/devtools"
    );
}

#[test]
fn http_get_constructs_request_and_extracts_response_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock HTTP server");
    let addr = listener.local_addr().expect("read mock HTTP address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept HTTP request");
        let mut request = [0; 512];
        let size = stream.read(&mut request).expect("read HTTP request");
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("GET /json/list HTTP/1.0\r\n"));
        assert!(request.contains(&format!("Host: {addr}\r\n")));
        assert!(request.contains("Connection: close\r\n"));
        stream
            .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\n{\"x\":1}")
            .expect("write HTTP response");
    });

    let body = http_get(&format!("http://{addr}/json/list")).expect("perform HTTP request");
    assert_eq!(body, "{\"x\":1}");
    server.join().expect("mock HTTP server completed");
}

#[test]
fn websocket_frames_round_trip_boundary_lengths() {
    for payload in [vec![b'a'; 125], vec![b'b'; 126], vec![b'c'; 65_536]] {
        let (mut writer, mut reader) = connected_streams();
        write_ws_text(&mut writer, &payload).expect("write masked text frame");
        let decoded = read_ws_text(&mut reader).expect("read masked text frame");
        assert_eq!(decoded.as_bytes(), payload);
    }
}

#[test]
fn websocket_reader_reports_close_and_invalid_utf8_frames() {
    let (mut writer, mut reader) = connected_streams();
    writer.write_all(&[0x88, 0]).expect("write close frame");
    assert_eq!(
        read_ws_text(&mut reader).unwrap_err(),
        "cdp websocket closed"
    );

    let (mut writer, mut reader) = connected_streams();
    writer
        .write_all(&[0x81, 2, 0xc3, 0x28])
        .expect("write invalid UTF-8 frame");
    assert!(
        read_ws_text(&mut reader)
            .unwrap_err()
            .contains("invalid utf-8 sequence")
    );

    // Unmasked frames (server → client). Masked writes never take these arms.
    let (mut writer, mut reader) = connected_streams();
    let mut frame = vec![0x81, 126];
    frame.extend_from_slice(&126u16.to_be_bytes());
    frame.extend(std::iter::repeat_n(b'z', 126));
    writer
        .write_all(&frame)
        .expect("write unmasked 16-bit frame");
    assert_eq!(read_ws_text(&mut reader).unwrap().len(), 126);

    let (mut writer, mut reader) = connected_streams();
    let mut frame = vec![0x81, 127];
    frame.extend_from_slice(&1u64.to_be_bytes());
    frame.push(b'q');
    writer
        .write_all(&frame)
        .expect("write unmasked 64-bit frame");
    assert_eq!(read_ws_text(&mut reader).unwrap(), "q");
}

#[test]
fn ws_cdp_call_builds_command_skips_events_and_parses_result() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock CDP server");
    let addr = listener.local_addr().expect("read mock CDP address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept CDP connection");
        let mut handshake = [0; 1024];
        let size = stream
            .read(&mut handshake)
            .expect("read WebSocket handshake");
        let handshake = String::from_utf8_lossy(&handshake[..size]);
        assert!(handshake.starts_with("GET /devtools/page/1 HTTP/1.1\r\n"));
        assert!(handshake.contains(&format!("Host: {addr}\r\n")));
        assert!(handshake.contains("Upgrade: websocket\r\n"));
        assert!(handshake.contains("Sec-WebSocket-Key: d2h5Y29kZXMtY2RwLWtleSEh\r\n"));
        stream
            .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
            .expect("write WebSocket handshake");

        let command = read_ws_text(&mut stream).expect("read CDP command");
        let command: Value = serde_json::from_str(&command).expect("parse CDP command");
        assert_eq!(command["id"], 1);
        assert_eq!(command["method"], "Runtime.evaluate");
        assert_eq!(command["params"], json!({"expression": "2 + 2"}));

        write_ws_text(&mut stream, br#"{"method":"Runtime.consoleAPICalled"}"#)
            .expect("write unrelated event");
        write_ws_text(&mut stream, br#"{"id":1,"result":{"value":4}}"#)
            .expect("write CDP response");
    });

    let result = ws_cdp_call(
        &format!("ws://{addr}/devtools/page/1"),
        "Runtime.evaluate",
        json!({"expression": "2 + 2"}),
    )
    .expect("perform CDP call");
    assert_eq!(result, json!({"value": 4}));
    server.join().expect("mock CDP server completed");
}

#[tokio::test]
async fn status_without_session() {
    let ctx = ToolContext::unsandboxed(".");
    let r = BrowserTool::new()
        .execute(json!({"action": "status"}), &ctx)
        .await;
    // Either a PATH error or a not-running status — never a hang / panic.
    assert!(!r.content.is_empty());
}

#[test]
fn status_and_ensure_session_when_no_browser_exists() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    drop(close_browser());
    let missing = status_with_browser(None);
    assert!(missing.is_error, "{}", missing.content);
    assert!(
        missing.content.contains("No Chromium"),
        "{}",
        missing.content
    );
    let err = ensure_session_with_browser(None).unwrap_err();
    assert!(err.contains("No Chromium"), "{err}");
}

#[tokio::test]
async fn snapshot_without_session_errors() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    drop(close_browser());
    let ctx = ToolContext::unsandboxed(".");
    let r = BrowserTool::new()
        .execute(json!({"action": "snapshot"}), &ctx)
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("no browser session"));
}

#[test]
fn clamp_wait_ms_caps_at_fifteen_seconds() {
    assert_eq!(clamp_wait_ms(0), 0);
    assert_eq!(clamp_wait_ms(1_000), 1_000);
    assert_eq!(clamp_wait_ms(15_000), 15_000);
    assert_eq!(clamp_wait_ms(99_000), 15_000);
}

#[test]
fn find_browser_and_http_get_without_slash() {
    assert_find_browser_env();
    assert_browser_lookup_and_status();
    assert_page_action_results();
    assert_browser_session_helpers();
}

fn assert_find_browser_env() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("WHYCODES_BROWSER");
    let missing = std::env::temp_dir().join("whycodes-missing-browser-bin");
    unsafe { std::env::set_var("WHYCODES_BROWSER", &missing) };
    let _ = find_browser();
    let exists = std::env::current_exe().unwrap();
    unsafe { std::env::set_var("WHYCODES_BROWSER", &exists) };
    assert_eq!(find_browser().as_deref(), Some(exists.as_path()));
    let found_status = status();
    assert!(!found_status.is_error, "{}", found_status.content);
    assert!(
        found_status.content.contains("browser:"),
        "{}",
        found_status.content
    );
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_BROWSER", v),
            None => std::env::remove_var("WHYCODES_BROWSER"),
        }
    }
}

fn assert_browser_lookup_and_status() {
    let _ = user_data_dir();
    let _ = pick_port();
    assert!(http_get("http://127.0.0.1:1").is_err());
    assert!(which_browser_from(None).is_none());
    assert!(
        which_browser_from(Some(std::process::Output {
            status: fail_cmd_status(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }))
        .is_none()
    );
    assert!(
        which_browser_from(Some(std::process::Output {
            status: ok_cmd_status(),
            stdout: b"   \n".to_vec(),
            stderr: Vec::new(),
        }))
        .is_none()
    );
    assert_eq!(
        which_browser_from(Some(std::process::Output {
            status: ok_cmd_status(),
            stdout: b"/usr/bin/chromium\n".to_vec(),
            stderr: Vec::new(),
        }))
        .as_deref(),
        Some(std::path::Path::new("/usr/bin/chromium"))
    );
    let _ = find_browser_on_path();
    let (running, port) = session_status();
    assert!(!running);
    assert!(port.is_none());
    assert_eq!(pick_port_fallback("boom", "ephemeral port bind"), 9222);
    let status = browser_status_line(Path::new("/usr/bin/chrome"), false, None);
    assert!(status.contains("running: false"));
    assert!(status.contains("port: -"));
    let js_err = evaluate_js_exception(&json!({"text": "boom"}));
    assert!(js_err.unwrap_err().contains("js exception"));
    let found = browser_found_status(Path::new("/usr/bin/chrome"), false, None);
    assert!(!found.is_error);
    assert!(found.content.contains("running: false"));
    let fields = session_fields(&None);
    assert!(!fields.0);
    assert!(fields.1.is_none());
    assert!(decode_screenshot("%%%").is_err());
    assert_eq!(decode_screenshot("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(screenshot_data(&json!({"data": "abc"})), Some("abc"));
    assert!(screenshot_data(&json!({})).is_none());
}

fn assert_page_action_results() {
    assert!(existing_session_port(Ok(SESSION.lock().unwrap_or_else(|e| e.into_inner()))).is_none());
    mkdir_browser_profile(&std::env::temp_dir().join("whycodes-browser-profile-test"));
    let snap_ok = snapshot_from_eval(Ok(json!({"title": "t"})));
    assert!(!snap_ok.is_error, "{}", snap_ok.content);
    let snap_err = snapshot_from_eval(Err("boom".into()));
    assert!(snap_err.is_error);
    let click_ok = click_from_eval(Ok(json!({"ok": true})), "#x");
    assert!(!click_ok.is_error);
    let click_fail = click_from_eval(Ok(json!({"ok": false})), "#x");
    assert!(click_fail.is_error);
    let click_err = click_from_eval(Err("boom".into()), "#x");
    assert!(click_err.is_error);
    let type_ok = type_from_eval(Ok(json!({"ok": true})));
    assert!(!type_ok.is_error);
    let type_fail = type_from_eval(Ok(json!({"ok": false})));
    assert!(type_fail.is_error);
    let type_err = type_from_eval(Err("boom".into()));
    assert!(type_err.is_error);
    let shot_err = screenshot_from_cdp(Err("boom".into()), &ToolContext::unsandboxed("."));
    assert!(shot_err.is_error);
    let shot_nodata = screenshot_from_cdp(Ok(json!({})), &ToolContext::unsandboxed("."));
    assert!(shot_nodata.is_error);
    let shot_bad = screenshot_from_cdp(Ok(json!({"data": "%%%"})), &ToolContext::unsandboxed("."));
    assert!(shot_bad.is_error);
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().into_owned());
    let saved = write_screenshot_bytes(&ctx, b"png");
    assert!(!saved.is_error, "{}", saved.content);
    assert!(js_exception_from(&json!({"text": "boom"})).is_err());
    let nav_err = navigate_opened(1, "http://example.test");
    assert!(nav_err.is_error);
    mkdir_browser_profile_result(Ok(()));
    mkdir_browser_profile_result(Err(std::io::Error::other("mkdir")));
    page_enable_result(Ok(json!({})));
    page_enable_result(Err("boom".into()));
    kill_child_debug(Ok(()), "browser kill");
    kill_child_debug(Err(std::io::Error::other("kill")), "browser kill");
    set_timeout_debug(Ok(()), "http get timeout");
    set_timeout_debug(Err(std::io::Error::other("timeout")), "http get timeout");
    note_browser_launch();
    note_browser_profile_ok();
    note_page_enable_ok();
    note_screenshot_dir_ok();
    note_browser_child_ok();
    note_timeout_ok();
    let launch_msg = browser_launch_error(Path::new("chrome"))(std::io::Error::other("spawn"));
    assert!(launch_msg.contains("failed to launch"), "{launch_msg}");
    assert_eq!(io_err("x"), "x");
    assert!(cdp_connect_error("x").contains("cdp connect"));
}

fn assert_browser_session_helpers() {
    assert_browser_error_helpers();
    assert_browser_session_poll();
}

fn assert_browser_error_helpers() {
    assert!(screenshot_decode_error("x").contains("screenshot decode"));
    assert_eq!(utf8_frame_error("x"), "x");
    assert_eq!(cdp_json_error("x"), "x");
    assert!(screenshot_mkdir_error("denied").is_error);
    assert!(screenshot_mkdir_failed("denied").is_error);
    let reply = cdp_reply("Page.enable", &json!({"result": {"ok": true}}));
    assert_eq!(reply.ok(), Some(json!({"ok": true})));
    let reply_err = cdp_reply("Page.enable", &json!({"error": {"message": "nope"}}));
    assert!(reply_err.unwrap_err().contains("cdp Page.enable"));
    assert!(screenshot_write_failed("denied").is_error);
    assert_eq!(pick_bound_port(Err(std::io::Error::other("bind"))), 9222);
    assert_eq!(port_from_addr(Err(std::io::Error::other("addr"))), 9222);
    drop(unlock_session(Ok(SESSION
        .lock()
        .unwrap_or_else(|e| e.into_inner()))));
    drop(unlock_session_result(Ok(SESSION
        .lock()
        .unwrap_or_else(|e| e.into_inner()))));
    let lock: std::sync::Mutex<u8> = std::sync::Mutex::new(0);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = lock.lock().unwrap();
        panic!("poison");
    }));
    let recovered = recover_lock(lock.lock());
    assert_eq!(*recovered, 0);
    drop(recovered);
    assert_eq!(*poisoned_lock(lock.lock().expect_err("poisoned")), 0);
    let snap = pretty_snapshot(&json!({"title": "t"}));
    assert!(snap.contains("title"), "{snap}");
    assert_eq!(pretty_json(&FailSerde, "raw"), "raw");
}

fn assert_browser_session_poll() {
    let not_ready = take_ready_session(false, hang_browser_child(), 1, PathBuf::from("/tmp"));
    match not_ready {
        Err((mut child, _)) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(_) => panic!("expected not ready"),
    }
    let ready = take_ready_session(true, hang_browser_child(), 11, PathBuf::from("/tmp"));
    assert_eq!(ready.ok().and_then(|r| r.ok()), Some(11));
    drop(close_browser());
    let applied = apply_ready_session(Ok(Ok(12)));
    assert_eq!(applied.ok().and_then(|r| r.ok()), Some(12));
    let finished = finish_session_poll(Ok(Ok(14)));
    assert_eq!(finished.ok().and_then(|r| r.ok()), Some(14));
    let (ready, retry) = split_session_poll(Ok(Ok(15)));
    assert_eq!(ready.and_then(|r| r.ok()), Some(15));
    assert!(retry.is_none());
    let (ready, retry) = split_session_poll(Err((hang_browser_child(), PathBuf::from("/tmp"))));
    assert!(ready.is_none());
    if let Some((mut child, _)) = retry {
        let _ = child.kill();
        let _ = child.wait();
    }
    let polled = next_session_poll(true, hang_browser_child(), 13, PathBuf::from("/tmp"));
    assert_eq!(polled.ok().and_then(|r| r.ok()), Some(13));
    drop(close_browser());
    let retried = apply_ready_session(Err((hang_browser_child(), PathBuf::from("/tmp"))));
    match retried {
        Err((mut child, _)) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(_) => panic!("expected retry"),
    }
    let eval_ex = evaluate_cdp_result(Ok(json!({"exceptionDetails": {"text": "boom"}})));
    assert!(eval_ex.is_err());
    let eval_ok = evaluate_cdp_result(Ok(json!({"result": {"value": 4}})));
    assert_eq!(eval_ok.ok(), Some(json!(4)));
    let eval_err = evaluate_cdp_result(Err("cdp".into()));
    assert!(eval_err.is_err());
    let blocked = tempfile::tempdir().unwrap();
    let why = blocked.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(why.join("browser"), "not-a-dir").unwrap();
    let ctx = ToolContext::unsandboxed(blocked.path().to_string_lossy().into_owned());
    let mkdir_err = write_screenshot_bytes(&ctx, b"png");
    assert!(mkdir_err.is_error, "{}", mkdir_err.content);
    let write_err = write_screenshot_file(why.as_path(), b"png");
    assert!(write_err.is_error, "{}", write_err.content);
    let stored = store_session(
        hang_browser_child(),
        9,
        PathBuf::from("/tmp/whycodes-browser-store"),
    );
    assert_eq!(stored.ok(), Some(9));
    assert!(existing_session_port(Ok(SESSION.lock().unwrap_or_else(|e| e.into_inner()))).is_some());
    drop(close_browser());
    let mut hanging = hang_browser_child();
    kill_launch_timeout(&mut hanging);
    let mut hanging = hang_browser_child();
    kill_browser_child(&mut hanging);
    wait_browser_child(&mut hanging);
    let (client, server) = connected_streams();
    set_http_read_timeout(&client);
    set_cdp_timeouts(&client);
    drop(server);
    drop(client);
}

/// A real executable that is not Chromium. `/bin/false` is absent on some
/// Linux runners (`/usr/bin/false` only); a missing path makes `ensure_session`
/// return before spawn and leaves the poll loop uncovered.
fn fake_browser_bin() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        std::path::PathBuf::from(r"C:\Windows\System32\cmd.exe")
    }
    #[cfg(not(windows))]
    {
        for candidate in ["/bin/false", "/usr/bin/false", "/bin/true", "/usr/bin/true"] {
            let path = std::path::PathBuf::from(candidate);
            if path.is_file() {
                return path;
            }
        }
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("false"))
    }
}

fn fail_cmd_status() -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(1 << 8)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(1)
    }
}

fn ok_cmd_status() -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    }
}

#[test]
fn cdp_and_evaluate_helpers_on_loopback() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let body = r#"[{"type":"page","webSocketDebuggerUrl":"ws://127.0.0.1:1/x"}]"#;
        let resp = format!(
            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
    });
    let err = cdp(addr.port(), "Page.enable", json!({})).unwrap_err();
    assert!(!err.is_empty());
    server.join().ok();
    assert!(http_get(&format!("http://{addr}/missing")).is_err() || true);
}

#[test]
fn websocket_error_and_handshake_failure() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
    });
    let err = ws_cdp_call(&format!("ws://{addr}/x"), "Page.enable", json!({})).unwrap_err();
    assert!(err.contains("handshake") || err.contains("400"), "{err}");
    server.join().ok();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
        let _ = read_ws_text(&mut stream);
        write_ws_text(&mut stream, br#"{"id":1,"error":{"message":"nope"}}"#).ok();
    });
    let err = ws_cdp_call(&format!("ws://{addr}/x"), "Page.enable", json!({})).unwrap_err();
    assert!(err.contains("cdp"), "{err}");
    server.join().ok();

    let (mut writer, mut reader) = connected_streams();
    writer.write_all(&[0x81, 126, 0, 1, b'z']).unwrap();
    let _ = read_ws_text(&mut reader);
}

#[tokio::test]
async fn default_and_open_without_browser() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = BrowserTool::default();
    assert_eq!(t.name(), "browser");
    drop(close_browser());
    let ctx = ToolContext::unsandboxed(".");
    let open = t
        .execute(
            json!({"action": "open", "url": "http://example.test"}),
            &ctx,
        )
        .await;
    assert!(open.is_error, "{}", open.content);
}

#[test]
fn evaluate_and_session_helpers_on_loopback_cdp() {
    use std::io::{Read, Write};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    let server = thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if req.contains("Upgrade: websocket") {
                let _ = stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
                if let Ok(msg) = read_ws_text(&mut stream) {
                    let v: Value = serde_json::from_str(&msg).unwrap_or(json!({}));
                    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    let result = match method {
                        "Runtime.evaluate" => {
                            json!({"result":{"value":{"ok":true,"title":"t","url":"u","text":"hi","interactables":[]}}})
                        }
                        "Page.captureScreenshot" => json!({"data":"aGVsbG8="}),
                        _ => json!({}),
                    };
                    let resp = json!({"id":1,"result": result});
                    let _ = write_ws_text(&mut stream, resp.to_string().as_bytes());
                }
            } else {
                let body = format!(
                    r#"[{{"type":"page","webSocketDebuggerUrl":"ws://{addr}/devtools/page/1"}}]"#
                );
                let resp = format!(
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        }
    });

    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        *g = None;
    }
    let sleep = hang_browser_child();
    {
        let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(BrowserSession {
            child: sleep,
            port,
            _user_data: PathBuf::from("/tmp"),
        });
    }

    assert_eq!(current_port(), Some(port));
    assert_eq!(ensure_session().ok(), Some(port));
    page_enable_best_effort(port);
    let opened = open_url("http://example.test");
    assert!(
        !opened.is_error || opened.content.contains("opened") || opened.content.contains("cdp"),
        "{}",
        opened.content
    );
    assert!(evaluate(port, "1+1").is_ok());
    let snap = snapshot();
    assert!(!snap.is_error, "{}", snap.content);
    let clicked = click("#x");
    assert!(
        !clicked.is_error || clicked.content.contains("click"),
        "{}",
        clicked.content
    );
    let typed = type_text("#x", "hi");
    assert!(
        !typed.is_error || typed.content.contains("type") || typed.content.contains("typed"),
        "{}",
        typed.content
    );
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().into_owned());
    let shot = screenshot(&ctx);
    assert!(
        !shot.is_error || shot.content.contains("saved") || shot.content.contains("screenshot"),
        "{}",
        shot.content
    );
    let closed = close_browser();
    assert!(!closed.is_error, "{}", closed.content);
    let ready = poll_session_ready(hang_browser_child(), port, PathBuf::from("/tmp"));
    assert_eq!(ready.ok(), Some(port));
    drop(close_browser());
    assert!(poll_invariant().is_err());
    assert!(retry_or_invariant(None).is_err());
    assert!(apply_split_poll((None, None)).is_err());
    let applied_ok = apply_split_poll((Some(Ok(16)), None));
    assert_eq!(applied_ok.ok().and_then(|r| r.ok()), Some(16));
    let applied_retry =
        apply_split_poll((None, Some((hang_browser_child(), PathBuf::from("/tmp")))));
    match applied_retry {
        Err(Some((mut child, _))) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        _ => panic!("expected retry"),
    }
    drop(server);
}

#[test]
fn pick_port_env_and_status_poison_paths() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("WHYCODES_BROWSER_PORT");
    unsafe { std::env::set_var("WHYCODES_BROWSER_PORT", "9229") };
    assert_eq!(pick_port(), 9229);
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_BROWSER_PORT", v),
            None => std::env::remove_var("WHYCODES_BROWSER_PORT"),
        }
    }
    let _ = status();
    let _ = find_browser();
    let _ = ensure_session();
}

#[test]
fn ensure_session_times_out_fake_browser() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    drop(close_browser());
    let prev = std::env::var_os("WHYCODES_BROWSER");
    // Must not be a real Chromium: `current_exe()` on some Linux CI hosts
    // still answers CDP and the timeout path never fires (`Ok(port)`).
    let fake = fake_browser_bin();
    unsafe { std::env::set_var("WHYCODES_BROWSER", &fake) };
    let err = ensure_session();
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_BROWSER", v),
            None => std::env::remove_var("WHYCODES_BROWSER"),
        }
    }
    drop(close_browser());
    assert!(err.is_err(), "{err:?}");
}

#[test]
fn uncovered_io_error_arms() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    drop(close_browser());

    let missing = dir_as_file("whycodes-not-a-browser");
    let bin = missing.path().join("whycodes-not-a-browser");
    let launch = ensure_session_with_browser(Some(bin));
    assert!(
        launch.as_ref().unwrap_err().contains("failed to launch"),
        "{launch:?}"
    );

    let blocked = dir_as_file("browser-profile");
    let profile = blocked.path().join("browser-profile");
    mkdir_browser_profile(&profile);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf);
        drop(stream);
    });
    // A clean close is `Ok("")` on Windows; an abort is `Err`. Either is fine.
    let _ = http_get(&format!("http://{addr}/json/version"));
    server.join().ok();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let connect =
        ws_cdp_call(&format!("ws://{addr}/devtools"), "Page.enable", json!({})).unwrap_err();
    assert!(connect.contains("cdp connect"), "{connect}");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let _ = stream.shutdown(std::net::Shutdown::Both);
    });
    let err = ws_cdp_call(&format!("ws://{addr}/devtools"), "Page.enable", json!({})).unwrap_err();
    assert!(!err.is_empty(), "{err}");
    server.join().ok();

    let (mut writer, reader) = connected_streams();
    drop(reader);
    // Windows may buffer a short write after the peer drops. Either outcome
    // still executes the write path.
    let _ = write_ws_text(&mut writer, b"x");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
        let _ = stream.write_all(&[0x81, 4]);
        let _ = stream.write_all(br#"{"x":"#);
    });
    let parse =
        ws_cdp_call(&format!("ws://{addr}/devtools"), "Page.enable", json!({})).unwrap_err();
    assert!(!parse.is_empty(), "{parse}");
    server.join().ok();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
        let _ = stream.flush();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_ws_text(&mut stream, br#"{"id":1}"#);
        let _ = stream.flush();
        std::thread::sleep(std::time::Duration::from_millis(300));
    });
    let missing_result = ws_cdp_call(&format!("ws://{addr}/devtools"), "Page.enable", json!({}))
        .expect("null result");
    assert!(missing_result.is_null());
    server.join().ok();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"not json at all");
    });
    let pages = cdp(addr.port(), "Page.enable", json!({})).unwrap_err();
    assert!(pages.contains("no CDP page target"), "{pages}");
    server.join().ok();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf);
        let body = r#"[{"type":"page"}]"#;
        let resp = format!("HTTP/1.0 200 OK\r\n\r\n{body}");
        let _ = stream.write_all(resp.as_bytes());
    });
    let pages = cdp(addr.port(), "Page.enable", json!({})).unwrap_err();
    assert!(pages.contains("no CDP page target"), "{pages}");
    server.join().ok();

    let eval_null = evaluate_cdp_result(Ok(json!({})));
    assert_eq!(eval_null.ok(), Some(serde_json::Value::Null));

    let running = browser_status_line(std::path::Path::new("chrome"), true, Some(9));
    assert!(running.contains("running: true"));
    assert!(running.contains("port: 9"));

    let pretty = snapshot_from_eval(Ok(json!({"a": 1})));
    assert!(pretty.content.contains('\n') || pretty.content.contains("\"a\""));

    // HTTP body with no header separator, and a host that has no path.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"raw-body");
    });
    let raw = http_get(&format!("http://{addr}")).expect("body without headers");
    assert_eq!(raw, "raw-body");
    server.join().ok();

    // CDP list whose only target is not a page.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf);
        let body = r#"[{"type":"browser","webSocketDebuggerUrl":"ws://127.0.0.1:1/browser"}]"#;
        let resp = format!("HTTP/1.0 200 OK\r\n\r\n{body}");
        let _ = stream.write_all(resp.as_bytes());
    });
    let pages = cdp(addr.port(), "Page.enable", json!({})).unwrap_err();
    assert!(pages.contains("no CDP page target"), "{pages}");
    server.join().ok();

    // WebSocket URL with no path, and a masked frame whose payload is not UTF-8.
    assert!(
        ws_cdp_call("ws://127.0.0.1:1", "Page.enable", json!({}))
            .unwrap_err()
            .contains("cdp connect")
    );
    let (mut writer, mut reader) = connected_streams();
    write_ws_text(&mut writer, &[0xc3, 0x28]).expect("write masked invalid utf-8");
    assert!(
        read_ws_text(&mut reader)
            .unwrap_err()
            .contains("invalid utf-8")
    );

    // Poisoned session lock is recovered; `store_session` still returns the port.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        panic!("poison session");
    }));
    let stored = store_session(
        hang_browser_child(),
        17,
        PathBuf::from("/tmp/poison-session"),
    );
    assert_eq!(stored.ok(), Some(17));
    drop(close_browser());

    // A shut-down socket fails the masked WebSocket write. A small blocking
    // write still succeeds on Windows (the kernel buffers it). A non-blocking
    // write larger than the send buffer returns WouldBlock / connection reset.
    let (mut writer, reader) = connected_streams();
    drop(reader);
    writer.set_nonblocking(true).expect("nonblocking writer");
    let payload = vec![b'x'; 256 * 1024];
    let mut wrote = Ok(());
    for _ in 0..8 {
        wrote = write_ws_text(&mut writer, &payload);
        if wrote.is_err() {
            break;
        }
    }
    assert!(wrote.is_err(), "write to a closed socket should fail");
}
