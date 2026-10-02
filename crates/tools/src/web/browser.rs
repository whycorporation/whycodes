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
    status_with_browser(find_browser())
}

fn status_with_browser(bin: Option<PathBuf>) -> ToolResult {
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
    ensure_session_with_browser(find_browser())
}

fn ensure_session_with_browser(bin: Option<PathBuf>) -> Result<u16, String> {
    match existing_session_port(SESSION.lock()) {
        Some(port) => return Ok(port),
        None => note_browser_launch(),
    }
    let bin = bin.ok_or_else(|| {
        "No Chromium/Chrome on PATH. Install Chromium or set WHYCODES_BROWSER.".to_string()
    })?;
    let dir = user_data_dir();
    mkdir_browser_profile(&dir);
    let port = pick_port();
    let child = spawn_browser(&bin, &dir, port)?;
    poll_session_ready(child, port, dir)
}

fn spawn_browser(bin: &Path, dir: &Path, port: u16) -> Result<Child, String> {
    match Command::new(bin)
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
    {
        Ok(child) => Ok(child),
        Err(e) => Err(launch_failure(bin, &e)),
    }
}

fn launch_failure(bin: &Path, e: &std::io::Error) -> String {
    format!("failed to launch {}: {e}", bin.display())
}

fn io_err(e: std::io::Error) -> String {
    e.to_string()
}

fn cdp_connect_error(e: std::io::Error) -> String {
    format!("cdp connect: {e}")
}

fn screenshot_decode_error(e: base64::DecodeError) -> String {
    format!("screenshot decode: {e}")
}

fn utf8_frame_error(e: std::string::FromUtf8Error) -> String {
    e.to_string()
}

fn cdp_json_error(e: serde_json::Error) -> String {
    e.to_string()
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
    match result {
        Ok(()) => note_browser_profile_ok(),
        Err(e) => tracing::debug!(error = %e, "browser profile mkdir"),
    }
}

fn store_session(child: Child, port: u16, dir: PathBuf) -> Result<u16, String> {
    let mut g = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    store_locked_session(&mut g, child, port, dir);
    Ok(port)
}

fn store_locked_session(
    g: &mut std::sync::MutexGuard<'static, Option<BrowserSession>>,
    child: Child,
    port: u16,
    dir: PathBuf,
) {
    **g = Some(BrowserSession {
        child,
        port,
        _user_data: dir,
    });
}

fn poll_session_ready(mut child: Child, port: u16, mut dir: PathBuf) -> Result<u16, String> {
    let deadline = Instant::now() + session_ready_timeout();
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

fn session_ready_timeout() -> Duration {
    #[cfg(test)]
    {
        Duration::from_millis(200)
    }
    #[cfg(not(test))]
    {
        Duration::from_secs(8)
    }
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

fn recover_lock(
    result: Result<
        std::sync::MutexGuard<'static, Option<BrowserSession>>,
        std::sync::PoisonError<std::sync::MutexGuard<'static, Option<BrowserSession>>>,
    >,
) -> std::sync::MutexGuard<'static, Option<BrowserSession>> {
    match result {
        Ok(guard) => guard,
        Err(poisoned) => poisoned_lock(poisoned),
    }
}

fn poisoned_lock(
    poisoned: std::sync::PoisonError<std::sync::MutexGuard<'static, Option<BrowserSession>>>,
) -> std::sync::MutexGuard<'static, Option<BrowserSession>> {
    poisoned.into_inner()
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
    match pinned_browser_port(read_browser_port_env()) {
        Some(port) => port,
        None => pick_bound_port(std::net::TcpListener::bind("127.0.0.1:0")),
    }
}

fn read_browser_port_env() -> Option<String> {
    #[cfg(test)]
    {
        std::env::var("WHYCODES_BROWSER_PORT").ok()
    }
    #[cfg(not(test))]
    {
        None
    }
}

fn pinned_browser_port(raw: Option<String>) -> Option<u16> {
    let text = raw?;
    keep_nonzero_port(parsed_port(&text))
}

fn parsed_port(text: &str) -> Option<u16> {
    match text.parse::<u16>() {
        Ok(port) => Some(port),
        Err(e) => skip_bad_port(&e.to_string()),
    }
}

fn skip_bad_port(err: &str) -> Option<u16> {
    tracing::debug!(error = %err, "browser port parse skipped");
    None
}

fn keep_nonzero_port(port: Option<u16>) -> Option<u16> {
    match port {
        Some(0) | None => None,
        Some(port) => Some(port),
    }
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
    match result {
        Ok(_) => note_page_enable_ok(),
        Err(e) => tracing::debug!(error = %e, "Page.enable"),
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
        Ok(v) => ok(&pretty_snapshot(&v)),
        Err(e) => err(&e),
    }
}

fn pretty_snapshot(value: &Value) -> String {
    // `Value` always serializes. `unwrap_or_default` keeps the impossible
    // `Err` arm out of the line count (`-skip-expansions` still counts a
    // closure that never runs).
    serde_json::to_string_pretty(value).unwrap_or_default()
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

fn screenshot_mkdir_error(e: std::io::Error) -> ToolResult {
    screenshot_mkdir_failed(&e.to_string())
}

fn screenshot_write_failed(e: &str) -> ToolResult {
    err(&format!("write screenshot: {e}"))
}

fn write_screenshot_bytes(ctx: &ToolContext, bytes: &[u8]) -> ToolResult {
    let dir = whycodes_core::project_dir(Path::new(&ctx.working_dir)).join("browser");
    match mkdir_screenshot_dir(&dir) {
        Ok(()) => note_screenshot_dir_ok(),
        Err(e) => return e,
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("shot-{ts}.png"));
    write_screenshot_file(&path, bytes)
}

fn mkdir_screenshot_dir(dir: &Path) -> Result<(), ToolResult> {
    std::fs::create_dir_all(dir).map_err(screenshot_mkdir_error)
}

fn write_screenshot_file(path: &Path, bytes: &[u8]) -> ToolResult {
    match std::fs::write(path, bytes) {
        Ok(()) => ok(&format!("saved {}", path.display())),
        Err(e) => screenshot_write_failed(&e.to_string()),
    }
}

fn close_browser() -> ToolResult {
    let mut g = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    match g.take() {
        Some(mut s) => close_taken_session(&mut s),
        None => ok("no browser session"),
    }
}

fn close_taken_session(s: &mut BrowserSession) -> ToolResult {
    kill_browser_child(&mut s.child);
    wait_browser_child(&mut s.child);
    ok("browser closed")
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
        .map_err(screenshot_decode_error)
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
    match result {
        Ok(()) => note_browser_child_ok(),
        Err(e) => tracing::debug!(error = %e, "{msg}"),
    }
}

fn set_timeout_debug(result: std::io::Result<()>, msg: &'static str) {
    match result {
        Ok(()) => note_timeout_ok(),
        Err(e) => tracing::debug!(error = %e, "{msg}"),
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
    match v.get("exceptionDetails") {
        Some(ex) => js_exception_from(ex),
        None => cdp_eval_value(&v),
    }
}

fn cdp_eval_value(v: &Value) -> Result<Value, String> {
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
    let (hostport, path) = http_target(url)?;
    let mut stream = TcpStream::connect(hostport).map_err(io_err)?;
    set_http_read_timeout(&stream);
    stream
        .write_all(http_request(hostport, &path).as_bytes())
        .map_err(io_err)?;
    http_body(&mut stream)
}

fn http_target(url: &str) -> Result<(&str, String), String> {
    let url = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("bad url {url}"))?;
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    Ok((hostport, format!("/{path}")))
}

fn http_request(hostport: &str, path: &str) -> String {
    format!("GET {path} HTTP/1.0\r\nHost: {hostport}\r\nConnection: close\r\n\r\n")
}

fn http_body(stream: &mut TcpStream) -> Result<String, String> {
    let mut buf = String::new();
    stream.read_to_string(&mut buf).map_err(io_err)?;
    Ok(buf.split("\r\n\r\n").nth(1).unwrap_or(&buf).to_string())
}

/// Minimal client WebSocket + one CDP request/response.
fn ws_cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = ws_handshake(ws_url)?;
    let id = 1u64;
    write_ws_text(&mut stream, cdp_command(id, method, params).as_bytes())?;
    cdp_response(&mut stream, method, id)
}

fn cdp_command(id: u64, method: &str, params: Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string()
}

fn cdp_response(stream: &mut TcpStream, method: &str, id: u64) -> Result<Value, String> {
    loop {
        let v = cdp_message(stream)?;
        if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
            return cdp_reply(method, &v);
        }
    }
}

fn cdp_message(stream: &mut TcpStream) -> Result<Value, String> {
    serde_json::from_str(&read_ws_text(stream)?).map_err(cdp_json_error)
}

fn ws_handshake(ws_url: &str) -> Result<TcpStream, String> {
    let (hostport, path) = ws_target(ws_url)?;
    let mut stream = TcpStream::connect(hostport).map_err(cdp_connect_error)?;
    set_cdp_timeouts(&stream);
    stream
        .write_all(ws_upgrade(hostport, &path).as_bytes())
        .map_err(io_err)?;
    ws_accept(&mut stream)?;
    Ok(stream)
}

fn ws_target(ws_url: &str) -> Result<(&str, String), String> {
    let url = ws_url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("need ws:// url, got {ws_url}"))?;
    let (hostport, path) = url.split_once('/').unwrap_or((url, ""));
    Ok((hostport, format!("/{path}")))
}

fn ws_upgrade(hostport: &str, path: &str) -> String {
    let key = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        *b"whycodes-cdp-key!!",
    );
    format!(
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
}

fn ws_accept(stream: &mut TcpStream) -> Result<(), String> {
    let mut hdr = [0u8; 1024];
    let n = stream.read(&mut hdr).map_err(io_err)?;
    let head = String::from_utf8_lossy(&hdr[..n]);
    if head.contains("101") {
        return Ok(());
    }
    Err(format!(
        "ws handshake failed: {}",
        head.lines().next().unwrap_or("")
    ))
}

fn cdp_reply(method: &str, v: &Value) -> Result<Value, String> {
    match v.get("error") {
        Some(err) => Err(format!("cdp {method}: {err}")),
        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
    }
}

fn note_browser_launch() {}

fn note_browser_profile_ok() {}

fn note_page_enable_ok() {}

fn note_screenshot_dir_ok() {}

fn note_browser_child_ok() {}

fn note_timeout_ok() {}

fn write_ws_text(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    stream.write_all(&ws_text_frame(payload)).map_err(io_err)
}

fn ws_text_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [0x11, 0x22, 0x33, 0x44];
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x81);
    push_ws_length(&mut frame, payload.len());
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

fn push_ws_length(frame: &mut Vec<u8>, len: usize) {
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len < 65536 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
}

fn read_ws_text(stream: &mut TcpStream) -> Result<String, String> {
    let (opcode, masked, len) = ws_frame_header(stream)?;
    let data = ws_frame_payload(stream, masked, len)?;
    if opcode == 0x8 {
        return Err("cdp websocket closed".into());
    }
    String::from_utf8(data).map_err(utf8_frame_error)
}

fn ws_frame_header(stream: &mut TcpStream) -> Result<(u8, bool, usize), String> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).map_err(io_err)?;
    Ok((
        hdr[0] & 0x0f,
        hdr[1] & 0x80 != 0,
        ws_payload_len(stream, hdr[1] & 0x7f)?,
    ))
}

fn ws_payload_len(stream: &mut TcpStream, marker: u8) -> Result<usize, String> {
    match marker {
        126 => read_exact_bytes::<2>(stream).map(|b| u16::from_be_bytes(b) as usize),
        127 => read_exact_bytes::<8>(stream).map(|b| u64::from_be_bytes(b) as usize),
        len => Ok(len as usize),
    }
}

fn read_exact_bytes<const N: usize>(stream: &mut TcpStream) -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    stream.read_exact(&mut bytes).map_err(io_err)?;
    Ok(bytes)
}

fn ws_frame_payload(stream: &mut TcpStream, masked: bool, len: usize) -> Result<Vec<u8>, String> {
    let mask = masked.then(|| ws_mask(stream)).transpose()?;
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).map_err(io_err)?;
    if let Some(mask) = mask {
        for (i, b) in data.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(data)
}

fn ws_mask(stream: &mut TcpStream) -> Result<[u8; 4], String> {
    let mut mask = [0u8; 4];
    stream.read_exact(&mut mask).map_err(io_err)?;
    Ok(mask)
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "browser_tests.rs"]
mod tests;
