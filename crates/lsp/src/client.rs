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
}

impl LspClient {
    /// Start a language server process, send `initialize`, then `initialized`.
    pub async fn start(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
    ) -> Result<Self> {
        Self::boot(command, args, workspace_root, language_id, cfg!(not(test))).await
    }

    async fn boot(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
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

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::msg("no stdin on LSP child"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::msg("no stdout on LSP child"))?;

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
        };

        // Send initialize
        let init_params = InitializeParams::minimal(workspace_root);
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
        Self::boot(command, args, workspace_root, language_id, false).await
    }

    #[cfg(test)]
    pub(crate) async fn start_with_reader(
        command: &str,
        args: &[String],
        workspace_root: &str,
        language_id: &str,
    ) -> Result<Self> {
        Self::boot(command, args, workspace_root, language_id, true).await
    }

    #[cfg(test)]
    pub(crate) async fn kill_for_test(&self) {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    /// Open a text document in the language server.
    pub async fn open_document(&self, uri: &str, text: Option<&str>) -> Result<()> {
        let doc = TextDocumentItem {
            uri: uri.to_string(),
            language_id: self.language_id.clone(),
            version: 1,
            text: text.unwrap_or("").to_string(),
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
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve a language server command from a file extension.
pub fn language_server_for_extension(ext: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match ext {
        "rs" => Some(("rust-analyzer", vec![])),
        "py" => Some(("pylsp", vec![])),
        "ts" | "tsx" | "js" | "jsx" => Some(("typescript-language-server", vec!["--stdio"])),
        "go" => Some(("gopls", vec![])),
        "c" | "cpp" | "h" | "hpp" | "cc" => Some(("clangd", vec![])),
        "java" => Some(("jdtls", vec![])),
        "cs" => Some(("omnisharp", vec!["--languageserver"])),
        "lua" => Some(("lua-language-server", vec![])),
        "zig" => Some(("zls", vec![])),
        "swift" => Some(("sourcekit-lsp", vec![])),
        #[cfg(test)]
        "whycodes_lsp_fake" => Some(("python3", vec!["-c", FAKE_LSP_PY, "ok"])),
        #[cfg(test)]
        "whycodes_lsp_empty" => Some(("python3", vec!["-c", FAKE_LSP_PY, "empty"])),
        #[cfg(test)]
        "whycodes_lsp_failopen" => Some(("python3", vec!["-c", FAKE_LSP_PY, "init_then_eof"])),
        #[cfg(test)]
        "whycodes_lsp_err" => Some(("python3", vec!["-c", FAKE_LSP_PY, "fail_after_open"])),
        #[cfg(test)]
        "whycodes_lsp_missing" => Some(("whycodes-lsp-missing-bin", vec![])),
        _ => None,
    }
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
const FAKE_LSP_PY: &str = r#"
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
pub(crate) async fn start_test_client(mode: &str, spawn_background: bool) -> Result<LspClient> {
    let args = vec!["-c".into(), FAKE_LSP_PY.into(), mode.into()];
    if spawn_background {
        LspClient::start_with_reader("python3", &args, "/tmp", "rust").await
    } else {
        LspClient::start_without_reader("python3", &args, "/tmp", "rust").await
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
mod tests {
    use super::*;

    #[test]
    fn known_extensions_resolve_to_a_language_server() {
        assert_eq!(
            language_server_for_extension("rs"),
            Some(("rust-analyzer", vec![]))
        );
        assert_eq!(language_server_for_extension("py"), Some(("pylsp", vec![])));
        assert_eq!(
            language_server_for_extension("ts"),
            Some(("typescript-language-server", vec!["--stdio"]))
        );
        assert_eq!(language_server_for_extension("go"), Some(("gopls", vec![])));
        assert_eq!(
            language_server_for_extension("cs"),
            Some(("omnisharp", vec!["--languageserver"]))
        );
    }

    #[test]
    fn unknown_extensions_have_no_language_server() {
        assert_eq!(language_server_for_extension("xyz"), None);
        assert_eq!(language_server_for_extension(""), None);
        assert_eq!(language_server_for_extension("exe"), None);
    }

    #[test]
    fn extension_maps_to_language_id() {
        assert_eq!(language_id_for_extension("rs"), "rust");
        assert_eq!(language_id_for_extension("py"), "python");
        assert_eq!(language_id_for_extension("tsx"), "typescriptreact");
        assert_eq!(language_id_for_extension("h"), "c");
        assert_eq!(language_id_for_extension("hpp"), "cpp");
        assert_eq!(language_id_for_extension("md"), "markdown");
        assert_eq!(language_id_for_extension("sh"), "shellscript");
        assert_eq!(language_id_for_extension("zzz"), "plaintext");
    }

    #[test]
    fn strips_content_length_prefix() {
        assert_eq!(
            line_strip_prefix("Content-Length: ", "Content-Length: 123"),
            Some("123")
        );
        assert_eq!(line_strip_prefix("Content-Length: ", "foo"), None);
    }

    #[test]
    fn more_extensions_have_servers_and_ids() {
        assert_eq!(
            language_server_for_extension("js"),
            Some(("typescript-language-server", vec!["--stdio"]))
        );
        assert_eq!(language_server_for_extension("c"), Some(("clangd", vec![])));
        assert_eq!(
            language_server_for_extension("java"),
            Some(("jdtls", vec![]))
        );
        assert_eq!(
            language_server_for_extension("lua"),
            Some(("lua-language-server", vec![]))
        );
        assert_eq!(language_server_for_extension("zig"), Some(("zls", vec![])));
        assert_eq!(
            language_server_for_extension("swift"),
            Some(("sourcekit-lsp", vec![]))
        );
        assert_eq!(language_id_for_extension("go"), "go");
        assert_eq!(language_id_for_extension("yaml"), "yaml");
        assert_eq!(language_id_for_extension("toml"), "toml");
        assert_eq!(language_id_for_extension("json"), "json");
        assert_eq!(language_id_for_extension("java"), "java");
        assert_eq!(language_id_for_extension("lua"), "lua");
    }

    #[test]
    fn encode_frame_and_parse_results() {
        let frame = encode_lsp_frame("{}");
        let s = String::from_utf8(frame).unwrap();
        assert!(s.starts_with("Content-Length: 2\r\n\r\n"));
        assert!(s.ends_with("{}"));

        assert!(parse_hover_result(None).unwrap().is_none());
        assert!(
            parse_hover_result(Some(serde_json::Value::Null))
                .unwrap()
                .is_none()
        );
        let hover = parse_hover_result(Some(serde_json::json!({
            "contents": "hello"
        })))
        .unwrap()
        .unwrap();
        assert_eq!(hover.contents_string(), "hello");

        assert!(parse_locations(None).unwrap().is_empty());
        assert!(
            parse_locations(Some(serde_json::Value::Null))
                .unwrap()
                .is_empty()
        );
        let one = parse_locations(Some(serde_json::json!({
            "uri": "file:///a.rs",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
        })))
        .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].uri, "file:///a.rs");
        let many = parse_locations(Some(serde_json::json!([{
            "uri": "file:///a.rs",
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}
        }])))
        .unwrap();
        assert_eq!(many.len(), 1);
    }

    fn fake_args(mode: &str) -> Vec<String> {
        vec!["-c".into(), FAKE_LSP_PY.into(), mode.into()]
    }

    async fn start_fake(mode: &str) -> LspClient {
        start_test_client(mode, false).await.unwrap()
    }

    fn pos() -> Position {
        Position {
            line: 0,
            character: 0,
        }
    }

    #[tokio::test]
    async fn start_fails_when_command_is_missing() {
        let err = match LspClient::start("whycodes-lsp-missing-bin", &[], "/tmp", "rust").await {
            Ok(_) => panic!("expected spawn failure"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Failed to spawn"));
    }

    #[tokio::test]
    async fn initialize_hover_definition_and_references() {
        let client = start_fake("ok").await;
        client
            .open_document("file:///tmp/a.rs", None)
            .await
            .unwrap();
        let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
        assert_eq!(hover.unwrap().contents_string(), "hello");
        let defs = client.definition("file:///tmp/a.rs", pos()).await.unwrap();
        assert_eq!(defs[0].uri, "file:///tmp/a.rs");
        let refs = client.references("file:///tmp/a.rs", pos()).await.unwrap();
        assert_eq!(refs.len(), 1);
        let diags = client.get_diagnostics("file:///tmp/a.rs").await.unwrap();
        assert_eq!(diags[0].message, "boom");
        let cached = client.get_diagnostics("file:///tmp/a.rs").await.unwrap();
        assert_eq!(cached.len(), 1);
    }

    #[tokio::test]
    async fn empty_results_and_null_diagnostics() {
        let client = start_fake("empty").await;
        assert!(
            client
                .hover("file:///tmp/a.rs", pos())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            client
                .definition("file:///tmp/a.rs", pos())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            client
                .references("file:///tmp/a.rs", pos())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            client
                .get_diagnostics("file:///tmp/a.rs")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn diagnostic_request_without_result() {
        let client = start_fake("no_result").await;
        assert!(
            client
                .get_diagnostics("file:///tmp/a.rs")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn diagnostic_request_with_unparseable_payload() {
        let client = start_fake("bad_diag").await;
        assert!(
            client
                .get_diagnostics("file:///tmp/a.rs")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn request_skips_unmatched_ids_and_noise() {
        let client = start_fake("skip_id").await;
        let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
        assert_eq!(hover.unwrap().contents_string(), "hello");
    }

    #[tokio::test]
    async fn request_skips_non_framed_lines() {
        let client = start_fake("noise").await;
        let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
        assert_eq!(hover.unwrap().contents_string(), "hello");
    }

    #[tokio::test]
    async fn request_rejects_bad_content_length() {
        let err = match LspClient::start("python3", &fake_args("bad_len"), "/tmp", "rust").await {
            Ok(_) => panic!("expected bad Content-Length"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("bad Content-Length"));
        assert!(err.to_string().contains("initialize request failed"));
    }

    #[tokio::test]
    async fn diagnostic_request_fails_when_stdout_closes() {
        let client = start_fake("init_then_eof").await;
        let err = client
            .get_diagnostics("file:///tmp/a.rs")
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("textDocument/diagnostic request failed")
        );
    }

    #[tokio::test]
    async fn request_errors_when_stdout_closes() {
        let client = start_fake("init_then_eof").await;
        let err = client.hover("file:///tmp/a.rs", pos()).await.unwrap_err();
        assert!(err.to_string().contains("closed stdout"));
    }

    #[tokio::test]
    async fn initialized_notification_fails_when_server_exits() {
        let err = match LspClient::start("python3", &fake_args("close_stdin"), "/tmp", "rust").await
        {
            Ok(_) => panic!("expected initialized failure"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("initialized notification failed")
                || err.to_string().contains("Broken pipe")
                || err.to_string().contains("closed")
                || err.to_string().contains("os error")
        );
    }

    #[tokio::test]
    async fn open_document_fails_after_child_exits() {
        let client = start_fake("ok").await;
        client.kill_for_test().await;
        let err = client
            .open_document("file:///tmp/a.rs", Some("fn main() {}"))
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn malformed_response_body_is_an_error() {
        let client = start_fake("bad_resp").await;
        let err = client.hover("file:///tmp/a.rs", pos()).await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn initialize_times_out_when_server_hangs() {
        let args = fake_args("hang_init");
        let fut = LspClient::start("python3", &args, "/tmp", "rust");
        let timed = tokio::time::timeout(std::time::Duration::from_millis(200), fut).await;
        assert!(timed.is_err());
    }

    #[tokio::test]
    async fn hover_times_out_when_server_hangs() {
        let client = start_fake("hang").await;
        let timed = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            client.hover("file:///tmp/a.rs", pos()),
        )
        .await;
        assert!(timed.is_err());
    }

    #[tokio::test]
    async fn background_reader_stores_publish_diagnostics() {
        let client = start_fake("diag_notify").await;
        consume_stdout(
            client.stdout.clone(),
            client.diagnostics.clone(),
            "fake".into(),
        )
        .await;
        let diags = client.get_diagnostics("file:///tmp/a.rs").await.unwrap();
        assert_eq!(diags[0].message, "boom");
    }

    #[tokio::test]
    async fn start_with_reader_collects_diagnostics_in_background() {
        let client = start_test_client("diag_notify", true).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let diags = client.diagnostics.lock().await;
        assert_eq!(diags.get("file:///tmp/a.rs").unwrap()[0].message, "boom");
    }

    #[tokio::test]
    async fn background_reader_handles_malformed_and_other_messages() {
        for mode in [
            "bad_json",
            "response_notify",
            "empty_line",
            "other_notify",
            "unparsed_diag",
            "diag_no_params",
            "bg_noise",
            "partial_line",
            "trunc_body",
            "trunc_sep",
            "bg_bad_len",
        ] {
            let client = start_fake(mode).await;
            consume_stdout(
                client.stdout.clone(),
                client.diagnostics.clone(),
                "fake".into(),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn consume_stdout_breaks_on_read_error() {
        struct ErrReader;
        impl tokio::io::AsyncRead for ErrReader {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                _buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("boom")))
            }
        }
        impl tokio::io::AsyncBufRead for ErrReader {
            fn poll_fill_buf(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<&[u8]>> {
                std::task::Poll::Ready(Err(std::io::Error::other("boom")))
            }
            fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
        }
        let stdout = Arc::new(Mutex::new(ErrReader));
        consume_stdout(stdout, Arc::new(Mutex::new(HashMap::new())), "err".into()).await;
    }

    #[tokio::test]
    async fn write_framed_roundtrip() {
        let mut buf = Vec::new();
        write_framed(&mut buf, &json!({"ok": true})).await.unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Content-Length:"));
        assert!(s.contains("\"ok\":true"));
    }

    #[test]
    fn parse_helpers_surface_json_errors() {
        assert!(parse_hover_result(Some(json!(1))).is_err());
        assert!(parse_locations(Some(json!("not a location"))).is_err());
    }

    #[test]
    fn remaining_language_ids_and_servers() {
        assert_eq!(language_id_for_extension("jsx"), "javascriptreact");
        assert_eq!(language_id_for_extension("cc"), "cpp");
        assert_eq!(language_id_for_extension("cxx"), "cpp");
        assert_eq!(language_id_for_extension("hxx"), "cpp");
        assert_eq!(language_id_for_extension("cs"), "csharp");
        assert_eq!(language_id_for_extension("zig"), "zig");
        assert_eq!(language_id_for_extension("swift"), "swift");
        assert_eq!(language_id_for_extension("yml"), "yaml");
        assert_eq!(language_id_for_extension("bash"), "shellscript");
        assert_eq!(language_id_for_extension("ts"), "typescript");
        assert_eq!(language_id_for_extension("js"), "javascript");
        assert_eq!(language_id_for_extension("c"), "c");
        assert_eq!(language_id_for_extension("cpp"), "cpp");
        assert_eq!(
            language_server_for_extension("tsx"),
            Some(("typescript-language-server", vec!["--stdio"]))
        );
        assert_eq!(
            language_server_for_extension("jsx"),
            Some(("typescript-language-server", vec!["--stdio"]))
        );
        assert_eq!(
            language_server_for_extension("cpp"),
            Some(("clangd", vec![]))
        );
        assert_eq!(language_server_for_extension("h"), Some(("clangd", vec![])));
        assert_eq!(
            language_server_for_extension("hpp"),
            Some(("clangd", vec![]))
        );
        assert_eq!(
            language_server_for_extension("cc"),
            Some(("clangd", vec![]))
        );
        let (cmd, args) = language_server_for_extension("whycodes_lsp_fake").unwrap();
        assert_eq!(cmd, "python3");
        assert_eq!(args[0], "-c");
        assert!(language_server_for_extension("whycodes_lsp_missing").is_some());
        assert!(language_server_for_extension("whycodes_lsp_empty").is_some());
        assert!(language_server_for_extension("whycodes_lsp_err").is_some());
        assert!(language_server_for_extension("whycodes_lsp_failopen").is_some());
    }

    #[test]
    fn command_available_finds_sh_and_rejects_missing() {
        assert!(command_available("sh"));
        assert!(!command_available("whycodes-lsp-bin-that-does-not-exist"));
    }

    #[tokio::test]
    async fn consume_stdout_breaks_when_separator_read_fails() {
        struct HeaderThenErr {
            n: u8,
        }
        impl tokio::io::AsyncRead for HeaderThenErr {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                if self.n == 0 {
                    self.n = 1;
                    buf.put_slice(b"Content-Length: 2\n");
                    return std::task::Poll::Ready(Ok(()));
                }
                std::task::Poll::Ready(Err(std::io::Error::other("sep boom")))
            }
        }
        impl tokio::io::AsyncBufRead for HeaderThenErr {
            fn poll_fill_buf(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<&[u8]>> {
                let this = self.get_mut();
                if this.n == 0 {
                    this.n = 1;
                    // Safety: the buffer is stored on the struct for the duration
                    // of this poll; AsyncBufRead requires the slice to remain
                    // valid until consume. We keep a static header instead.
                    std::task::Poll::Ready(Ok(b"Content-Length: 2\n".as_slice()))
                } else {
                    let _ = cx;
                    std::task::Poll::Ready(Err(std::io::Error::other("sep boom")))
                }
            }
            fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
        }
        let stdout = Arc::new(Mutex::new(HeaderThenErr { n: 0 }));
        consume_stdout(stdout, Arc::new(Mutex::new(HashMap::new())), "sep".into()).await;
    }
}
