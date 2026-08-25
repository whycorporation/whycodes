use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;

use whycodes_core::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

use crate::client::{self, LspClient};
use crate::types::Position;

/// Tool that delegates to language servers via LSP.
pub struct LspTool {
    /// Lazy-initialized, cached LspClient per file extension.
    clients: Arc<Mutex<HashMap<String, Arc<LspClient>>>>,
}

impl Default for LspTool {
    fn default() -> Self {
        Self::new()
    }
}

impl LspTool {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or start an LSP client for the given extension.
    async fn get_client(&self, ext: &str, workspace_root: &str) -> Result<Arc<LspClient>, String> {
        let mut clients = self.clients.lock().await;
        if let Some(c) = clients.get(ext) {
            return Ok(Arc::clone(c));
        }

        let (server_cmd, args) = client::language_server_for_extension(ext)
            .ok_or_else(|| format!("No language server configured for '.{ext}' files"))?;

        let strings_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let lang_id = client::language_id_for_extension(ext);

        let lsp = LspClient::start(server_cmd, &strings_args, workspace_root, lang_id)
            .await
            .map_err(|e| format!("Failed to start {server_cmd}: {e}"))?;

        let client = Arc::new(lsp);
        clients.insert(ext.to_string(), Arc::clone(&client));
        Ok(client)
    }
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Language Server Protocol tool — diagnostics, hover, go-to-definition, find references. Requires a language server installed for the file type."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["diagnostics", "hover", "definition", "references"],
                    "description": "What LSP action to perform"
                },
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to analyze"
                },
                "line": {
                    "type": "integer",
                    "description": "1-indexed line number (required for hover, definition, references)"
                },
                "character": {
                    "type": "integer",
                    "description": "1-indexed character offset on the line (required for hover, definition, references)"
                }
            },
            "required": ["action", "file_path"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        let file_path = args["file_path"].as_str().unwrap_or("");

        if file_path.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Error: 'file_path' is required".to_string(),
                is_error: true,
            };
        }

        // Determine extension
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error: cannot determine file extension for '{file_path}'"),
                is_error: true,
            };
        }

        let client = match self.get_client(ext, &ctx.working_dir).await {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error: {e}"),
                    is_error: true,
                };
            }
        };

        let uri = format!("file://{file_path}");

        // Ensure document is opened before querying
        if let Err(e) = client.open_document(&uri, None).await {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error opening document: {e}"),
                is_error: true,
            };
        }

        let pos = Position {
            line: args["line"]
                .as_u64()
                .map(|l| l.saturating_sub(1) as u32)
                .unwrap_or(0),
            character: args["character"]
                .as_u64()
                .map(|c| c.saturating_sub(1) as u32)
                .unwrap_or(0),
        };

        match action {
            "diagnostics" => match client.get_diagnostics(&uri).await {
                Ok(diags) => {
                    if diags.is_empty() {
                        ToolResult {
                            tool_call_id: String::new(),
                            content: "No diagnostics found.".to_string(),
                            is_error: false,
                        }
                    } else {
                        let lines: Vec<String> = diags
                            .iter()
                            .map(|d| {
                                format!(
                                    "[L{}:C{}-L{}:C{}] {:?}: {}",
                                    d.range.start.line + 1,
                                    d.range.start.character + 1,
                                    d.range.end.line + 1,
                                    d.range.end.character + 1,
                                    d.severity,
                                    d.message
                                )
                            })
                            .collect();
                        ToolResult {
                            tool_call_id: String::new(),
                            content: lines.join("\n"),
                            is_error: false,
                        }
                    }
                }
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error fetching diagnostics: {e}"),
                    is_error: true,
                },
            },
            "hover" => match client.hover(&uri, pos).await {
                Ok(Some(h)) => ToolResult {
                    tool_call_id: String::new(),
                    content: h.contents_string(),
                    is_error: false,
                },
                Ok(None) => ToolResult {
                    tool_call_id: String::new(),
                    content: "No hover information at this position.".to_string(),
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error getting hover: {e}"),
                    is_error: true,
                },
            },
            "definition" => match client.definition(&uri, pos).await {
                Ok(locations) => {
                    if locations.is_empty() {
                        ToolResult {
                            tool_call_id: String::new(),
                            content: "No definition found.".to_string(),
                            is_error: false,
                        }
                    } else {
                        let lines: Vec<String> = locations
                            .iter()
                            .map(|loc| {
                                format!(
                                    "{}:{}:{}",
                                    loc.uri,
                                    loc.range.start.line + 1,
                                    loc.range.start.character + 1
                                )
                            })
                            .collect();
                        ToolResult {
                            tool_call_id: String::new(),
                            content: lines.join("\n"),
                            is_error: false,
                        }
                    }
                }
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error getting definition: {e}"),
                    is_error: true,
                },
            },
            "references" => match client.references(&uri, pos).await {
                Ok(locations) => {
                    if locations.is_empty() {
                        ToolResult {
                            tool_call_id: String::new(),
                            content: "No references found.".to_string(),
                            is_error: false,
                        }
                    } else {
                        let lines: Vec<String> = locations
                            .iter()
                            .map(|loc| {
                                format!(
                                    "{}:{}:{}",
                                    loc.uri,
                                    loc.range.start.line + 1,
                                    loc.range.start.character + 1
                                )
                            })
                            .collect();
                        ToolResult {
                            tool_call_id: String::new(),
                            content: lines.join("\n"),
                            is_error: false,
                        }
                    }
                }
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error getting references: {e}"),
                    is_error: true,
                },
            },
            _ => ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Unknown action '{}'. Valid: diagnostics, hover, definition, references",
                    action
                ),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycodes_core::tool::ToolContext;

    #[test]
    fn describes_itself() {
        let tool = LspTool::new();
        assert_eq!(tool.name(), "lsp");
        assert!(tool.description().contains("diagnostics"));
        let p = tool.parameters();
        assert_eq!(p["properties"]["action"]["enum"][0], "diagnostics");
        assert_eq!(p["properties"]["action"]["enum"][3], "references");
        assert!(
            p["required"]
                .as_array()
                .unwrap()
                .contains(&json!("file_path"))
        );
    }

    #[tokio::test]
    async fn requires_file_path() {
        let tool = LspTool::new();
        let ctx = ToolContext::new("/tmp");
        let result = tool.execute(json!({ "action": "diagnostics" }), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("file_path"));
    }

    #[tokio::test]
    async fn errors_when_the_extension_cannot_be_determined() {
        let tool = LspTool::new();
        let ctx = ToolContext::new("/tmp");
        let result = tool
            .execute(
                json!({ "action": "diagnostics", "file_path": "/tmp/README" }),
                &ctx,
            )
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("file extension"));
    }

    #[tokio::test]
    async fn reports_missing_language_server_for_unknown_extension() {
        let tool = LspTool::new();
        let ctx = ToolContext::new("/tmp");
        let result = tool
            .execute(
                json!({ "action": "diagnostics", "file_path": "/tmp/x.zzz" }),
                &ctx,
            )
            .await;
        assert!(result.is_error);
        assert!(
            result
                .content
                .contains("No language server configured for '.zzz'")
        );
    }
}
