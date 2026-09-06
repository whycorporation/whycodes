use super::*;
use whycodes_tools::executor::ToolExecutor;

fn exec() -> ToolExecutor {
    ToolExecutor::new()
}

fn perms() -> PermissionSet {
    PermissionSet {
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        ..Default::default()
    }
}

async fn rpc(msg: Value) -> Option<Value> {
    handle_rpc(msg, &exec(), &perms(), ToolProfile::Core, ".")
        .await
        .unwrap()
}

#[tokio::test]
async fn notification_has_no_reply() {
    let msg = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    assert!(rpc(msg).await.is_none());
}

#[tokio::test]
async fn initialize_and_ping() {
    let init = rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
        .await
        .unwrap();
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(init["result"]["serverInfo"]["name"], "whycodes");

    let ping = rpc(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
        .await
        .unwrap();
    assert_eq!(ping["result"], json!({}));
}

#[tokio::test]
async fn tools_list_includes_core_read() {
    let resp = rpc(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}))
        .await
        .unwrap();
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|t| t["name"] == "read"),
        "core profile should advertise read: {tools:?}"
    );
}

#[tokio::test]
async fn tools_call_requires_name() {
    let resp = rpc(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {}
    }))
    .await
    .unwrap();
    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn tools_call_reads_a_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello mcp").unwrap();
    let resp = handle_rpc(
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "read",
                "arguments": {"path": "note.txt"}
            }
        }),
        &exec(),
        &perms(),
        ToolProfile::Core,
        dir.path().to_str().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("hello mcp"), "{text}");
}

#[tokio::test]
async fn unknown_method_is_minus_32601() {
    let resp = rpc(json!({"jsonrpc": "2.0", "id": 6, "method": "nope"}))
        .await
        .unwrap();
    assert_eq!(resp["error"]["code"], -32601);
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("method not found"),
        "{resp}"
    );
}

#[tokio::test]
async fn tools_call_defaults_arguments_and_empty_method() {
    let missing_method = rpc(json!({"jsonrpc": "2.0", "id": 7})).await.unwrap();
    assert_eq!(missing_method["error"]["code"], -32601);

    let numeric_method = rpc(json!({"jsonrpc": "2.0", "id": 8, "method": 1}))
        .await
        .unwrap();
    assert_eq!(numeric_method["error"]["code"], -32601);

    let numeric_name = rpc(json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {"name": 3}
    }))
    .await
    .unwrap();
    assert_eq!(numeric_name["error"]["code"], -32602);

    let resp = rpc(json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {"name": "read"}
    }))
    .await
    .unwrap();
    assert_eq!(resp["result"]["isError"], true);
}

#[tokio::test]
async fn run_stdio_io_skips_blank_and_bad_json_then_replies() {
    let input = concat!(
        "\n",
        "   \n",
        "not-json\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "\n",
    );
    let mut reader = std::io::Cursor::new(input.as_bytes().to_vec());
    let mut stdout = Vec::new();
    run_stdio_io(
        &mut reader,
        &mut stdout,
        Arc::new(exec()),
        perms(),
        ToolProfile::Core,
        ".".into(),
    )
    .await
    .unwrap();
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains("\"id\":1"), "{text}");
    assert!(text.contains("\"result\":{}"), "{text}");
}

#[tokio::test]
async fn run_stdio_server_uses_test_stdin() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        "\n",
    );
    TEST_STDIN.with(|s| *s.borrow_mut() = input.as_bytes().to_vec());
    run_stdio_server(Arc::new(exec()), perms(), ToolProfile::Core, ".".into())
        .await
        .unwrap();
    let out = TEST_STDOUT.with(|s| s.borrow().clone());
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("protocolVersion"), "{text}");
    assert!(text.contains("\"id\":2"), "{text}");
}

#[tokio::test]
async fn run_stdio_io_surfaces_read_and_write_errors() {
    struct FailRead;
    impl std::io::Read for FailRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("read boom"))
        }
    }
    impl std::io::BufRead for FailRead {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("read boom"))
        }
        fn consume(&mut self, _: usize) {}
    }
    struct FailWrite;
    impl std::io::Write for FailWrite {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("write boom"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let err = run_stdio_io(
        &mut FailRead,
        &mut Vec::new(),
        Arc::new(exec()),
        perms(),
        ToolProfile::Core,
        ".".into(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("read boom"), "{err}");

    let mut reader = std::io::Cursor::new(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec());
    let err = run_stdio_io(
        &mut reader,
        &mut FailWrite,
        Arc::new(exec()),
        perms(),
        ToolProfile::Core,
        ".".into(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("write boom"), "{err}");

    struct FailFlush {
        wrote: bool,
    }
    impl std::io::Write for FailFlush {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.wrote = true;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("flush boom"))
        }
    }
    let mut reader = std::io::Cursor::new(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec());
    let err = run_stdio_io(
        &mut reader,
        &mut FailFlush { wrote: false },
        Arc::new(exec()),
        perms(),
        ToolProfile::Core,
        ".".into(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("flush boom"), "{err}");
}

#[test]
fn stdio_handles_are_readable() {
    TEST_STDIN.with(|s| *s.borrow_mut() = b"{}\n".to_vec());
    let (mut reader, mut writer) = open_stdio(true);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line, "{}\n");
    writer.write_all(b"ok").unwrap();
    writer.flush().unwrap();
    let out = TEST_STDOUT.with(|s| s.borrow().clone());
    assert_eq!(out, b"ok");

    // Production constructors: build and drop without reading the process stdin.
    let (_reader, _writer) = open_stdio(false);
}
