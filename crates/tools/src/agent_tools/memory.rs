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
                    match svc.search_code(q, limit, 0.12) {
                    Ok(hits) if hits.is_empty() => Ok(
                        "No code hits. Run memory action=index first (or `whycodes memory index`)."
                            .into(),
                    ),
                    Ok(hits) => {
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
                        Ok(out)
                    }
                    Err(e) => Err(e.to_string()),
                }
                }
                "index" => match svc.index_codebase(2000, 8000) {
                    Ok(n) => Ok(format!("Indexed {n} code chunks for this project.")),
                    Err(e) => Err(e.to_string()),
                },
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
                    match svc.remember(&lesson, ctx.session_id.as_deref()) {
                        Ok(id) => Ok(format!(
                            "Lesson stored {}:\n{lesson}",
                            &id[..8.min(id.len())]
                        )),
                        Err(e) => Err(e.to_string()),
                    }
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct IsolatedHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
        prev_no_memory: Option<std::ffi::OsString>,
    }

    impl IsolatedHome {
        fn new() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = tempfile::tempdir().expect("tempdir");
            let prev = std::env::var_os("WHYCODES_HOME");
            let prev_no_memory = std::env::var_os("WHYCODES_NO_MEMORY");
            unsafe {
                std::env::set_var("WHYCODES_HOME", dir.path());
                std::env::remove_var("WHYCODES_NO_MEMORY");
            }
            Self {
                _guard: guard,
                dir,
                prev,
                prev_no_memory,
            }
        }
    }

    impl Drop for IsolatedHome {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("WHYCODES_HOME", v),
                    None => std::env::remove_var("WHYCODES_HOME"),
                }
                match &self.prev_no_memory {
                    Some(v) => std::env::set_var("WHYCODES_NO_MEMORY", v),
                    None => std::env::remove_var("WHYCODES_NO_MEMORY"),
                }
            }
        }
    }

    #[test]
    fn memory_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn execute_write_list_search_delete_and_unknown() {
        let home = IsolatedHome::new();
        let ctx = ToolContext::new(home.dir.path().to_string_lossy());
        let tool = MemoryTool::new();

        let empty = tool
            .execute(serde_json::json!({"action": "list"}), &ctx)
            .await;
        assert!(!empty.is_error, "{}", empty.content);
        assert!(empty.content.contains("No memories"), "{}", empty.content);

        let missing_text = tool
            .execute(serde_json::json!({"action": "write"}), &ctx)
            .await;
        assert!(missing_text.is_error, "{}", missing_text.content);

        let written = tool
            .execute(
                serde_json::json!({"action": "write", "text": "prefer cargo test -p"}),
                &ctx,
            )
            .await;
        assert!(!written.is_error, "{}", written.content);
        assert!(
            written.content.contains("Saved memory"),
            "{}",
            written.content
        );

        let listed = tool
            .execute(serde_json::json!({"action": "list"}), &ctx)
            .await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(
            listed.content.contains("prefer cargo test -p"),
            "{}",
            listed.content
        );

        let found = tool
            .execute(
                serde_json::json!({"action": "search", "text": "cargo test"}),
                &ctx,
            )
            .await;
        assert!(!found.is_error, "{}", found.content);
        assert!(found.content.contains("cargo test"), "{}", found.content);

        let unknown = tool
            .execute(serde_json::json!({"action": "nope"}), &ctx)
            .await;
        assert!(unknown.is_error, "{}", unknown.content);
        assert!(
            unknown.content.contains("unknown action"),
            "{}",
            unknown.content
        );

        let disabled = {
            unsafe { std::env::set_var("WHYCODES_NO_MEMORY", "1") };
            tool.execute(serde_json::json!({"action": "list"}), &ctx)
                .await
        };
        assert!(disabled.is_error, "{}", disabled.content);
        assert!(
            disabled.content.contains("disabled"),
            "{}",
            disabled.content
        );
    }
}
