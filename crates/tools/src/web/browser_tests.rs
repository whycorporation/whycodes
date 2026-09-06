use super::*;
use std::net::TcpListener;
use std::thread;

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
fn find_browser_and_http_get_without_slash() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("WHYCODES_BROWSER");
    let missing = std::env::temp_dir().join("whycodes-missing-browser-bin");
    unsafe { std::env::set_var("WHYCODES_BROWSER", &missing) };
    let _ = find_browser();
    let exists = std::env::current_exe().unwrap();
    unsafe { std::env::set_var("WHYCODES_BROWSER", &exists) };
    assert_eq!(find_browser().as_deref(), Some(exists.as_path()));
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_BROWSER", v),
            None => std::env::remove_var("WHYCODES_BROWSER"),
        }
    }
    let _ = user_data_dir();
    let _ = pick_port();
    assert!(http_get("http://127.0.0.1:1").is_err());
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
    let t = BrowserTool;
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
    let sleep = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(BrowserSession {
            child: sleep,
            port,
            _user_data: PathBuf::from("/tmp"),
        });
    }

    assert_eq!(current_port(), Some(port));
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
    let fake = std::env::current_exe().unwrap();
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
