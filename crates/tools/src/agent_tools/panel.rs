//! Pin a file, unified diff, or mermaid diagram on the TUI side panel.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::panel::PanelUpdate;
use whycodes_core::types::ToolResult;

const MAX_PREVIEW_BYTES: usize = 64 * 1024;

/// Show a file / diff / mermaid on the host side panel.
pub struct PanelTool;

impl Default for PanelTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for PanelTool {
    fn name(&self) -> &str {
        "panel"
    }

    fn description(&self) -> &str {
        "Pin a file, unified diff, or mermaid diagram on the user's side panel \
         (Preview tab). Use to keep a reference visible while you keep working. \
         Does not edit files."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["show_file", "show_diff", "show_mermaid", "clear"],
                    "description": "What to put on the panel"
                },
                "path": {
                    "type": "string",
                    "description": "File path (show_file / show_diff). Relative to the project."
                },
                "source": {
                    "type": "string",
                    "description": "Inline mermaid source, or a unified diff when path is omitted"
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
                .trim();
            let path = args.get("path").and_then(|v| v.as_str());
            let source = args.get("source").and_then(|v| v.as_str());

            let update = match action {
                "clear" => PanelUpdate::Clear,
                "show_file" => {
                    let Some(p) = path.filter(|s| !s.is_empty()) else {
                        return err("show_file requires `path`");
                    };
                    let abs = resolve(ctx, p);
                    match read_capped(&abs) {
                        Ok(text) => PanelUpdate::File {
                            path: p.to_string(),
                            text,
                        },
                        Err(e) => return err(&e),
                    }
                }
                "show_diff" => {
                    let label = path.unwrap_or("diff").to_string();
                    let unified = if let Some(src) = source.filter(|s| !s.is_empty()) {
                        cap_text(src)
                    } else if let Some(p) = path.filter(|s| !s.is_empty()) {
                        match git_diff(ctx, p) {
                            Ok(s) if !s.trim().is_empty() => s,
                            Ok(_) => return err("git diff is empty for that path"),
                            Err(e) => return err(&e),
                        }
                    } else {
                        return err("show_diff requires `path` or `source`");
                    };
                    PanelUpdate::Diff {
                        path: label,
                        unified,
                    }
                }
                "show_mermaid" => {
                    let src = if let Some(s) = source.filter(|s| !s.is_empty()) {
                        cap_text(s)
                    } else if let Some(p) = path.filter(|s| !s.is_empty()) {
                        match read_capped(&resolve(ctx, p)) {
                            Ok(t) => t,
                            Err(e) => return err(&e),
                        }
                    } else {
                        return err("show_mermaid requires `source` or `path`");
                    };
                    PanelUpdate::Mermaid { source: src }
                }
                _ => return err("action must be show_file, show_diff, show_mermaid, or clear"),
            };

            if let Some(sink) = ctx.panel.as_ref() {
                sink(update.clone());
            }

            let msg = match &update {
                PanelUpdate::Clear => "Panel cleared.".to_string(),
                PanelUpdate::File { path, .. } => {
                    format!("Pinned file `{path}` on the side panel.")
                }
                PanelUpdate::Diff { path, .. } => {
                    format!("Pinned diff `{path}` on the side panel.")
                }
                PanelUpdate::Mermaid { .. } => {
                    "Pinned mermaid diagram on the side panel.".to_string()
                }
            };
            if ctx.panel.is_none() {
                return ok(format!("{msg} (no TUI panel attached — preview skipped)"));
            }
            ok(msg)
        })
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: true,
    }
}

fn ok(msg: String) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg,
        is_error: false,
    }
}

fn resolve(ctx: &ToolContext, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(&ctx.working_dir).join(p)
    }
}

fn read_capped(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() > MAX_PREVIEW_BYTES {
        return Err(format!(
            "file is {} bytes (limit {MAX_PREVIEW_BYTES}); pick a smaller file",
            bytes.len()
        ));
    }
    String::from_utf8(bytes).map_err(|_| "file is not valid UTF-8".to_string())
}

fn cap_text(s: &str) -> String {
    if s.len() <= MAX_PREVIEW_BYTES {
        s.to_string()
    } else {
        let mut t = s[..MAX_PREVIEW_BYTES].to_string();
        t.push_str("\n…");
        t
    }
}

fn git_diff(ctx: &ToolContext, path: &str) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["diff", "--", path])
        .current_dir(&ctx.working_dir)
        .output()
        .map_err(|e| format!("git diff: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git diff failed: {}", stderr.trim()));
    }
    Ok(cap_text(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn show_file_pushes_update() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "hello panel").unwrap();
        let seen = Arc::new(Mutex::new(None));
        let seen2 = Arc::clone(&seen);
        let mut ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().to_string());
        ctx.panel = Some(Arc::new(move |u| {
            *seen2.lock().unwrap() = Some(u);
        }));
        let tool = PanelTool::new();
        let result = tool
            .execute(json!({"action": "show_file", "path": "note.txt"}), &ctx)
            .await;
        assert!(!result.is_error, "{}", result.content);
        match seen.lock().unwrap().clone() {
            Some(PanelUpdate::File { path, text }) => {
                assert_eq!(path, "note.txt");
                assert_eq!(text, "hello panel");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn clear_without_sink_is_ok() {
        let ctx = ToolContext::unsandboxed(".");
        let result = PanelTool::new()
            .execute(json!({"action": "clear"}), &ctx)
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("no TUI panel"));
    }
}
