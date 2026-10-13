use super::*;
use tokio::sync::Mutex;
use whycodes_mcp::client::McpClient;

const SERVER: &str = r#"
import json, sys
mode = sys.argv[1]
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
def ok(rid, result):
    send({"jsonrpc":"2.0","id":rid,"result":result})
def fail(rid, message):
    send({"jsonrpc":"2.0","id":rid,"error":{"code":-32000,"message":message}})
caps = {"tools":{}} if mode == "bare" else {"tools":{},"prompts":{},"resources":{}}
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    p = msg.get("params") or {}
    if method == "initialize":
        ok(rid, {"protocolVersion":"2025-03-26","capabilities":caps,
                 "serverInfo":{"name":mode,"version":"1"}})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        ok(rid, {"tools":[]})
    elif mode == "broken":
        fail(rid, "broken server")
    elif method == "resources/list":
        ok(rid, {"resources":[
            {"uri":"doc://guide","name":"guide","description":"How to"},
            {"uri":"doc://logo","name":"logo"}
        ]})
    elif method == "resources/read":
        uri = p.get("uri")
        if uri == "doc://guide":
            ok(rid, {"contents":[{"uri":uri,"text":"Guide body"}]})
        elif uri == "doc://logo":
            ok(rid, {"contents":[
                {"uri":uri,"mimeType":"image/png","blob":"AAAA"},
                {"uri":uri,"blob":"AA"},
                {"uri":uri}
            ]})
        else:
            fail(rid, "no such resource")
    elif method == "prompts/list":
        ok(rid, {"prompts":[
            {"name":"review","description":"Review code","arguments":[
                {"name":"path","required":True},{"name":"focus"}]},
            {"name":"hello"},
            {"name":"fails"}
        ]})
    elif method == "prompts/get":
        if p.get("name") == "fails":
            fail(rid, "render failed")
        else:
            a = p.get("arguments") or {}
            text = p.get("name") + "|" + ",".join(k + "=" + a[k] for k in sorted(a))
            ok(rid, {"messages":[{"role":"user","content":{"type":"text","text":text}}]})
    else:
        ok(rid, {})
"#;

fn python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

async fn server(dir: &std::path::Path, name: &str, mode: &str) -> McpServerHandle {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let client = McpClient::connect_stdio(python(), &["-u", script.to_str().unwrap(), mode])
        .await
        .unwrap();
    McpServerHandle {
        name: name.to_string(),
        client: Arc::new(Mutex::new(client)),
    }
}

async fn call(executor: &ToolExecutor, name: &str, args: Value) -> (String, bool) {
    let ctx = whycodes_core::ToolContext::new(".".to_string());
    let call = whycodes_core::types::ToolCall {
        id: "t".into(),
        name: name.into(),
        arguments: args,
    };
    let result = executor
        .execute(&call, &ctx, &whycodes_core::types::PermissionSet::default())
        .await;
    (result.content, result.is_error)
}

#[tokio::test]
async fn resource_tools_list_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let docs = server(dir.path(), "docs", "full").await;
    let bare = server(dir.path(), "bare", "bare").await;
    let broken = server(dir.path(), "broken", "broken").await;

    let mut none = ToolExecutor::new();
    assert_eq!(
        register_resource_tools(&mut none, std::slice::from_ref(&bare)).await,
        0
    );
    assert!(none.get(LIST_RESOURCES_TOOL).is_none());

    let mut executor = ToolExecutor::new();
    let servers = [docs, bare, broken];
    assert_eq!(register_resource_tools(&mut executor, &servers).await, 2);

    let (all, _) = call(&executor, LIST_RESOURCES_TOOL, json!({})).await;
    assert!(all.contains("[docs] guide <doc://guide> — How to"), "{all}");
    assert!(all.contains("[docs] logo <doc://logo>"), "{all}");
    assert!(all.contains("[broken] error:"), "{all}");
    assert!(!all.contains("[bare]"), "{all}");
    let (only, _) = call(&executor, LIST_RESOURCES_TOOL, json!({"server": "docs"})).await;
    assert!(!only.contains("broken"), "{only}");
    let (empty, _) = call(&executor, LIST_RESOURCES_TOOL, json!({"server": "zzz"})).await;
    assert_eq!(empty, "No MCP resources.");

    let read = |uri: &str| json!({"server": "docs", "uri": uri});
    let (guide, err) = call(&executor, READ_RESOURCE_TOOL, read("doc://guide")).await;
    assert!(!err);
    assert_eq!(guide, "Guide body");
    let (logo, _) = call(&executor, READ_RESOURCE_TOOL, read("doc://logo")).await;
    assert_eq!(
        logo,
        "[binary image/png at doc://logo, 4 base64 chars]\n\n\
         [binary data at doc://logo, 2 base64 chars]\n\n[empty resource doc://logo]"
    );
    let (missing, err) = call(&executor, READ_RESOURCE_TOOL, read("doc://nope")).await;
    assert!(err && missing.contains("no such resource"), "{missing}");
    let (unknown, err) = call(
        &executor,
        READ_RESOURCE_TOOL,
        json!({"server": "x", "uri": "doc://guide"}),
    )
    .await;
    assert!(
        err && unknown.contains("unknown MCP server 'x'"),
        "{unknown}"
    );
    assert!(unknown.contains("docs, broken"), "{unknown}");
}

#[tokio::test]
async fn prompts_render_with_positional_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let servers = [
        server(dir.path(), "docs", "full").await,
        server(dir.path(), "broken", "broken").await,
    ];
    assert!(render_mcp_prompt(&servers, "review", "").await.is_none());
    assert!(render_mcp_prompt(&servers, "mcp__docs", "").await.is_none());
    assert!(
        render_mcp_prompt(&servers, "mcp__zzz__p", "")
            .await
            .is_none()
    );

    let ok = render_mcp_prompt(&servers, "mcp__docs__review", "src/a.rs  the auth flow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ok, "review|focus=the auth flow,path=src/a.rs");
    let one = render_mcp_prompt(&servers, "mcp__docs__review", "src/a.rs")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(one, "review|path=src/a.rs");
    let bare = render_mcp_prompt(&servers, "mcp__docs__hello", "ignored")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bare, "hello|");

    let missing = render_mcp_prompt(&servers, "mcp__docs__review", "  ")
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        missing,
        "missing argument 'path' — usage: /mcp__docs__review <path> <focus>"
    );
    let unknown = render_mcp_prompt(&servers, "mcp__docs__nope", "")
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(unknown, "'docs' has no prompt 'nope'");
    let failed = render_mcp_prompt(&servers, "mcp__docs__fails", "")
        .await
        .unwrap()
        .unwrap_err();
    assert!(failed.contains("render failed"), "{failed}");
    let broken = render_mcp_prompt(&servers, "mcp__broken__x", "")
        .await
        .unwrap()
        .unwrap_err();
    assert!(broken.contains("broken server"), "{broken}");
}

#[tokio::test]
async fn describe_lists_servers_prompts_and_resources() {
    assert!(describe_mcp_servers(&[]).await.contains("No MCP servers"));
    let dir = tempfile::tempdir().unwrap();
    let servers = [
        server(dir.path(), "docs", "full").await,
        server(dir.path(), "bare", "bare").await,
        server(dir.path(), "broken", "broken").await,
    ];
    let text = describe_mcp_servers(&servers).await;
    assert!(
        text.contains("docs (stdio), resources via mcp_list_resources"),
        "{text}"
    );
    assert!(
        text.contains("  /mcp__docs__review <path> <focus> — Review code"),
        "{text}"
    );
    assert!(text.contains("  /mcp__docs__hello\n"), "{text}");
    assert!(text.contains("\nbare (stdio)\n"), "{text}");
    assert!(text.contains("  prompts unavailable:"), "{text}");
}

#[tokio::test]
async fn agent_keeps_connected_servers() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let mut config = whycodes_config::Config::default();
    config.mcp_servers.insert(
        "docs".into(),
        whycodes_config::McpServerConfig {
            transport: Some(whycodes_config::McpTransportKind::Stdio),
            command: Some(python().into()),
            args: vec![
                "-u".into(),
                script.to_string_lossy().into_owned(),
                "full".into(),
            ],
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    let info = config.get_agent("build").cloned().unwrap();
    let agent = crate::Agent::new(info).with_mcp(&config).await;
    let names: Vec<&str> = agent
        .mcp_servers()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["docs"]);
}
