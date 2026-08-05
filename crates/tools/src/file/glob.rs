use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use super::paths::{display_path, resolve_path, walk_files};
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

const DEFAULT_MAX: usize = 200;
const HARD_MAX: usize = 2000;

pub struct GlobTool;

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern under the project (e.g. `**/*.rs`, `crates/**/mod.rs`). \
         Skips heavy dirs (target, node_modules, .git, …) for speed. Prefer over shell find."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. '*.rs', 'src/**/*.ts', '**/Cargo.toml')"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory for the search (default: project root)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum paths to return (default: 200)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let pattern_str = args["pattern"].as_str().unwrap_or("").trim();
        if pattern_str.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Missing required parameter `pattern`.".into(),
                is_error: true,
            };
        }

        let root_arg = args["path"].as_str().unwrap_or(&ctx.working_dir);
        let root = resolve_path(&ctx.working_dir, root_arg);
        let root_shown = display_path(&root, &ctx.working_dir);

        if !root.exists() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Path not found: {}", root_shown),
                is_error: true,
            };
        }

        let max_results = args["max_results"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX)
            .clamp(1, HARD_MAX);

        // Normalize pattern: if it has no `**`/`/` and is a simple suffix like `*.rs`,
        // match against file name; otherwise match relative path.
        let pattern = match glob::Pattern::new(pattern_str) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Invalid glob pattern: {}", e),
                    is_error: true,
                };
            }
        };

        // Also try matching against basename for patterns like `*.rs`
        let match_name_only = !pattern_str.contains('/') && !pattern_str.contains("**");

        let mut results: Vec<String> = Vec::new();
        let mut total = 0usize;
        let mut hit_cap = false;

        walk_files(&root, &mut |path, rel| {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();

            let ok = if match_name_only {
                pattern.matches(&name) || pattern.matches(rel)
            } else {
                pattern.matches(rel)
                    // Also allow patterns that include leading ** implicitly
                    || pattern.matches(&format!("./{rel}"))
            };

            if ok {
                total += 1;
                if results.len() < max_results {
                    results.push(rel.to_string());
                } else {
                    hit_cap = true;
                    // Keep counting a bit for accurate totals, but stop walk if way over
                    if total > max_results.saturating_mul(5) {
                        return false;
                    }
                }
            }
            true
        });

        // Fallback: if the walk found nothing, try the classic glob crate on the
        // joined pattern once — covers odd absolute patterns without exploding
        // into target/ when the walk already pruned correctly and truly found nothing.
        if results.is_empty() && total == 0 {
            let joined = if Path::new(pattern_str).is_absolute() {
                pattern_str.to_string()
            } else {
                format!(
                    "{}{}{}",
                    root.display(),
                    std::path::MAIN_SEPARATOR,
                    pattern_str
                )
            };
            if let Ok(paths) = glob::glob(&joined) {
                for entry in paths.flatten() {
                    // Skip anything under pruned directory names
                    if entry.components().any(|c| {
                        let s = c.as_os_str().to_string_lossy();
                        super::paths::is_skip_dir(&s)
                    }) {
                        continue;
                    }
                    total += 1;
                    if results.len() < max_results {
                        let rel = entry
                            .strip_prefix(&root)
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_else(|_| entry.display().to_string());
                        results.push(rel);
                    } else {
                        hit_cap = true;
                        break;
                    }
                }
            }
        }

        results.sort();

        if results.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "No files matched `{}` under {} (heavy dirs skipped).",
                    pattern_str, root_shown
                ),
                is_error: false,
            };
        }

        let mut output = format!(
            "Found {} file{} under {} matching `{}`:\n{}",
            total,
            if total == 1 { "" } else { "s" },
            root_shown,
            pattern_str,
            results.join("\n")
        );
        if hit_cap || total > results.len() {
            output.push_str(&format!(
                "\n… showing {} of {} (raise max_results for more)",
                results.len(),
                total
            ));
        }

        ToolResult {
            tool_call_id: String::new(),
            content: output,
            is_error: false,
        }
    }
}
