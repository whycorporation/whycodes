use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct GrepTool;

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files using regex. Fast, ripgrep-backed search."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob pattern to filter (e.g., '*.rs', '*.py')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 50)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let pattern = args["pattern"].as_str().unwrap_or("");
        let search_path = args["path"]
            .as_str()
            .unwrap_or(&ctx.working_dir)
            .to_string();
        let file_glob = args["include"].as_str();
        let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;

        // Try ripgrep first, fall back to grep
        let result = Self::try_ripgrep(pattern, &search_path, file_glob, max_results);

        match result {
            Ok(output) => ToolResult {
                tool_call_id: String::new(),
                content: if output.is_empty() {
                    "No matches found.".to_string()
                } else {
                    output
                },
                is_error: false,
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error: {}", e),
                is_error: true,
            },
        }
    }
}

impl GrepTool {
    fn try_ripgrep(
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        max_results: usize,
    ) -> Result<String, String> {
        let mut cmd = if Self::has_command("rg") {
            let mut c = Command::new("rg");
            c.arg("--line-number")
                .arg("--no-heading")
                .arg("--color=never")
                .arg("--max-count=1")
                .arg(format!("--max-count={}", max_results));
            c
        } else {
            let mut c = Command::new("grep");
            c.arg("-rn").arg("-I").arg("--color=never");
            if let Some(glob) = file_glob {
                c.arg(format!("--include={}", glob));
            }
            c
        };

        cmd.arg(pattern).arg(path);

        if let Some(glob) = file_glob
            && Self::has_command("rg")
        {
            cmd.arg("--glob").arg(glob);
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run search command: {}", e))?;

        if !output.status.success() && output.status.code() != Some(1) {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let lines: Vec<&str> = stdout.lines().take(max_results).collect();
        Ok(lines.join("\n"))
    }

    fn has_command(cmd: &str) -> bool {
        Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
