//! Cross-session memory tool (write / list / search / delete).

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;
use whycode_memory::{MemoryService, MemorySettings};

pub struct MemoryTool;

impl Default for MemoryTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTool {
    pub fn new() -> Self {
        Self
    }
}

fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "whycorporation", "whycode")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn service_for(ctx: &ToolContext) -> Result<MemoryService, String> {
    if std::env::var("WHYCODE_NO_MEMORY")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return Err("memory is disabled (WHYCODE_NO_MEMORY)".into());
    }
    let project = PathBuf::from(&ctx.working_dir);
    MemoryService::open(project, data_dir(), MemorySettings::default())
        .map_err(|e| format!("open memory store: {e}"))
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Save or recall durable project facts across sessions (preferences, build commands, decisions). \
         Prefer this over re-asking the user. Do not store secrets."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["write", "list", "search", "delete"],
                    "description": "write: store a fact; list: recent facts; search: semantic search; delete: remove by id"
                },
                "text": {
                    "type": "string",
                    "description": "Fact text (write) or search query (search)"
                },
                "id": {
                    "type": "string",
                    "description": "Memory id or unique prefix (delete)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results for list/search (default 10)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(100) as usize;

        let svc = match service_for(ctx) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: e,
                    is_error: true,
                };
            }
        };

        let result = match action.as_str() {
            "write" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if text.is_empty() {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: "write requires non-empty `text`".into(),
                        is_error: true,
                    };
                }
                match svc.remember(text, ctx.session_id.as_deref()) {
                    Ok(id) => Ok(format!(
                        "Saved memory {}:\n{}",
                        &id[..8.min(id.len())],
                        text
                    )),
                    Err(e) => Err(e.to_string()),
                }
            }
            "list" => match svc.list(limit) {
                Ok(rows) if rows.is_empty() => Ok("No memories for this project.".into()),
                Ok(rows) => {
                    let mut out = format!("{} memories:\n", rows.len());
                    for r in rows {
                        out.push_str(&format!(
                            "- [{}] {}\n",
                            &r.id[..8.min(r.id.len())],
                            r.text
                        ));
                    }
                    Ok(out)
                }
                Err(e) => Err(e.to_string()),
            },
            "search" => {
                let q = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if q.is_empty() {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: "search requires `text` query".into(),
                        is_error: true,
                    };
                }
                match svc.search(q, limit, 0.15) {
                    Ok(hits) if hits.is_empty() => Ok("No matching memories.".into()),
                    Ok(hits) => {
                        let mut out = format!("{} hits:\n", hits.len());
                        for h in hits {
                            out.push_str(&format!(
                                "- [{:.2}] [{}] {}\n",
                                h.score,
                                &h.entry.id[..8.min(h.entry.id.len())],
                                h.entry.text
                            ));
                        }
                        Ok(out)
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            "delete" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
                if id.is_empty() {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: "delete requires `id`".into(),
                        is_error: true,
                    };
                }
                match svc.delete(id) {
                    Ok(true) => Ok(format!("Deleted memory {id}")),
                    Ok(false) => Ok(format!("No memory matching '{id}'")),
                    Err(e) => Err(e.to_string()),
                }
            }
            _ => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("unknown action '{action}'; use write|list|search|delete"),
                    is_error: true,
                };
            }
        };

        match result {
            Ok(content) => ToolResult {
                tool_call_id: String::new(),
                content,
                is_error: false,
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: e,
                is_error: true,
            },
        }
    }
}
