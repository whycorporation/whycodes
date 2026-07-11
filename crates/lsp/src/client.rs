use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
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
            .with_context(|| format!("Failed to spawn LSP server: {command}"))?;

        let stdin = child.stdin.take().context("no stdin on LSP child")?;
        let stdout = child.stdout.take().context("no stdout on LSP child")?;

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
            .context("initialize request failed")?;

        // Send initialized notification
        client
            .notify("initialized", json!({}))
            .await
            .context("initialized notification failed")?;

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
                                                ) {
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
                && !d.is_empty() {
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
            .context("textDocument/diagnostic request failed")?;

        if let Some(result) = resp.result {
            let diagnostics: Vec<Diagnostic> = serde_json::from_value(result)
                .unwrap_or_default();
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
        match resp.result {
            Some(result) if !result.is_null() => {
                Ok(Some(serde_json::from_value(result)?))
            }
            _ => Ok(None),
        }
    }

    /// Go to definition.
    pub async fn definition(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>> {
        let params = TextDocumentPositionParams {
            text_document: crate::types::TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request("textDocument/definition", serde_json::to_value(&params)?)
            .await?;
        match resp.result {
            Some(result) if !result.is_null() => {
                // Can be a single Location or Vec<Location>
                if result.is_array() {
                    Ok(serde_json::from_value(result)?)
                } else {
                    Ok(vec![serde_json::from_value(result)?])
                }
            }
            _ => Ok(vec![]),
        }
    }

    /// Find references.
    pub async fn references(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>> {
        let params = json!({
            "textDocument": { "uri": uri },
            "position": position,
            "context": { "includeDeclaration": false }
        });
        let resp = self.request("textDocument/references", params).await?;
        match resp.result {
            Some(result) if !result.is_null() => Ok(serde_json::from_value(result)?),
            _ => Ok(vec![]),
        }
    }

    // ── Low-level JSON-RPC helpers ───────────────────────────────────────────

    async fn send_message(&self, msg: &serde_json::Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut writer = self.writer.lock().await;
        writer.write_all(header.as_bytes()).await?;
        writer.write_all(body.as_bytes()).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = JsonRpcNotification::new(method, params);
        self.send_message(&serde_json::to_value(&notif)?).await
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse> {
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
                bail!("LSP server closed stdout while waiting for response to {method}");
            }
            let trimmed = header.trim_end_matches('\n').trim_end_matches('\r');
            if let Some(len_str) = line_strip_prefix("Content-Length: ", trimmed) {
                let len: usize = len_str
                    .trim()
                    .parse()
                    .map_err(|_| anyhow!("bad Content-Length: {len_str}"))?;
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
                debug!("Ignoring LSP response with id {:?}, waiting for {id}", resp.id);
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
        "cs" => Some(("omnisharp", vec!("--languageserver"))),
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
