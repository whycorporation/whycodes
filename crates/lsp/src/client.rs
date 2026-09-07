use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{LspError, Result};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::types::{
    Diagnostic, HoverResult, IncomingMessage, InitializeParams, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, Location, Position, TextDocumentItem,
    TextDocumentPositionParams,
};

/// Buffered LSP client — wraps a subprocess and talks JSON-RPC over stdin/stdout
/// using `Content-Length`-delimited framing.
pub struct LspClient {
    writer: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    /// Server process handle. Never read directly: owned here so the `Child`
    /// outlives the client (the server exits on its own once stdin closes).
    #[allow(dead_code)]
    child: Arc<Mutex<Child>>,
    next_id: Arc<Mutex<i64>>,
    /// Stored diagnostics per URI, updated from `textDocument/publishDiagnostics`
    /// notifications.
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    language_id: String,
    settings: Option<serde_json::Value>,
    last_used: Arc<Mutex<std::time::Instant>>,
}

impl LspClient {
    /// Start a language server process, send `initialize`, then `initialized`.
    pub async fn start(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
    ) -> Result<Self> {
        Self::start_with(
            command,
            args,
            workspace_root,
            language_id,
            None,
            None,
            Self::spawn_background(cfg!(not(test))),
        )
        .await
    }

    /// Start with optional `initializationOptions` and `workspace/didChangeConfiguration`.
    pub async fn start_configured(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
        init_options: Option<&serde_json::Value>,
        settings: Option<&serde_json::Value>,
    ) -> Result<Self> {
        Self::start_with(
            command,
            args,
            workspace_root,
            language_id,
            init_options,
            settings,
            Self::spawn_background(cfg!(not(test))),
        )
        .await
    }

    fn spawn_background(for_production: bool) -> bool {
        for_production
    }

    async fn start_with(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
        init_options: Option<&serde_json::Value>,
        settings: Option<&serde_json::Value>,
        spawn_background: bool,
    ) -> Result<Self> {
        Self::boot(
            command,
            args,
            workspace_root,
            language_id,
            init_options,
            settings,
            spawn_background,
        )
        .await
    }

    async fn boot(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
        init_options: Option<&serde_json::Value>,
        settings: Option<&serde_json::Value>,
        spawn_background: bool,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(test)]
        cmd.kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| LspError::msg(format!("Failed to spawn LSP server {command}: {e}")))?;

        let stdin = take_child_pipe(child.stdin.take(), "stdin")?;
        let stdout = take_child_pipe(child.stdout.take(), "stdout")?;

        let writer = Arc::new(Mutex::new(stdin));
        let reader = Arc::new(Mutex::new(BufReader::new(stdout)));
        let child = Arc::new(Mutex::new(child));

        let client = Self {
            writer,
            stdout: reader,
            child,
            next_id: Arc::new(Mutex::new(1)),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            language_id: language_id.to_string(),
            settings: settings.cloned(),
            last_used: Arc::new(Mutex::new(std::time::Instant::now())),
        };

        // Send initialize
        let init_params = InitializeParams::with_options(workspace_root, init_options);
        let _init_resp = client
            .request("initialize", init_params.inner)
            .await
            .map_err(|e| LspError::msg(format!("initialize request failed: {e}")))?;

        // Send initialized notification
        #[cfg(test)]
        if args.iter().any(|a| a == "close_stdin") {
            client.kill_for_test().await;
        }
        client
            .notify("initialized", json!({}))
            .await
            .map_err(|e| LspError::msg(format!("initialized notification failed: {e}")))?;

        if let Some(settings) = settings {
            client
                .notify(
                    "workspace/didChangeConfiguration",
                    json!({ "settings": settings }),
                )
                .await
                .map_err(|e| LspError::msg(format!("didChangeConfiguration failed: {e}")))?;
        }

        info!("LSP server {command} initialized for {}", workspace_root);

        if spawn_background {
            // Spawn a background reader loop to collect diagnostics (and swallow
            // window/showMessage etc.).
            let stdout_bg = client.stdout.clone();
            let diags_bg = client.diagnostics.clone();
            let cmd_name = command.to_string();
            tokio::spawn(async move {
                consume_stdout(stdout_bg, diags_bg, cmd_name).await;
            });
        }

        Ok(client)
    }

    #[cfg(test)]
    pub(crate) async fn start_without_reader(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
    ) -> Result<Self> {
        Self::boot(
            command,
            args,
            workspace_root,
            language_id,
            None,
            None,
            false,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn start_with_reader(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
    ) -> Result<Self> {
        Self::boot(command, args, workspace_root, language_id, None, None, true).await
    }

    pub fn mark_used(&self) {
        if let Ok(mut t) = self.last_used.try_lock() {
            *t = std::time::Instant::now();
        }
    }

    pub fn idle_for(&self) -> std::time::Duration {
        self.last_used
            .try_lock()
            .map(|t| t.elapsed())
            .unwrap_or(std::time::Duration::ZERO)
    }

    pub fn settings(&self) -> Option<&serde_json::Value> {
        self.settings.as_ref()
    }

    #[cfg(test)]
    pub(crate) async fn kill_for_test(&self) {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    /// Open a text document in the language server.
    ///
    /// When `text` is `None`, the file is read from disk. Missing files open as empty.
    pub async fn open_document(&self, uri: &str, text: Option<&str>) -> Result<()> {
        let body = match text {
            Some(t) => t.to_string(),
            None => read_uri_text(uri).unwrap_or_default(),
        };
        let doc = TextDocumentItem {
            uri: uri.to_string(),
            language_id: self.language_id.clone(),
            version: 1,
            text: body,
        };
        self.notify("textDocument/didOpen", json!({ "textDocument": doc }))
            .await
    }

    /// Fetch diagnostics for a URI. Returns cached diagnostics if previously
    /// updated via `textDocument/publishDiagnostics`, or explicitly requests
    /// them.
    pub async fn get_diagnostics(&self, uri: &str) -> Result<Vec<Diagnostic>> {
        // Try cached first
        {
            let diags = self.diagnostics.lock().await;
            if let Some(d) = diags.get(uri)
                && !d.is_empty()
            {
                return Ok(d.clone());
            }
        }

        // Send textDocument/diagnostic request
        let params = json!({
            "textDocument": { "uri": uri }
        });
        let resp = self
            .request("textDocument/diagnostic", params)
            .await
            .map_err(|e| LspError::msg(format!("textDocument/diagnostic request failed: {e}")))?;

        if let Some(result) = resp.result {
            let diagnostics: Vec<Diagnostic> = serde_json::from_value(result).unwrap_or_default();
            self.diagnostics
                .lock()
                .await
                .insert(uri.to_string(), diagnostics.clone());
            Ok(diagnostics)
        } else {
            Ok(vec![])
        }
    }

    /// Get hover information at a file position.
    pub async fn hover(&self, uri: &str, position: Position) -> Result<Option<HoverResult>> {
        let params = TextDocumentPositionParams {
            text_document: crate::types::TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request("textDocument/hover", serde_json::to_value(&params)?)
            .await?;
        parse_hover_result(resp.result)
    }

    /// Go to definition.
    pub async fn definition(&self, uri: &str, position: Position) -> Result<Vec<Location>> {
        let params = TextDocumentPositionParams {
            text_document: crate::types::TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request("textDocument/definition", serde_json::to_value(&params)?)
            .await?;
        parse_locations(resp.result)
    }

    /// Find references.
    pub async fn references(&self, uri: &str, position: Position) -> Result<Vec<Location>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": position,
            "context": { "includeDeclaration": false }
        });
        let resp = self.request("textDocument/references", params).await?;
        parse_locations(resp.result)
    }

    /// Go to type definition.
    pub async fn type_definition(&self, uri: &str, position: Position) -> Result<Vec<Location>> {
        let params = TextDocumentPositionParams {
            text_document: crate::types::TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request(
                "textDocument/typeDefinition",
                serde_json::to_value(&params)?,
            )
            .await?;
        parse_locations(resp.result)
    }

    /// Go to implementation.
    pub async fn implementation(&self, uri: &str, position: Position) -> Result<Vec<Location>> {
        let params = TextDocumentPositionParams {
            text_document: crate::types::TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request(
                "textDocument/implementation",
                serde_json::to_value(&params)?,
            )
            .await?;
        parse_locations(resp.result)
    }

    /// Document symbols.
    pub async fn document_symbols(&self, uri: &str) -> Result<serde_json::Value> {
        let params = json!({ "textDocument": { "uri": uri } });
        let resp = self.request("textDocument/documentSymbol", params).await?;
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    /// Workspace symbol search.
    pub async fn workspace_symbols(&self, query: &str) -> Result<serde_json::Value> {
        let params = json!({ "query": query });
        let resp = self.request("workspace/symbol", params).await?;
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    // ── Low-level JSON-RPC helpers ───────────────────────────────────────────

    async fn send_message(&self, msg: &serde_json::Value) -> Result<()> {
        let mut writer = self.writer.lock().await;
        write_framed(&mut *writer, msg).await
    }

    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = JsonRpcNotification::new(method, params);
        self.send_message(&serde_json::to_value(&notif)?).await
    }

    async fn request(&self, method: &str, params: serde_json::Value) -> Result<JsonRpcResponse> {
        let id = {
            let mut lock = self.next_id.lock().await;
            let id = *lock;
            *lock += 1;
            id
        };
        let req = JsonRpcRequest::new(id, method, params);
        self.send_message(&serde_json::to_value(&req)?).await?;

        // Read back response matching the id.
        // We read from stdout using Content-Length framing.
        let mut reader = self.stdout.lock().await;
        let mut header = String::new();
        loop {
            header.clear();
            let n = reader.read_line(&mut header).await?;
            if n == 0 {
                return Err(LspError::msg(format!(
                    "LSP server closed stdout while waiting for response to {method}"
                )));
            }
            let trimmed = header.trim_end_matches('\n').trim_end_matches('\r');
            if let Some(len_str) = line_strip_prefix("Content-Length: ", trimmed) {
                let len: usize = len_str
                    .trim()
                    .parse()
                    .map_err(|_| LspError::msg(format!("bad Content-Length: {len_str}")))?;
                // Read the blank separator line after Content-Length
                let mut sep = String::new();
                reader.read_line(&mut sep).await?;
                // Read body
                let mut body = vec![0u8; len];
                reader.read_exact(&mut body).await?;
                let body_str = String::from_utf8_lossy(&body).to_string();
                let resp: JsonRpcResponse = serde_json::from_str(&body_str)?;
                if resp.id == Some(id) {
                    return Ok(resp);
                }
                // Not our response — could be a server notification or response
                // to another request.  For simplicity we ignore unmatched ids,
                // but a production client would queue them.
                debug!(
                    "Ignoring LSP response with id {:?}, waiting for {id}",
                    resp.id
                );
            }
            // Otherwise it's a non-Content-Length line, keep reading.
        }
    }
}

/// Check if a LSP executable exists in PATH.
pub fn command_available(cmd: &str) -> bool {
    crate::detect::which_command(cmd).is_some()
}

#[cfg(test)]
fn command_available_with(which: &str, cmd: &str) -> bool {
    std::process::Command::new(which)
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn read_uri_text(uri: &str) -> Option<String> {
    let path = crate::detect::path_from_file_uri(uri)?;
    std::fs::read_to_string(path).ok()
}

fn take_child_pipe<T>(pipe: Option<T>, name: &str) -> Result<T> {
    pipe.ok_or_else(|| LspError::msg(format!("no {name} on LSP child")))
}

/// Resolve a language server command from a file extension (built-in defaults).
pub fn language_server_for_extension(ext: &str) -> Option<(String, Vec<String>)> {
    let settings = crate::config::LspSettings::builtins();
    let (_, spec) = settings.spec_for_ext(ext)?;
    Some((spec.command.clone()?, spec.args.clone()))
}

/// Language ID for a file extension.
pub fn language_id_for_extension(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "go" => "go",
        "c" => "c",
        "cpp" | "cc" | "cxx" => "cpp",
        "h" => "c",
        "hpp" | "hxx" => "cpp",
        "java" => "java",
        "cs" => "csharp",
        "lua" => "lua",
        "zig" => "zig",
        "swift" => "swift",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" => "markdown",
        "sh" | "bash" => "shellscript",
        _ => "plaintext",
    }
}

#[cfg(test)]
pub(crate) const FAKE_LSP_PY: &str = r#"
import json, os, sys

def read_msg():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        if b":" not in line:
            continue
        k, v = line.decode("utf-8").split(":", 1)
        headers[k.strip().lower()] = v.strip()
    n = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(n)
    if not body:
        return None
    return json.loads(body)

def write_msg(obj):
    raw = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(raw)}\r\n\r\n".encode("ascii") + raw)
    sys.stdout.buffer.flush()

mode = sys.argv[1] if len(sys.argv) > 1 else "ok"
range0 = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
loc = {"uri": "file:///tmp/a.rs", "range": range0}
diag = {
    "range": range0,
    "severity": "error",
    "message": "boom",
    "source": "fake",
}

while True:
    msg = read_msg()
    if msg is None:
        break
    method = msg.get("method")
    mid = msg.get("id")
    if method == "initialize":
        if mode == "bad_len":
            sys.stdout.buffer.write(b"Content-Length: notanumber\r\n\r\n")
            sys.stdout.buffer.flush()
            break
        if mode == "close_stdin":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": {}}})
            try:
                sys.stdin.close()
            except Exception:
                pass
            import time
            time.sleep(2)
            break
        if mode == "die_after_init":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": {}}})
            os._exit(0)
        if mode == "hang_init":
            import time
            time.sleep(2)
        write_msg({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": {}}})
        extras = {
            "diag_notify", "bad_json", "response_notify", "empty_line",
            "other_notify", "diag_no_params", "bg_noise", "unparsed_diag",
            "partial_line", "trunc_body", "trunc_sep", "bg_bad_len",
        }
        if mode == "diag_notify":
            write_msg({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"uri": "file:///tmp/a.rs", "diagnostics": [diag]},
            })
        if mode == "bad_json":
            raw = b"{not json"
            sys.stdout.buffer.write(f"Content-Length: {len(raw)}\r\n\r\n".encode("ascii") + raw)
            sys.stdout.buffer.flush()
        if mode == "response_notify":
            write_msg({"jsonrpc": "2.0", "id": 7, "result": {}})
        if mode == "empty_line":
            sys.stdout.buffer.write(b"\n")
            sys.stdout.buffer.flush()
        if mode == "other_notify":
            write_msg({"jsonrpc": "2.0", "method": "window/showMessage", "params": {"message": "hi"}})
        if mode == "diag_no_params":
            write_msg({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics"})
        if mode == "bg_noise":
            sys.stdout.buffer.write(b"noise-line\r\n")
            sys.stdout.buffer.flush()
            write_msg({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"uri": "file:///tmp/a.rs", "diagnostics": [diag]},
            })
        if mode == "bg_bad_len":
            sys.stdout.buffer.write(b"Content-Length: xyz\r\n\r\n")
            sys.stdout.buffer.flush()
            write_msg({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"uri": "file:///tmp/a.rs", "diagnostics": [diag]},
            })
        if mode == "unparsed_diag":
            write_msg({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": "not-an-object",
            })
        if mode == "partial_line":
            sys.stdout.buffer.write(b"partial-without-newline")
            sys.stdout.buffer.flush()
        if mode == "trunc_body":
            sys.stdout.buffer.write(b"Content-Length: 40\r\n\r\n{}")
            sys.stdout.buffer.flush()
        if mode == "trunc_sep":
            sys.stdout.buffer.write(b"Content-Length: 2\r\n")
            sys.stdout.buffer.flush()
        if mode in extras:
            break
        continue
    if method == "initialized" or method == "textDocument/didOpen":
        if method == "initialized" and mode == "init_then_eof":
            break
        if method == "textDocument/didOpen" and mode == "fail_after_open":
            break
        continue
    if mid is None:
        continue
    if mode == "hang":
        import time
        time.sleep(2)
    if mode == "bad_resp":
        raw = b"{not json"
        sys.stdout.buffer.write(f"Content-Length: {len(raw)}\r\n\r\n".encode("ascii") + raw)
        sys.stdout.buffer.flush()
        continue
    if method == "textDocument/hover":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": None})
        elif mode == "skip_id":
            write_msg({"jsonrpc": "2.0", "id": mid + 99, "result": {"contents": "wrong"}})
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"contents": "hello"}})
        elif mode == "noise":
            sys.stdout.buffer.write(b"not-a-frame\r\n")
            sys.stdout.buffer.flush()
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"contents": "hello"}})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"contents": "hello"}})
    elif method == "textDocument/definition":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": []})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": loc})
    elif method == "textDocument/references":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": []})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": [loc]})
    elif method == "textDocument/typeDefinition" or method == "textDocument/implementation":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": []})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": loc})
    elif method == "textDocument/documentSymbol":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": []})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": [{"name": "main", "kind": 12, "range": range0, "selectionRange": range0}]})
    elif method == "workspace/symbol":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": []})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": [{"name": "main", "kind": 12, "location": loc}]})
    elif method == "textDocument/diagnostic":
        if mode == "empty":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": []})
        elif mode == "no_result":
            write_msg({"jsonrpc": "2.0", "id": mid})
        elif mode == "bad_diag":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"not": "array"}})
        else:
            write_msg({"jsonrpc": "2.0", "id": mid, "result": [diag]})
    else:
        write_msg({"jsonrpc": "2.0", "id": mid, "result": {}})
"#;

#[cfg(test)]
pub(crate) fn test_python() -> &'static str {
    for cmd in ["python3", "python", "py"] {
        if crate::detect::which_command(cmd).is_some() {
            return cmd;
        }
    }
    "python3"
}

#[cfg(test)]
pub(crate) async fn start_test_client(mode: &str, spawn_background: bool) -> Result<LspClient> {
    let args = vec!["-c".into(), FAKE_LSP_PY.into(), mode.into()];
    let py = test_python();
    if spawn_background {
        LspClient::start_with_reader(py, &args, "/tmp", "rust").await
    } else {
        LspClient::start_without_reader(py, &args, "/tmp", "rust").await
    }
}

async fn consume_stdout<R>(
    stdout: Arc<Mutex<R>>,
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    cmd_name: String,
) where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut lock = stdout.lock().await;
    let mut buf = String::new();
    loop {
        buf.clear();
        match lock.read_line(&mut buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                if buf.ends_with('\n') {
                    // Strip trailing newline — read_line includes it.
                    let line = buf.trim_end_matches('\n').trim_end_matches('\r');
                    if line.is_empty() {
                        continue;
                    }
                    // Try to read a Content-Length header.
                    if let Some(len_str) = line_strip_prefix("Content-Length: ", line) {
                        let len: usize = match len_str.trim().parse() {
                            Ok(l) => l,
                            Err(_) => continue,
                        };
                        // Read and skip the blank separator line.
                        let mut sep = String::new();
                        if lock.read_line(&mut sep).await.is_err() {
                            break;
                        }
                        // Read `len` bytes of JSON body.
                        let mut body = vec![0u8; len];
                        if lock.read_exact(&mut body).await.is_err() {
                            break;
                        }
                        let body_str = String::from_utf8_lossy(&body).to_string();
                        match IncomingMessage::from_line(&body_str) {
                            Ok(IncomingMessage::Notification(notif)) => {
                                if notif.method == "textDocument/publishDiagnostics"
                                    && let Some(params) = notif.params
                                    && let Ok(parsed) = serde_json::from_value::<
                                        crate::types::PublishDiagnosticsParams,
                                    >(params)
                                {
                                    diagnostics
                                        .lock()
                                        .await
                                        .insert(parsed.uri, parsed.diagnostics);
                                }
                            }
                            Ok(IncomingMessage::Response(_)) => {
                                // silently discard — handled by `request()`
                            }
                            Err(e) => {
                                warn!("Bad LSP message from {}: {}", cmd_name, e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("LSP stdout read error ({}): {}", cmd_name, e);
                break;
            }
        }
    }
    debug!("LSP background reader for {} exited", cmd_name);
}
fn line_strip_prefix<'a>(prefix: &str, line: &'a str) -> Option<&'a str> {
    line.strip_prefix(prefix)
}

pub(crate) fn encode_lsp_frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

pub(crate) async fn write_framed<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &serde_json::Value,
) -> Result<()> {
    let body = serde_json::to_string(msg)?;
    writer.write_all(&encode_lsp_frame(&body)).await?;
    writer.flush().await?;
    Ok(())
}

pub(crate) fn parse_hover_result(result: Option<serde_json::Value>) -> Result<Option<HoverResult>> {
    match result {
        Some(result) if !result.is_null() => Ok(Some(serde_json::from_value(result)?)),
        _ => Ok(None),
    }
}

pub(crate) fn parse_locations(result: Option<serde_json::Value>) -> Result<Vec<Location>> {
    match result {
        Some(result) if !result.is_null() => {
            if result.is_array() {
                Ok(serde_json::from_value(result)?)
            } else {
                Ok(vec![serde_json::from_value(result)?])
            }
        }
        _ => Ok(vec![]),
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
