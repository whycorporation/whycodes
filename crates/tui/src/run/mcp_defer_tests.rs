use super::*;
use crate::run::tests::test_runtime;
use std::time::{Duration, Instant};

/// Python that answers `initialize` and lists one `echo` tool.
const ECHO_SERVER: &str = r#"
import json, sys
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    req = json.loads(line)
    mid = req.get("id")
    method = req.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"ghost","version":"0"}}})
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object"}}]}})
    elif mid is not None:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"unknown"}})
"#;

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        std::process::Command::new(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Config with the echo server, or `None` when no Python is installed.
fn echo_config(dir: &std::path::Path) -> Option<Config> {
    let python = python()?;
    let script = dir.join("echo.py");
    std::fs::write(&script, ECHO_SERVER).unwrap();
    let mut config = Config::default();
    config.mcp_servers.insert(
        "ghost".into(),
        whycodes_config::McpServerConfig {
            transport: Some(whycodes_config::McpTransportKind::Stdio),
            command: Some(python.into()),
            args: vec!["-u".into(), script.to_string_lossy().into_owned()],
            env: None,
            cwd: Some(dir.to_string_lossy().into_owned()),
            url: None,
            headers: None,
        },
    );
    Some(config)
}

async fn until_finished(mcp: &PendingMcp) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !mcp.0.as_ref().is_some_and(JoinHandle::is_finished) {
        assert!(Instant::now() < deadline, "MCP load never finished");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn no_servers_load_inline() {
    let mut rt = test_runtime();
    let mut mcp = PendingMcp::start(&mut rt, &Config::default()).await;
    assert!(mcp.0.is_none());
    mcp.poll(&mut rt, &mut []);
    mcp.wait(&mut rt, &mut []).await;
    assert!(rt.agent.mcp_servers().is_empty());
}

#[tokio::test]
async fn poll_installs_once_finished_and_every_session_is_idle() {
    let dir = tempfile::tempdir().unwrap();
    let Some(config) = echo_config(dir.path()) else {
        return;
    };
    let mut rt = test_runtime();
    let mut others = vec![test_runtime()];
    let mut mcp = PendingMcp::start(&mut rt, &config).await;
    assert!(mcp.0.is_some(), "servers connect in the background");
    until_finished(&mcp).await;

    rt.agent_busy = true;
    mcp.poll(&mut rt, &mut others);
    assert!(
        mcp.0.is_some(),
        "the active session's agent is out on a turn"
    );
    rt.agent_busy = false;
    others[0].agent_busy = true;
    mcp.poll(&mut rt, &mut others);
    assert!(
        mcp.0.is_some(),
        "a background session's agent is out on a turn"
    );
    others[0].agent_busy = false;

    mcp.poll(&mut rt, &mut others);
    assert!(mcp.0.is_none());
    assert_eq!(rt.agent.mcp_servers().len(), 1);
    assert_eq!(others[0].agent.mcp_servers().len(), 1);
}

#[tokio::test]
async fn wait_installs_into_idle_sessions_only() {
    let dir = tempfile::tempdir().unwrap();
    let Some(config) = echo_config(dir.path()) else {
        return;
    };
    let mut rt = test_runtime();
    let mut others = vec![test_runtime(), test_runtime()];
    others[1].agent_busy = true;
    let mut mcp = PendingMcp::start(&mut rt, &config).await;
    mcp.wait(&mut rt, &mut others).await;
    assert!(mcp.0.is_none());
    assert_eq!(rt.agent.mcp_servers().len(), 1);
    assert_eq!(others[0].agent.mcp_servers().len(), 1);
    assert!(others[1].agent.mcp_servers().is_empty());
}

#[tokio::test]
async fn failed_load_task_installs_nothing() {
    let mut config = Config::default();
    config.mcp_servers.insert(
        "gone".into(),
        whycodes_config::McpServerConfig {
            transport: Some(whycodes_config::McpTransportKind::Stdio),
            command: Some("whycodes-no-such-mcp-server".into()),
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    let mut rt = test_runtime();
    let mut mcp = PendingMcp::start(&mut rt, &config).await;
    if let Some(handle) = mcp.0.as_ref() {
        handle.abort();
    }
    mcp.wait(&mut rt, &mut []).await;
    assert!(mcp.0.is_none());
    assert!(rt.agent.mcp_servers().is_empty());
}

#[test]
fn mcp_slash_commands_read_the_servers() {
    for cmd in ["/mcp", "/mcp list", "/mcp__git__review x"] {
        assert!(reads_mcp_servers(cmd), "{cmd}");
    }
    for cmd in ["/model", "/mcpx", "/help"] {
        assert!(!reads_mcp_servers(cmd), "{cmd}");
    }
}
