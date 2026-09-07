use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;

use whycodes_core::tool::{Tool, ToolContext, ToolFuture};
use whycodes_core::types::ToolResult;

use crate::client::LspClient;
use crate::config::{LspSettings, ResolvedServer};
use crate::detect;
use crate::types::Position;

const ACTIONS: &[&str] = &[
    "diagnostics",
    "hover",
    "definition",
    "references",
    "type_definition",
    "implementation",
    "symbols",
];

/// Tool that delegates to language servers via LSP.
pub struct LspTool {
    settings: LspSettings,
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
        Self::with_settings(LspSettings::builtins())
    }

    pub fn with_overlay(overlay: &LspSettings) -> Self {
        Self::with_settings(LspSettings::resolved(overlay))
    }

    pub fn with_settings(settings: LspSettings) -> Self {
        Self {
            settings,
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or start an LSP client for the given extension.
    async fn get_client(&self, ext: &str, workspace_root: &str) -> Result<Arc<LspClient>, String> {
        self.evict_idle().await;
        let mut clients = self.clients.lock().await;
        if let Some(c) = clients.get(ext) {
            c.mark_used();
            return Ok(Arc::clone(c));
        }

        let cwd = Path::new(workspace_root);
        let resolved = self
            .settings
            .resolve(ext, cwd)
            .ok_or_else(|| missing_server_message(&self.settings, ext, cwd))?;

        let client = start_resolved(&resolved, workspace_root).await?;
        clients.insert(ext.to_string(), Arc::clone(&client));
        Ok(client)
    }

    async fn evict_idle(&self) {
        let Some(ms) = self.settings.idle_timeout_ms.filter(|n| *n > 0) else {
            return;
        };
        let limit = Duration::from_millis(ms);
        let mut clients = self.clients.lock().await;
        clients.retain(|_, c| c.idle_for() < limit);
    }

    #[cfg(test)]
    async fn insert_client(&self, ext: &str, client: Arc<LspClient>) {
        self.clients.lock().await.insert(ext.to_string(), client);
    }
}

async fn start_resolved(
    resolved: &ResolvedServer,
    workspace_root: &str,
) -> Result<Arc<LspClient>, String> {
    let cmd = resolved.command.to_string_lossy();
    let lang_id =
        resolved.spec.language_id.as_deref().unwrap_or_else(|| {
            crate::client::language_id_for_extension(primary_ext(&resolved.spec))
        });
    let lsp = LspClient::start_configured(
        cmd.as_ref(),
        &resolved.spec.args,
        workspace_root,
        lang_id,
        resolved.spec.init_options.as_ref(),
        resolved.spec.settings.as_ref(),
    )
    .await
    .map_err(|e| format!("Failed to start {cmd}: {e}"))?;
    Ok(Arc::new(lsp))
}

fn primary_ext(spec: &crate::config::LspServerSpec) -> &str {
    spec.file_types
        .first()
        .map(|s| s.trim_start_matches('.'))
        .unwrap_or("")
}

fn missing_server_message(settings: &LspSettings, ext: &str, cwd: &Path) -> String {
    match settings.spec_for_ext(ext) {
        None => format!("No language server configured for '.{ext}' files"),
        Some((name, spec)) => {
            let cmd = spec.command.as_deref().unwrap_or(name);
            if !detect::root_markers_match(cwd, &spec.root_markers) {
                format!(
                    "Language server '{name}' for '.{ext}' files needs a project marker in {} ({})",
                    cwd.display(),
                    spec.root_markers.join(", ")
                )
            } else {
                format!(
                    "Language server '{name}' for '.{ext}' files is configured but '{cmd}' was not found on PATH"
                )
            }
        }
    }
}

fn format_locations(locations: &[crate::types::Location]) -> String {
    locations
        .iter()
        .map(|loc| {
            format!(
                "{}:{}:{}",
                loc.uri,
                loc.range.start.line + 1,
                loc.range.start.character + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_or_empty(value: serde_json::Value, empty: &str) -> ToolResult {
    if value.is_null() || value.as_array().is_some_and(|a| a.is_empty()) {
        ToolResult {
            tool_call_id: String::new(),
            content: empty.to_string(),
            is_error: false,
        }
    } else {
        ToolResult {
            tool_call_id: String::new(),
            content: value.to_string(),
            is_error: false,
        }
    }
}

impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Language Server Protocol tool — diagnostics, hover, definition, references, type_definition, implementation, document/workspace symbols. Requires a language server installed for the file type."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ACTIONS,
                    "description": "What LSP action to perform"
                },
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to analyze"
                },
                "line": {
                    "type": "integer",
                    "description": "1-indexed line number (required for hover, definition, references, type_definition, implementation)"
                },
                "character": {
                    "type": "integer",
                    "description": "1-indexed character offset on the line"
                },
                "query": {
                    "type": "string",
                    "description": "Workspace symbol query (symbols action without file_path)"
                }
            },
            "required": ["action"]
        })
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let action = args["action"].as_str().unwrap_or("");
            let file_path = args["file_path"].as_str().unwrap_or("");
            let query = args["query"].as_str().unwrap_or("");

            if action == "symbols" && file_path.is_empty() {
                // Workspace-wide search: pick any running client, else rust if present.
                return workspace_symbols(self, ctx, query).await;
            }

            if file_path.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "Error: 'file_path' is required".to_string(),
                    is_error: true,
                };
            }

            let ext = Path::new(file_path)
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

            let uri = detect::file_uri(file_path);

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
                    Ok(locations) if locations.is_empty() => ToolResult {
                        tool_call_id: String::new(),
                        content: "No definition found.".to_string(),
                        is_error: false,
                    },
                    Ok(locations) => ToolResult {
                        tool_call_id: String::new(),
                        content: format_locations(&locations),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error getting definition: {e}"),
                        is_error: true,
                    },
                },
                "references" => match client.references(&uri, pos).await {
                    Ok(locations) if locations.is_empty() => ToolResult {
                        tool_call_id: String::new(),
                        content: "No references found.".to_string(),
                        is_error: false,
                    },
                    Ok(locations) => ToolResult {
                        tool_call_id: String::new(),
                        content: format_locations(&locations),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error getting references: {e}"),
                        is_error: true,
                    },
                },
                "type_definition" => match client.type_definition(&uri, pos).await {
                    Ok(locations) if locations.is_empty() => ToolResult {
                        tool_call_id: String::new(),
                        content: "No type definition found.".to_string(),
                        is_error: false,
                    },
                    Ok(locations) => ToolResult {
                        tool_call_id: String::new(),
                        content: format_locations(&locations),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error getting type definition: {e}"),
                        is_error: true,
                    },
                },
                "implementation" => match client.implementation(&uri, pos).await {
                    Ok(locations) if locations.is_empty() => ToolResult {
                        tool_call_id: String::new(),
                        content: "No implementation found.".to_string(),
                        is_error: false,
                    },
                    Ok(locations) => ToolResult {
                        tool_call_id: String::new(),
                        content: format_locations(&locations),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error getting implementation: {e}"),
                        is_error: true,
                    },
                },
                "symbols" => match client.document_symbols(&uri).await {
                    Ok(value) => json_or_empty(value, "No symbols found."),
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error getting symbols: {e}"),
                        is_error: true,
                    },
                },
                _ => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Unknown action '{}'. Valid: {}", action, ACTIONS.join(", ")),
                    is_error: true,
                },
            }
        })
    }
}

async fn workspace_symbols(tool: &LspTool, ctx: &ToolContext, query: &str) -> ToolResult {
    let cached = {
        let clients = tool.clients.lock().await;
        clients.values().next().cloned()
    };
    let client = if let Some(c) = cached {
        c
    } else {
        match tool.get_client("rs", &ctx.working_dir).await {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error: {e}"),
                    is_error: true,
                };
            }
        }
    };
    match client.workspace_symbols(query).await {
        Ok(value) => json_or_empty(value, "No symbols found."),
        Err(e) => ToolResult {
            tool_call_id: String::new(),
            content: format!("Error getting symbols: {e}"),
            is_error: true,
        },
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
