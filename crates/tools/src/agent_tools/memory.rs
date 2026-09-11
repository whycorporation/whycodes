//! Cross-session memory tool (write / list / search / delete).

use serde_json::json;
use std::path::PathBuf;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;
use whycodes_memory::{MemoryService, MemorySettings};

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
    whycodes_core::paths::data_dir()
}

fn service_for(ctx: &ToolContext) -> Result<MemoryService, String> {
    if std::env::var("WHYCODES_NO_MEMORY")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return Err("memory is disabled (WHYCODES_NO_MEMORY)".into());
    }
    let project = PathBuf::from(&ctx.working_dir);
    MemoryService::open(project, data_dir(), MemorySettings::default())
        .map_err(|e| format!("open memory store: {e}"))
}
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
                    "enum": ["write", "list", "search", "delete", "code_search", "index", "learn"],
                    "description": "write/list/search/delete facts; learn a reusable lesson; code_search over indexed code; index the codebase for RAG"
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
                Err(e) => return memory_err(e),
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
                    map_svc(
                        svc.remember(text, ctx.session_id.as_deref())
                            .map_err(|e| e.to_string()),
                        |id| format!("Saved memory {}:\n{text}", &id[..8.min(id.len())]),
                    )
                }
                "list" => map_svc(
                    svc.list(limit).map_err(|e| e.to_string()),
                    format_memory_list,
                ),
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
                    map_svc(
                        svc.search(q, limit, 0.15).map_err(|e| e.to_string()),
                        format_memory_hits,
                    )
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
                    map_svc(svc.delete(id).map_err(|e| e.to_string()), |ok| {
                        format_delete(id, ok)
                    })
                }
                "code_search" => {
                    let q = args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if q.is_empty() {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: "code_search requires `text` query".into(),
                            is_error: true,
                        };
                    }
                    map_svc(
                        svc.search_code(q, limit, 0.12).map_err(|e| e.to_string()),
                        format_code_hits,
                    )
                }
                "index" => map_svc(
                    svc.index_codebase(2000, 8000).map_err(|e| e.to_string()),
                    |n| format!("Indexed {n} code chunks for this project."),
                ),
                "learn" => {
                    let text = args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if text.is_empty() {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: "learn requires non-empty `text` (the reusable lesson)".into(),
                            is_error: true,
                        };
                    }
                    let lesson = format!("Lesson: {text}");
                    map_svc(
                        svc.remember(&lesson, ctx.session_id.as_deref())
                            .map_err(|e| e.to_string()),
                        |id| format!("Lesson stored {}:\n{lesson}", &id[..8.min(id.len())]),
                    )
                }
                _ => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!(
                            "unknown action '{action}'; use write|list|search|delete|code_search|index|learn"
                        ),
                        is_error: true,
                    };
                }
            };

            memory_result(result)
        })
    }
}

fn map_svc<T>(result: Result<T, String>, ok: impl FnOnce(T) -> String) -> Result<String, String> {
    match result {
        Ok(v) => Ok(ok(v)),
        Err(e) => Err(memory_svc_err(&e)),
    }
}

fn format_memory_list(rows: Vec<whycodes_memory::MemoryRow>) -> String {
    if rows.is_empty() {
        return "No memories for this project.".into();
    }
    let mut out = format!("{} memories:\n", rows.len());
    for r in rows {
        out.push_str(&format!("- [{}] {}\n", &r.id[..8.min(r.id.len())], r.text));
    }
    out
}

fn format_memory_hits(hits: Vec<whycodes_memory::RecallHit>) -> String {
    if hits.is_empty() {
        return "No matching memories.".into();
    }
    let mut out = format!("{} hits:\n", hits.len());
    for h in hits {
        out.push_str(&format!(
            "- [{:.2}] [{}] {}\n",
            h.score,
            &h.entry.id[..8.min(h.entry.id.len())],
            h.entry.text
        ));
    }
    out
}

fn format_delete(id: &str, ok: bool) -> String {
    if ok {
        format!("Deleted memory {id}")
    } else {
        format!("No memory matching '{id}'")
    }
}

fn format_code_hits(hits: Vec<whycodes_memory::CodeHit>) -> String {
    if hits.is_empty() {
        return "No code hits. Run memory action=index first (or `whycodes memory index`).".into();
    }
    let mut out = format!("{} code hits:\n", hits.len());
    for h in hits {
        out.push_str(&format!(
            "- [{:.2}] {}:{}-{}\n{}\n",
            h.score,
            h.entry.path,
            h.entry.start_line,
            h.entry.end_line,
            h.entry.text.lines().take(6).collect::<Vec<_>>().join("\n")
        ));
    }
    out
}

fn memory_result(result: Result<String, String>) -> ToolResult {
    match result {
        Ok(content) => memory_ok(content),
        Err(e) => memory_err(e),
    }
}

fn memory_ok(content: String) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content,
        is_error: false,
    }
}

fn memory_err(e: String) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: e,
        is_error: true,
    }
}

fn memory_svc_err(e: &str) -> String {
    e.to_string()
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
