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

use serde_json::{Value, json};

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

type SessionPoll = Result<Result<u16, String>, (Child, PathBuf)>;
type SplitPoll = (Option<Result<u16, String>>, Option<(Child, PathBuf)>);

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

    fn execute<'a>(&'a self, args: Value, ctx: &'a ToolContext) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
        })
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: true,
    }
}

fn ok(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: false,
    }
}

const BROWSER_NAMES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "msedge",
    "microsoft-edge",
    "chrome",
];

fn find_browser() -> Option<PathBuf> {
    find_browser_from_env().or_else(find_browser_on_path)
}

fn find_browser_from_env() -> Option<PathBuf> {
    let p = std::env::var("WHYCODES_BROWSER").ok()?;
    let pb = PathBuf::from(p);
    pb.exists().then_some(pb)
}

fn find_browser_on_path() -> Option<PathBuf> {
    BROWSER_NAMES.iter().find_map(|name| which_browser(name))
}

fn which_browser(name: &str) -> Option<PathBuf> {
    which_browser_from(Command::new("which").arg(name).output().ok())
}

fn which_browser_from(output: Option<std::process::Output>) -> Option<PathBuf> {
    let out = output?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() {
        None
    } else {
        Some(PathBuf::from(p))
    }
}

fn status() -> ToolResult {
    let bin = find_browser();
    let (running, port) = session_status();
    match bin {
        None => err(
            "No Chromium/Chrome on PATH. Install Chromium or set WHYCODES_BROWSER=/path/to/chrome.",
        ),
        Some(p) => browser_found_status(&p, running, port),
    }
}

fn user_data_dir() -> PathBuf {
    whycodes_core::paths::data_dir().join("browser-profile")
}

fn ensure_session() -> Result<u16, String> {
    if let Some(port) = existing_session_port(SESSION.lock()) {
        return Ok(port);
    }
    let bin = find_browser().ok_or_else(|| {
        "No Chromium/Chrome on PATH. Install Chromium or set WHYCODES_BROWSER.".to_string()
    })?;
    let dir = user_data_dir();
    mkdir_browser_profile(&dir);
    let port = pick_port();
    let child = Command::new(&bin)
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

    poll_session_ready(child, port, dir)
}

fn browser_found_status(path: &Path, running: bool, port: Option<u16>) -> ToolResult {
    ok(&browser_status_line(path, running, port))
}

fn browser_status_line(path: &Path, running: bool, port: Option<u16>) -> String {
    format!(
        "browser: {}\nrunning: {}\nport: {}",
        path.display(),
        running,
        port.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
    )
}

fn session_status() -> (bool, Option<u16>) {
    session_from_guard(SESSION.lock())
}

fn existing_session_port(
    result: Result<
        std::sync::MutexGuard<'static, Option<BrowserSession>>,
        std::sync::PoisonError<std::sync::MutexGuard<'static, Option<BrowserSession>>>,
    >,
) -> Option<u16> {
    let (running, port) = session_from_guard(result);
    if running { port } else { None }
}

fn mkdir_browser_profile(dir: &Path) {
    mkdir_browser_profile_result(std::fs::create_dir_all(dir));
}

fn mkdir_browser_profile_result(result: std::io::Result<()>) {
    if let Err(e) = result {
        tracing::debug!(error = %e, "browser profile mkdir");
    }
}

fn store_session(child: Child, port: u16, dir: PathBuf) -> Result<u16, String> {
    if let Ok(mut g) = SESSION.lock() {
        *g = Some(BrowserSession {
            child,
            port,
            _user_data: dir,
        });
    }
    Ok(port)
}

fn poll_session_ready(mut child: Child, port: u16, mut dir: PathBuf) -> Result<u16, String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        match apply_split_poll(split_session_poll(finish_session_poll(step_session_poll(
            child, port, dir,
        )))) {
            Ok(stored) => return stored,
            Err(retry) => {
                let (c, d) = retry_or_invariant(retry)?;
                child = c;
                dir = d;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    kill_launch_timeout(&mut child);
    Err("Chromium started but CDP never became ready".into())
}

fn poll_invariant() -> Result<u16, String> {
    Err("browser session poll invariant".into())
}

fn retry_or_invariant(retry: Option<(Child, PathBuf)>) -> Result<(Child, PathBuf), String> {
    match retry {
        Some(retry) => Ok(retry),
        None => Err(poll_invariant().unwrap_err()),
    }
}

fn apply_split_poll(split: SplitPoll) -> Result<Result<u16, String>, Option<(Child, PathBuf)>> {
    match split {
        (Some(stored), None) => Ok(stored),
        (None, Some(retry)) => Err(Some(retry)),
        _ => Err(None),
    }
}

fn split_session_poll(poll: SessionPoll) -> SplitPoll {
    match poll {
        Ok(stored) => (Some(stored), None),
        Err(retry) => (None, Some(retry)),
    }
}

fn finish_session_poll(result: SessionPoll) -> SessionPoll {
    result
}

fn step_session_poll(child: Child, port: u16, dir: PathBuf) -> SessionPoll {
    next_session_poll(
        http_get(&format!("http://127.0.0.1:{port}/json/version")).is_ok(),
        child,
        port,
        dir,
    )
}

fn next_session_poll(ready: bool, child: Child, port: u16, dir: PathBuf) -> SessionPoll {
    apply_ready_session(take_ready_session(ready, child, port, dir))
}

fn take_ready_session(ready: bool, child: Child, port: u16, dir: PathBuf) -> SessionPoll {
    if ready {
        Ok(store_session(child, port, dir))
    } else {
        Err((child, dir))
    }
}

fn apply_ready_session(result: SessionPoll) -> SessionPoll {
    result
}

fn kill_launch_timeout(child: &mut Child) {
    kill_child_debug(child.kill(), "browser launch timeout kill");
}

fn session_from_guard(
    result: Result<
        std::sync::MutexGuard<'static, Option<BrowserSession>>,
        std::sync::PoisonError<std::sync::MutexGuard<'static, Option<BrowserSession>>>,
    >,
) -> (bool, Option<u16>) {
    session_from_unlocked(unlock_session(result))
}

fn unlock_session(
    result: Result<
        std::sync::MutexGuard<'static, Option<BrowserSession>>,
        std::sync::PoisonError<std::sync::MutexGuard<'static, Option<BrowserSession>>>,
    >,
) -> std::sync::MutexGuard<'static, Option<BrowserSession>> {
    unlock_session_result(result)
}

fn unlock_session_result(
    result: Result<
        std::sync::MutexGuard<'static, Option<BrowserSession>>,
        std::sync::PoisonError<std::sync::MutexGuard<'static, Option<BrowserSession>>>,
    >,
) -> std::sync::MutexGuard<'static, Option<BrowserSession>> {
    recover_lock(result)
}

fn recover_lock<T>(result: Result<T, std::sync::PoisonError<T>>) -> T {
    match result {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn session_from_unlocked(
    g: std::sync::MutexGuard<'static, Option<BrowserSession>>,
) -> (bool, Option<u16>) {
    session_fields(&g)
}

fn session_fields(g: &Option<BrowserSession>) -> (bool, Option<u16>) {
    (g.is_some(), g.as_ref().map(|s| s.port))
}

fn pick_port_fallback(err: &str, msg: &'static str) -> u16 {
    tracing::debug!(error = %err, "{msg}");
    9222
}

fn pick_bound_port(result: std::io::Result<std::net::TcpListener>) -> u16 {
    match result {
        Ok(l) => port_from_addr(l.local_addr()),
        Err(e) => pick_port_fallback(&e.to_string(), "ephemeral port bind"),
    }
}

fn port_from_addr(result: std::io::Result<std::net::SocketAddr>) -> u16 {
    match result {
        Ok(a) => a.port(),
        Err(e) => pick_port_fallback(&e.to_string(), "ephemeral port local_addr"),
    }
}

fn pick_port() -> u16 {
    #[cfg(test)]
    if let Ok(p) = std::env::var("WHYCODES_BROWSER_PORT")
        && let Ok(n) = p.parse::<u16>()
        && n != 0
    {
        return n;
    }
    pick_bound_port(std::net::TcpListener::bind("127.0.0.1:0"))
}

fn open_url(url: &str) -> ToolResult {
    match ensure_session() {
        Ok(port) => navigate_opened(port, url),
        Err(e) => err(&e),
    }
}

fn navigate_opened(port: u16, url: &str) -> ToolResult {
    match cdp(port, "Page.navigate", json!({ "url": url })) {
        Ok(_) => {
            page_enable_best_effort(port);
            ok(&format!("opened {url}"))
        }
        Err(e) => err(&e),
    }
}

fn page_enable_best_effort(port: u16) {
    page_enable_result(cdp(port, "Page.enable", json!({})));
}

fn page_enable_result(result: Result<Value, String>) {
    if let Err(e) = result {
        tracing::debug!(error = %e, "Page.enable");
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
    snapshot_from_eval(evaluate(port, expr))
}

fn snapshot_from_eval(result: Result<Value, String>) -> ToolResult {
    match result {
        Ok(v) => ok(&serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())),
        Err(e) => err(&e),
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
    click_from_eval(evaluate(port, &expr), selector)
}

fn click_from_eval(result: Result<Value, String>, selector: &str) -> ToolResult {
    match result {
        Ok(v) if v.get("ok").and_then(|b| b.as_bool()) == Some(true) => {
            ok(&format!("clicked {selector}"))
        }
        Ok(v) => err(&format!("click failed: {v}")),
        Err(e) => err(&e),
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
    type_from_eval(evaluate(port, &expr))
}

fn type_from_eval(result: Result<Value, String>) -> ToolResult {
    match result {
        Ok(v) if v.get("ok").and_then(|b| b.as_bool()) == Some(true) => ok("typed"),
        Ok(v) => err(&format!("type failed: {v}")),
        Err(e) => err(&e),
    }
}

fn clamp_wait_ms(ms: u64) -> u64 {
    ms.min(15_000)
}

fn wait_ms(ms: u64) -> ToolResult {
    let ms = clamp_wait_ms(ms);
    std::thread::sleep(Duration::from_millis(ms));
    ok(&format!("waited {ms}ms"))
}

fn screenshot(ctx: &ToolContext) -> ToolResult {
    let port = match current_port() {
        Some(p) => p,
        None => return err("no browser session — call browser open first"),
    };
    screenshot_from_cdp(
        cdp(port, "Page.captureScreenshot", json!({ "format": "png" })),
        ctx,
    )
}

fn screenshot_from_cdp(result: Result<Value, String>, ctx: &ToolContext) -> ToolResult {
    let v = match result {
        Ok(v) => v,
        Err(e) => return err(&e),
    };
    let Some(b64) = screenshot_data(&v) else {
        return err("screenshot: no data");
    };
    let bytes = match decode_screenshot(b64) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    write_screenshot_bytes(ctx, &bytes)
}

fn screenshot_mkdir_failed(e: &str) -> ToolResult {
    err(&format!("mkdir: {e}"))
}

fn screenshot_write_failed(e: &str) -> ToolResult {
    err(&format!("write screenshot: {e}"))
}

fn write_screenshot_bytes(ctx: &ToolContext, bytes: &[u8]) -> ToolResult {
    let dir = whycodes_core::project_dir(Path::new(&ctx.working_dir)).join("browser");
    if let Err(e) = mkdir_screenshot_dir(&dir) {
        return e;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("shot-{ts}.png"));
    write_screenshot_file(&path, bytes)
}

fn mkdir_screenshot_dir(dir: &Path) -> Result<(), ToolResult> {
    std::fs::create_dir_all(dir).map_err(|e| screenshot_mkdir_failed(&e.to_string()))
}

fn write_screenshot_file(path: &Path, bytes: &[u8]) -> ToolResult {
    match std::fs::write(path, bytes) {
        Ok(()) => ok(&format!("saved {}", path.display())),
        Err(e) => screenshot_write_failed(&e.to_string()),
    }
}

fn close_browser() -> ToolResult {
    let mut g = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(mut s) = g.take() {
        kill_browser_child(&mut s.child);
        wait_browser_child(&mut s.child);
        return ok("browser closed");
    }
    ok("no browser session")
}

fn current_port() -> Option<u16> {
    session_status().1
}

fn screenshot_data(v: &Value) -> Option<&str> {
    v.get("data").and_then(|d| d.as_str())
}

fn decode_screenshot(b64: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("screenshot decode: {e}"))
}

fn evaluate_js_exception(ex: &Value) -> Result<Value, String> {
    Err(format!("js exception: {ex}"))
}

fn js_exception_from(ex: &Value) -> Result<Value, String> {
    evaluate_js_exception(ex)
}

fn kill_browser_child(child: &mut Child) {
    kill_child_debug(child.kill(), "browser kill");
}

fn wait_browser_child(child: &mut Child) {
    kill_child_debug(child.wait().map(|_| ()), "browser wait");
}

fn set_http_read_timeout(stream: &TcpStream) {
    set_timeout_debug(
        stream.set_read_timeout(Some(Duration::from_secs(5))),
        "http get timeout",
    );
}

fn set_cdp_timeouts(stream: &TcpStream) {
    set_timeout_debug(
        stream.set_read_timeout(Some(Duration::from_secs(10))),
        "cdp read timeout",
    );
    set_timeout_debug(
        stream.set_write_timeout(Some(Duration::from_secs(10))),
        "cdp write timeout",
    );
}

fn kill_child_debug(result: std::io::Result<()>, msg: &'static str) {
    if let Err(e) = result {
        tracing::debug!(error = %e, "{msg}");
    }
}

fn set_timeout_debug(result: std::io::Result<()>, msg: &'static str) {
    if let Err(e) = result {
        tracing::debug!(error = %e, "{msg}");
    }
}

fn evaluate(port: u16, expression: &str) -> Result<Value, String> {
    evaluate_cdp_result(cdp(
        port,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true
        }),
    ))
}

fn evaluate_cdp_result(result: Result<Value, String>) -> Result<Value, String> {
    let v = result?;
    if let Some(ex) = v.get("exceptionDetails") {
        return js_exception_from(ex);
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
    set_http_read_timeout(&stream);
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
    set_cdp_timeouts(&stream);
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
#[allow(clippy::await_holding_lock)]
#[path = "browser_tests.rs"]
mod tests;
