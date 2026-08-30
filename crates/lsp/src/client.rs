use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{LspError, Result};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
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
        client
            .notify("initialized", json!({}))
            .await
            .map_err(|e| LspError::msg(format!("initialized notification failed: {e}")))?;

        info!("LSP server {command} initialized for {}", workspace_root);

        // Spawn a background reader loop to collect diagnostics (and swallow
        // window/showMessage etc.).
        let stdout_bg = client.stdout.clone();
        let diags_bg = client.diagnostics.clone();
        let cmd_name = command.to_string();
        tokio::spawn(async move {
            let mut lock = stdout_bg.lock().await;
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
                                use tokio::io::AsyncReadExt;
                                if lock.get_mut().read_exact(&mut body).await.is_err() {
                                    break;
                                }
                                let body_str = String::from_utf8_lossy(&body).to_string();
                                match IncomingMessage::from_line(&body_str) {
                                    Ok(IncomingMessage::Notification(notif)) => {
                                        if notif.method == "textDocument/publishDiagnostics"
                                            && let Some(params) = notif.params
                                            && let Ok(parsed) = serde_json::from_value::<
                                                crate::types::PublishDiagnosticsParams,
                                            >(
                                                params
                                            )
                                        {
                                            diags_bg
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
        });

        Ok(client)
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
                use tokio::io::AsyncReadExt;
                reader.get_mut().read_exact(&mut body).await?;
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

    #[test]
    fn command_available_known_and_missing() {
        assert!(command_available("sh") || command_available("true"));
        assert!(!command_available("whycodes-definitely-not-on-path-xyz"));
    }
}
