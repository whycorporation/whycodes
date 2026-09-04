use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct CodeModeTool;

impl Default for CodeModeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for CodeModeTool {
    fn name(&self) -> &str {
        "code_mode"
    }

    fn description(&self) -> &str {
        "Transform or refactor code files"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the code file to transform"
                },
                "instruction": {
                    "type": "string",
                    "description": "Natural language description of the transformation to apply"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language of the file (optional)"
                }
            },
            "required": ["path", "instruction"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_str = args["path"].as_str().unwrap_or("");
            let instruction = args["instruction"].as_str().unwrap_or("");
            let language = args["language"].as_str().unwrap_or("");

            let full_path = if std::path::Path::new(path_str).is_absolute() {
                path_str.to_string()
            } else {
                std::path::Path::new(&ctx.working_dir)
                    .join(path_str)
                    .to_string_lossy()
                    .to_string()
            };

            let file_content = match std::fs::read_to_string(&full_path) {
                Ok(content) => content,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error reading file '{}': {}", full_path, e),
                        is_error: true,
                    };
                }
            };

            let lang_hint = if language.is_empty() {
                String::new()
            } else {
                format!("Language: {}\n", language)
            };

            let prompt = format!(
                "// Instruction: {}\n// {}\n\nThe file at {} has been loaded. Apply the following transformation: {}. The current content is:\n\n{}",
                instruction,
                lang_hint.trim_end(),
                path_str,
                instruction,
                file_content
            );

            ToolResult {
                tool_call_id: String::new(),
                content: prompt,
                is_error: false,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    #[test]
    fn code_mode_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn execute_reads_file_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn x() {}\n").unwrap();
        let t = CodeModeTool;
        assert_eq!(t.name(), "code_mode");
        assert!(!t.description().is_empty());
        let params = t.parameters();
        assert_eq!(
            params["required"],
            serde_json::json!(["path", "instruction"])
        );
        let ctx = ToolContext::new(dir.path().to_string_lossy());
        let ok = t
            .execute(
                serde_json::json!({
                    "path": "a.rs",
                    "instruction": "rename x",
                    "language": "rust"
                }),
                &ctx,
            )
            .await;
        assert!(!ok.is_error, "{}", ok.content);
        assert!(ok.content.contains("rename x"), "{}", ok.content);
        assert!(ok.content.contains("Language: rust"), "{}", ok.content);
        assert!(ok.content.contains("fn x()"), "{}", ok.content);

        let abs = dir.path().join("a.rs");
        let via_abs = t
            .execute(
                serde_json::json!({
                    "path": abs.to_string_lossy(),
                    "instruction": "noop"
                }),
                &ctx,
            )
            .await;
        assert!(!via_abs.is_error, "{}", via_abs.content);

        let miss = t
            .execute(
                serde_json::json!({"path": "gone.rs", "instruction": "x"}),
                &ctx,
            )
            .await;
        assert!(miss.is_error, "{}", miss.content);
        assert!(miss.content.contains("Error reading"), "{}", miss.content);
    }
}
