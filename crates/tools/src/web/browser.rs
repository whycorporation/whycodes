//! Built-in browser tool (Chrome/Chromium CDP).
//!
//! Not a multi-tenant sandbox. The real browser is outside bwrap; permission
//! defaults to **ask**. Network allowlists do not apply inside the browser.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

struct BrowserSession {
    child: Child,
    port: u16,
    _user_data: PathBuf,
}

static SESSION: Mutex<Option<BrowserSession>> = Mutex::new(None);

/// Interact with a local Chromium via CDP.
pub struct BrowserTool;

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Control a local Chromium/Chrome window (CDP). Actions: status, open, \
         snapshot, click, type, wait, screenshot, close. Not in the core tool \
         profile — use tool_search. The OS sandbox does not apply; domain \
         allowlists do not apply inside the browser. Permission defaults to ask."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "open", "snapshot", "click", "type", "wait", "screenshot", "close"],
                    "description": "Browser action"
                },
                "url": { "type": "string", "description": "URL for open" },
                "selector": { "type": "string", "description": "CSS selector for click/type" },
                "text": { "type": "string", "description": "Text to type" },
                "ms": { "type": "integer", "description": "Wait milliseconds (default 1000)" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        match action {
            "status" => status(),
            "open" => {
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if url.is_empty() {
                    return err("open requires `url`");
                }
                open_url(url)
            }
            "snapshot" => snapshot(),
            "click" => {
                let sel = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                if sel.is_empty() {
                    return err("click requires `selector`");
                }
                click(sel)
            }
            "type" => {
                let sel = args.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if sel.is_empty() {
                    return err("type requires `selector`");
                }
                type_text(sel, text)
            }
            "wait" => {
                let ms = args.get("ms").and_then(|v| v.as_u64()).unwrap_or(1000);
                wait_ms(ms)
            }
            "screenshot" => screenshot(ctx),
            "close" => close_browser(),
            _ => err("action must be status|open|snapshot|click|type|wait|screenshot|close"),
        }
    }
}

fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: true,
    }
}

fn ok(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: false,
    }
}

fn find_browser() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WHYCODES_BROWSER") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    const NAMES: &[&str] = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "msedge",
        "microsoft-edge",
        "chrome",
    ];
    for name in NAMES {
        if let Ok(out) = Command::new("which").arg(name).output()
            && out.status.success()
        {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

fn status() -> ToolResult {
    let bin = find_browser();
    let (running, port) = match SESSION.lock() {
        Ok(g) => (g.is_some(), g.as_ref().map(|s| s.port)),
        Err(p) => {
            let g = p.into_inner();
            (g.is_some(), g.as_ref().map(|s| s.port))
        }
    };
    match bin {
        None => err(
            "No Chromium/Chrome on PATH. Install Chromium or set WHYCODES_BROWSER=/path/to/chrome.",
        ),
        Some(p) => ok(format!(
            "browser: {}\nrunning: {}\nport: {}",
            p.display(),
            running,
            port.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
        )),
    }
}

fn user_data_dir() -> PathBuf {
    whycodes_core::paths::data_dir().join("browser-profile")
}

fn ensure_session() -> Result<u16, String> {
    if let Ok(g) = SESSION.lock()
        && let Some(s) = g.as_ref()
    {
        return Ok(s.port);
    }
    let bin = find_browser().ok_or_else(|| {
        "No Chromium/Chrome on PATH. Install Chromium or set WHYCODES_BROWSER.".to_string()
    })?;
    let dir = user_data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, "browser profile mkdir");
    }
    let port = pick_port();
    let mut child = Command::new(&bin)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-dev-shm-usage",
        ])
        .arg(format!("--user-data-dir={}", dir.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to launch {}: {e}", bin.display()))?;

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if http_get(&format!("http://127.0.0.1:{port}/json/version")).is_ok() {
            if let Ok(mut g) = SESSION.lock() {
                *g = Some(BrowserSession {
                    child,
                    port,
                    _user_data: dir,
                });
            }
            return Ok(port);
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    if let Err(e) = child.kill() {
        tracing::debug!(error = %e, "browser launch timeout kill");
    }
    Err("Chromium started but CDP never became ready".into())
}

fn pick_port() -> u16 {
    match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => match l.local_addr() {
            Ok(a) => a.port(),
            Err(e) => {
                tracing::debug!(error = %e, "ephemeral port local_addr");
                9222
            }
        },
        Err(e) => {
            tracing::debug!(error = %e, "ephemeral port bind");
            9222
        }
    }
}

fn open_url(url: &str) -> ToolResult {
    let port = match ensure_session() {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    match cdp(port, "Page.navigate", json!({ "url": url })) {
        Ok(_) => {
            if let Err(e) = cdp(port, "Page.enable", json!({})) {
                tracing::debug!(error = %e, "Page.enable");
            }
            ok(format!("opened {url}"))
        }
        Err(e) => err(e),
    }
}

fn snapshot() -> ToolResult {
    let port = match current_port() {
        Some(p) => p,
        None => return err("no browser session — call browser open first"),
    };
    let expr = r#"(function(){
      const text = document.body ? document.body.innerText.slice(0, 12000) : '';
      const title = document.title || '';
      const url = location.href;
      const els = Array.from(document.querySelectorAll('a,button,input,textarea,select'))
        .slice(0, 40)
        .map(e => {
          const id = e.id ? '#'+e.id : '';
          const name = e.getAttribute('name') ? '[name='+e.getAttribute('name')+']' : '';
          const label = (e.innerText || e.getAttribute('aria-label') || e.getAttribute('placeholder') || '').trim().slice(0, 60);
          return e.tagName.toLowerCase() + id + name + (label ? ' "'+label+'"' : '');
        });
      return {title, url, text, interactables: els};
    })()"#;
    match evaluate(port, expr) {
        Ok(v) => ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())),
        Err(e) => err(e),
    }
}

fn click(selector: &str) -> ToolResult {
    let port = match current_port() {
        Some(p) => p,
        None => return err("no browser session — call browser open first"),
    };
    let expr = format!(
        r#"(function(){{ const e = document.querySelector({sel}); if(!e) return {{ok:false,error:'not found'}}; e.click(); return {{ok:true}}; }})()"#,
        sel = json!(selector)
    );
    match evaluate(port, &expr) {
        Ok(v) if v.get("ok").and_then(|b| b.as_bool()) == Some(true) => {
            ok(format!("clicked {selector}"))
        }
        Ok(v) => err(format!("click failed: {v}")),
        Err(e) => err(e),
    }
}

fn type_text(selector: &str, text: &str) -> ToolResult {
    let port = match current_port() {
        Some(p) => p,
        None => return err("no browser session — call browser open first"),
    };
    let expr = format!(
        r#"(function(){{ const e = document.querySelector({sel}); if(!e) return {{ok:false,error:'not found'}}; e.focus(); e.value = {val}; e.dispatchEvent(new Event('input',{{bubbles:true}})); return {{ok:true}}; }})()"#,
        sel = json!(selector),
        val = json!(text)
    );
    match evaluate(port, &expr) {
        Ok(v) if v.get("ok").and_then(|b| b.as_bool()) == Some(true) => ok("typed"),
        Ok(v) => err(format!("type failed: {v}")),
        Err(e) => err(e),
    }
}

fn wait_ms(ms: u64) -> ToolResult {
    let ms = ms.min(15_000);
    std::thread::sleep(Duration::from_millis(ms));
    ok(format!("waited {ms}ms"))
}

fn screenshot(ctx: &ToolContext) -> ToolResult {
    let port = match current_port() {
        Some(p) => p,
        None => return err("no browser session — call browser open first"),
    };
    let v = match cdp(port, "Page.captureScreenshot", json!({ "format": "png" })) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    let Some(b64) = v.get("data").and_then(|d| d.as_str()) else {
        return err("screenshot: no data");
    };
    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => b,
        Err(e) => return err(format!("screenshot decode: {e}")),
    };
    let dir = whycodes_core::project_dir(Path::new(&ctx.working_dir)).join("browser");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return err(format!("mkdir: {e}"));
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("shot-{ts}.png"));
    if let Err(e) = std::fs::write(&path, bytes) {
        return err(format!("write screenshot: {e}"));
    }
    ok(format!("saved {}", path.display()))
}

fn close_browser() -> ToolResult {
    let mut g = match SESSION.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(mut s) = g.take() {
        if let Err(e) = s.child.kill() {
            tracing::debug!(error = %e, "browser kill");
        }
        if let Err(e) = s.child.wait() {
            tracing::debug!(error = %e, "browser wait");
        }
        return ok("browser closed");
    }
    ok("no browser session")
}

fn current_port() -> Option<u16> {
    match SESSION.lock() {
        Ok(g) => g.as_ref().map(|s| s.port),
        Err(p) => p.into_inner().as_ref().map(|s| s.port),
    }
}

fn evaluate(port: u16, expression: &str) -> Result<Value, String> {
    let v = cdp(
        port,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true
        }),
    )?;
    if let Some(ex) = v.get("exceptionDetails") {
        return Err(format!("js exception: {ex}"));
    }
    Ok(v.get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

fn cdp(port: u16, method: &str, params: Value) -> Result<Value, String> {
    let list = http_get(&format!("http://127.0.0.1:{port}/json/list"))?;
    let pages: Vec<Value> = serde_json::from_str(&list).unwrap_or_default();
    let ws_url = pages
        .iter()
        .find(|p| p.get("type").and_then(|t| t.as_str()) == Some("page"))
        .and_then(|p| p.get("webSocketDebuggerUrl"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| "no CDP page target".to_string())?;
    ws_cdp_call(ws_url, method, params)
}

fn http_get(url: &str) -> Result<String, String> {
    // Tiny blocking GET so we don't need an async runtime in this helper.
    let url = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("bad url {url}"))?;
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{path}");
    let mut stream = TcpStream::connect(hostport).map_err(|e| e.to_string())?;
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(5))) {
        tracing::debug!(error = %e, "http get timeout");
    }
    let req = format!("GET {path} HTTP/1.0\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).map_err(|e| e.to_string())?;
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or(&buf);
    Ok(body.to_string())
}

/// Minimal client WebSocket + one CDP request/response.
fn ws_cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let url = ws_url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("need ws:// url, got {ws_url}"))?;
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{path}");
    let mut stream = TcpStream::connect(hostport).map_err(|e| format!("cdp connect: {e}"))?;
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(10))) {
        tracing::debug!(error = %e, "cdp read timeout");
    }
    if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(10))) {
        tracing::debug!(error = %e, "cdp write timeout");
    }
    let key = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        *b"whycodes-cdp-key!!",
    );
    let hs = format!(
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(hs.as_bytes()).map_err(|e| e.to_string())?;
    let mut hdr = [0u8; 1024];
    let n = stream.read(&mut hdr).map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&hdr[..n]);
    if !head.contains("101") {
        return Err(format!(
            "ws handshake failed: {}",
            head.lines().next().unwrap_or("")
        ));
    }
    let id = 1u64;
    let payload = json!({"id": id, "method": method, "params": params}).to_string();
    write_ws_text(&mut stream, payload.as_bytes())?;
    loop {
        let msg = read_ws_text(&mut stream)?;
        let v: Value = serde_json::from_str(&msg).map_err(|e| e.to_string())?;
        if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
            if let Some(err) = v.get("error") {
                return Err(format!("cdp {method}: {err}"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

fn write_ws_text(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x81);
    let mask = [0x11, 0x22, 0x33, 0x44];
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len < 65536 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        frame.push(b ^ mask[i % 4]);
    }
    stream.write_all(&frame).map_err(|e| e.to_string())
}

fn read_ws_text(stream: &mut TcpStream) -> Result<String, String> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    let opcode = hdr[0] & 0x0f;
    let mut len = (hdr[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).map_err(|e| e.to_string())?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).map_err(|e| e.to_string())?;
        len = u64::from_be_bytes(ext) as usize;
    }
    if hdr[1] & 0x80 != 0 {
        let mut mask = [0u8; 4];
        stream.read_exact(&mut mask).map_err(|e| e.to_string())?;
        let mut data = vec![0u8; len];
        stream.read_exact(&mut data).map_err(|e| e.to_string())?;
        for (i, b) in data.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
        return String::from_utf8(data).map_err(|e| e.to_string());
    }
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).map_err(|e| e.to_string())?;
    if opcode == 0x8 {
        return Err("cdp websocket closed".into());
    }
    String::from_utf8(data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
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
        let ctx = ToolContext::unsandboxed(".");
        let r = BrowserTool::new()
            .execute(json!({"action": "snapshot"}), &ctx)
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("no browser session"));
    }
}
