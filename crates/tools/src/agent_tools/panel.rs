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

    #[tokio::test]
    async fn remaining_actions_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
        let t = PanelTool;
        assert_eq!(t.name(), "panel");
        assert!(!t.description().is_empty());
        assert_eq!(t.parameters()["required"][0], "action");
        let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().to_string());

        let missing_file = t.execute(json!({"action": "show_file"}), &ctx).await;
        assert!(missing_file.is_error);
        let gone = t
            .execute(json!({"action": "show_file", "path": "gone.txt"}), &ctx)
            .await;
        assert!(gone.is_error);

        let abs = dir.path().join("note.txt");
        let via_abs = t
            .execute(
                json!({"action": "show_file", "path": abs.to_string_lossy()}),
                &ctx,
            )
            .await;
        assert!(!via_abs.is_error, "{}", via_abs.content);

        let mermaid = t
            .execute(
                json!({"action": "show_mermaid", "source": "graph TD; A-->B;"}),
                &ctx,
            )
            .await;
        assert!(!mermaid.is_error, "{}", mermaid.content);
        let mermaid_file = t
            .execute(json!({"action": "show_mermaid", "path": "note.txt"}), &ctx)
            .await;
        assert!(!mermaid_file.is_error, "{}", mermaid_file.content);
        let mermaid_missing = t.execute(json!({"action": "show_mermaid"}), &ctx).await;
        assert!(mermaid_missing.is_error);

        let diff_src = t
            .execute(
                json!({"action": "show_diff", "source": "--- a\n+++ b\n"}),
                &ctx,
            )
            .await;
        assert!(!diff_src.is_error, "{}", diff_src.content);
        let diff_empty = t
            .execute(json!({"action": "show_diff", "path": "note.txt"}), &ctx)
            .await;
        assert!(diff_empty.is_error, "{}", diff_empty.content);
        let diff_missing = t.execute(json!({"action": "show_diff"}), &ctx).await;
        assert!(diff_missing.is_error);

        let huge = "x".repeat(MAX_PREVIEW_BYTES + 8);
        let capped = cap_text(&huge);
        assert!(capped.ends_with("\n…"));
        std::fs::write(dir.path().join("big.txt"), &huge).unwrap();
        let too_big = t
            .execute(json!({"action": "show_file", "path": "big.txt"}), &ctx)
            .await;
        assert!(too_big.is_error);

        let bad = t.execute(json!({"action": "nope"}), &ctx).await;
        assert!(bad.is_error);

        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen2 = std::sync::Arc::clone(&seen);
        let mut with_sink = ctx.clone();
        with_sink.panel = Some(std::sync::Arc::new(move |u| {
            *seen2.lock().unwrap() = Some(u);
        }));
        let cleared = t.execute(json!({"action": "clear"}), &with_sink).await;
        assert!(!cleared.is_error, "{}", cleared.content);
        assert!(!cleared.content.contains("no TUI panel"));
    }

    #[tokio::test]
    async fn git_diff_success_and_mermaid_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@whycodes.local"])
            .current_dir(dir.path())
            .status();
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "whycodes-test"])
            .current_dir(dir.path())
            .status();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let t = PanelTool::new();
        let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().to_string());
        let diff = t
            .execute(json!({"action": "show_diff", "path": "a.txt"}), &ctx)
            .await;
        assert!(!diff.is_error, "{}", diff.content);

        let missing = t
            .execute(json!({"action": "show_mermaid", "path": "gone.mmd"}), &ctx)
            .await;
        assert!(missing.is_error, "{}", missing.content);

        let git_fail = git_diff(&ctx, "nope.txt");
        assert!(git_fail.is_ok() || git_fail.unwrap_err().contains("git"));
    }
}
