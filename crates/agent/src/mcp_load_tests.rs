use super::*;

fn server(transport: McpTransportKind) -> McpServerConfig {
    McpServerConfig {
        transport: Some(transport),
        command: None,
        args: Vec::new(),
        env: None,
        cwd: None,
        url: None,
        headers: None,
    }
}

fn err_of(result: anyhow::Result<McpClient>) -> String {
    match result {
        Ok(_) => panic!("expected connect to fail"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn stdio_without_command_errors() {
    let err = err_of(connect_mcp_server(&server(McpTransportKind::Stdio)).await);
    assert!(err.contains("command"), "{err}");
}

#[tokio::test]
async fn remote_transports_require_url() {
    for kind in [
        McpTransportKind::Http,
        McpTransportKind::Sse,
        McpTransportKind::Auto,
    ] {
        let err = err_of(connect_mcp_server(&server(kind)).await);
        assert!(err.contains("url"), "{kind:?}: {err}");
    }
}

#[tokio::test]
async fn neither_command_nor_url_errors() {
    let s = McpServerConfig {
        transport: None,
        command: None,
        args: Vec::new(),
        env: None,
        cwd: None,
        url: None,
        headers: None,
    };
    let err = err_of(connect_mcp_server(&s).await);
    assert!(err.contains("command") || err.contains("url"), "{err}");
}

#[tokio::test]
async fn register_with_no_servers_returns_zero() {
    let mut executor = ToolExecutor::new();
    let config = Config::default();
    assert_eq!(config.mcp_servers.len(), 0);
    let count = register_mcp_tools(&mut executor, &config).await;
    assert_eq!(count, 0);
}

#[test]
fn resolved_transport_auto_for_url_only() {
    let s = McpServerConfig {
        transport: None,
        command: None,
        args: Vec::new(),
        env: None,
        cwd: None,
        url: Some("https://mcp.example.com".into()),
        headers: None,
    };
    assert_eq!(s.resolved_transport().unwrap(), McpTransportKind::Auto);
    assert!(s.is_remote());
}

#[tokio::test]
async fn register_failing_stdio_command_returns_zero() {
    let mut executor = ToolExecutor::new();
    let mut config = Config::default();
    config.mcp_servers.insert(
        "ghost".into(),
        McpServerConfig {
            transport: Some(McpTransportKind::Stdio),
            command: Some("whycodes-definitely-missing-mcp-binary".into()),
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    let count = register_mcp_tools(&mut executor, &config).await;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn connect_stdio_missing_binary_errors() {
    let s = McpServerConfig {
        transport: Some(McpTransportKind::Stdio),
        command: Some("whycodes-definitely-missing-mcp-binary".into()),
        args: vec!["--help".into()],
        env: None,
        cwd: None,
        url: None,
        headers: None,
    };
    let err = err_of(connect_mcp_server(&s).await);
    assert!(!err.is_empty(), "{err}");
}

#[tokio::test]
async fn register_failing_url_logs_url_detail() {
    let mut executor = ToolExecutor::new();
    let mut config = Config::default();
    config.mcp_servers.insert(
        "ghost-http".into(),
        McpServerConfig {
            transport: Some(McpTransportKind::Http),
            command: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            url: Some("http://127.0.0.1:1".into()),
            headers: None,
        },
    );
    let count = register_mcp_tools(&mut executor, &config).await;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn connect_http_sse_auto_with_bad_url_errors() {
    for kind in [
        McpTransportKind::Http,
        McpTransportKind::Sse,
        McpTransportKind::Auto,
    ] {
        let s = McpServerConfig {
            transport: Some(kind),
            command: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            url: Some("http://127.0.0.1:1".into()),
            headers: Some(std::collections::HashMap::from([(
                "X-Test".into(),
                "1".into(),
            )])),
        };
        let err = err_of(connect_mcp_server(&s).await);
        assert!(!err.is_empty(), "{kind:?}: {err}");
    }
}

fn python() -> &'static str {
    python_from(&["python3", "python", "py"])
}

fn python_from(cmds: &[&'static str]) -> &'static str {
    python_from_with(
        cmds,
        command_on_path,
        python_bin(std::path::Path::new("/usr/bin/python3").exists()),
    )
}

fn python_from_with(
    cmds: &[&'static str],
    available: fn(&str) -> bool,
    fallback: &'static str,
) -> &'static str {
    for cmd in cmds {
        if available(cmd) {
            return cmd;
        }
    }
    fallback
}

fn python_bin(usr_bin_exists: bool) -> &'static str {
    if usr_bin_exists {
        "/usr/bin/python3"
    } else {
        "python3"
    }
}

fn command_on_path(cmd: &str) -> bool {
    command_on_path_with(
        cmd,
        std::env::var("PATH").ok().as_deref(),
        std::env::var("PATHEXT").ok().as_deref(),
        |p| p.is_file(),
    )
}

fn command_on_path_with(
    cmd: &str,
    path: Option<&str>,
    pathext: Option<&str>,
    is_file: impl Fn(&std::path::Path) -> bool,
) -> bool {
    let Some(path) = path else {
        return false;
    };
    let exts: Vec<String> = pathext
        .map(|v| {
            v.split(';')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    for dir in std::env::split_paths(path) {
        if exts.is_empty() {
            if is_file(&dir.join(cmd)) {
                return true;
            }
            continue;
        }
        // Windows CreateProcess only resolves PATHEXT (.exe/.cmd/…). An
        // extensionless Git-Bash `python3` shim is a regular file but is
        // not spawnable — skip it so `python` / `py` win.
        if let Some(ext) = std::path::Path::new(cmd).extension() {
            let dotted = format!(".{}", ext.to_string_lossy());
            if exts.iter().any(|e| e.eq_ignore_ascii_case(&dotted)) && is_file(&dir.join(cmd)) {
                return true;
            }
        }
        for ext in &exts {
            if is_file(&dir.join(format!("{cmd}{ext}"))) {
                return true;
            }
        }
    }
    false
}

fn write_stdio_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let script = dir.join("server.py");
    std::fs::write(&script, body).unwrap();
    script
}

fn stdio_echo_script() -> String {
    r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":rid,"result":{
            "protocolVersion":"2025-03-26",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"mock-stdio","version":"0.0.1"}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":rid,"result":{"tools":[
            {"name":"echo","description":"e","inputSchema":{"type":"object"}},
            {"name":"bare","description":"bare","inputSchema":{"type":"object"}},
            {"name":"nullschema","inputSchema":None},
            {"name":"nodesc","inputSchema":{"type":"object","properties":{}}}
        ]}})
    elif method == "tools/call":
        args = (msg.get("params") or {}).get("arguments") or {}
        send({"jsonrpc":"2.0","id":rid,"result":{
            "content":[{"type":"text","text":"echo:" + str(args.get("text",""))}],
            "isError": False
        }})
    else:
        send({"jsonrpc":"2.0","id":rid,"result":{"ok":True}})
"#
    .to_string()
}

fn stdio_list_error_script() -> String {
    r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":rid,"result":{
            "protocolVersion":"2025-03-26",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"mock-list-err","version":"0.0.1"}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":rid,"error":{"code":-32000,"message":"nope"}})
    else:
        send({"jsonrpc":"2.0","id":rid,"result":{"ok":True}})
"#
    .to_string()
}

fn stdio_config(dir: &std::path::Path, script: &std::path::Path) -> McpServerConfig {
    McpServerConfig {
        transport: Some(McpTransportKind::Stdio),
        command: Some(python().into()),
        args: vec!["-u".into(), script.to_string_lossy().into_owned()],
        env: None,
        cwd: Some(dir.to_string_lossy().into_owned()),
        url: None,
        headers: None,
    }
}

#[tokio::test]
async fn register_stdio_success_and_call_bridged_tool() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_stdio_script(dir.path(), &stdio_echo_script());
    let mut executor = ToolExecutor::new();
    let mut config = Config::default();
    config
        .mcp_servers
        .insert("ghost".into(), stdio_config(dir.path(), &script));
    let count = register_mcp_tools(&mut executor, &config).await;
    assert!(count >= 1, "expected at least one MCP tool, got {count}");
    assert!(executor.get("ghost_echo").is_some());
    assert!(executor.get("ghost_bare").is_some());
    assert!(
        executor.get("ghost_nullschema").is_some(),
        "null inputSchema should fall back to an empty object schema"
    );
    assert!(
        executor.get("ghost_nodesc").is_some(),
        "missing description should still register"
    );

    let ctx = whycodes_core::ToolContext::new(dir.path().to_string_lossy().into_owned());
    let call = whycodes_core::types::ToolCall {
        id: "t1".into(),
        name: "ghost_echo".into(),
        arguments: serde_json::json!({"text": "hi"}),
    };
    let result = executor
        .execute(&call, &ctx, &whycodes_core::types::PermissionSet::default())
        .await;
    assert!(!result.is_error, "{result:?}");
    assert!(
        result.content.contains("echo:") || result.content.contains("hi"),
        "{}",
        result.content
    );
}

#[test]
fn mcp_helpers_map_error_and_required_field() {
    assert_eq!(python_bin(true), "/usr/bin/python3");
    assert_eq!(python_bin(false), "python3");
    assert_eq!(
        python_from_with(&["python3", "python"], |_| false, "python3"),
        "python3"
    );
    assert_eq!(
        python_from_with(&["python3", "python"], |c| c == "python", "python3"),
        "python"
    );
    assert!(!command_on_path_with("python3", None, None, |_| true));
    assert!(command_on_path_with(
        "python3",
        Some("/usr/bin"),
        None,
        |p| p.file_name().and_then(|n| n.to_str()) == Some("python3"),
    ));
    assert!(!command_on_path_with(
        "python3",
        Some(r"C:\shim"),
        Some(".EXE;.CMD"),
        |p| p.file_name().and_then(|n| n.to_str()) == Some("python3"),
    ));
    assert!(command_on_path_with(
        "python",
        Some(r"C:\Python"),
        Some(".EXE;.CMD"),
        |p| p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("python.exe")),
    ));
    assert_eq!(mcp_call_error(Ok("ok".into())).expect("ok"), "ok");
    let err = mcp_call_error(Err(whycodes_mcp::McpError::msg("boom"))).unwrap_err();
    assert!(err.contains("boom"), "{err}");
    assert_eq!(
        required_mcp_field(Some("cmd"), "missing").expect("present"),
        "cmd"
    );
    let missing = required_mcp_field(None, "stdio MCP server missing `command`").unwrap_err();
    assert!(missing.to_string().contains("command"), "{missing}");
    assert_eq!(stdio_command(Some("cmd")), "cmd");
    assert_eq!(stdio_command(None), "");
}

#[tokio::test]
async fn register_stdio_list_tools_error_skips_server() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_stdio_script(dir.path(), &stdio_list_error_script());
    let mut executor = ToolExecutor::new();
    let mut config = Config::default();
    config
        .mcp_servers
        .insert("ghost".into(), stdio_config(dir.path(), &script));
    let count = register_mcp_tools(&mut executor, &config).await;
    assert_eq!(count, 0);
    assert!(executor.get("ghost_echo").is_none());
}

#[tokio::test]
async fn register_unresolved_transport_logs_err_detail() {
    let mut executor = ToolExecutor::new();
    let mut config = Config::default();
    config.mcp_servers.insert(
        "ghost-bad".into(),
        McpServerConfig {
            transport: None,
            command: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    let count = register_mcp_tools(&mut executor, &config).await;
    assert_eq!(count, 0);
}
