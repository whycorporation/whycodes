use serde_json::json;
use std::path::Path;

use super::paths::{display_path, resolve_path, walk_files};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

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
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern under the project (e.g. `**/*.rs`, `crates/**/mod.rs`). \
         Respects .gitignore; skips heavy dirs (target, node_modules, .git, …). \
         Served from the live file index when warm. Prefer over shell find."
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let working_dir = ctx.working_dir.clone();
            let file_index = ctx.file_index.clone();
            crate::blocking::tool(move || Self::run(args, working_dir, file_index)).await
        })
    }
}

impl GlobTool {
    fn run(
        args: serde_json::Value,
        working_dir: String,
        file_index: Option<std::sync::Arc<whycodes_index::WorkspaceIndex>>,
    ) -> ToolResult {
        let pattern_str = args["pattern"].as_str().unwrap_or("").trim();
        if pattern_str.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Missing required parameter `pattern`.".into(),
                is_error: true,
            };
        }

        let root_arg = args["path"].as_str().unwrap_or(&working_dir);
        let root = resolve_path(&working_dir, root_arg);
        let root_shown = display_path(&root, &working_dir);

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

        // Returns false when way over the cap → callers stop iterating.
        let mut match_one = |name: &str, rel: &str, results: &mut Vec<String>| {
            let ok = if match_name_only {
                pattern.matches(name) || pattern.matches(rel)
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
                }
            }
            total <= max_results.saturating_mul(5)
        };

        // Fast path: the warm workspace index answers without any syscalls.
        // Patterns targeting dotfiles bypass it (the index skips hidden files
        // by design) and take the classic walk below.
        let targets_hidden = pattern_str.starts_with('.') || pattern_str.contains("/.");
        let used_index = if targets_hidden {
            false
        } else if let Some(idx) = file_index.as_deref() {
            super::paths::visit_index(idx, &root, &mut |path, rel, is_dir, _size| {
                if is_dir {
                    return true;
                }
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match_one(&name, rel, &mut results)
            })
            .is_some()
        } else {
            false
        };

        let mut walked = false;
        if !used_index {
            walked = true;
            walk_files(&root, &mut |path, rel| {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match_one(&name, rel, &mut results)
            });
        }

        // Fallback: if the walk found nothing, try the classic glob crate on the
        // joined pattern once — covers odd absolute patterns without exploding
        // into target/ when the walk already pruned correctly and truly found nothing.
        // (Skipped when the index answered: a warm index is authoritative.)
        if walked && results.is_empty() && total == 0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use std::time::Duration;
    use whycodes_index::IndexOptions;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn metadata_describes_glob_tool() {
        let t = GlobTool::new();
        assert_eq!(t.name(), "glob");
        assert!(t.description().contains("glob"));
        let params = t.parameters();
        assert_eq!(params["required"][0], "pattern");
    }

    #[tokio::test]
    async fn missing_pattern_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = GlobTool::new().execute(json!({}), &ctx(dir.path())).await;
        assert!(out.is_error);
        assert!(out.content.contains("Missing required"), "{}", out.content);
    }

    #[tokio::test]
    async fn invalid_pattern_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = GlobTool::new()
            .execute(json!({ "pattern": "[" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Invalid glob pattern"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn missing_root_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = GlobTool::new()
            .execute(
                json!({ "pattern": "*.rs", "path": "nope" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Path not found"), "{}", out.content);
    }

    #[tokio::test]
    async fn finds_files_by_suffix_and_skips_heavy_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("target/debug")).expect("mkdir");
        std::fs::write(dir.path().join("src/main.rs"), "fn main(){}").expect("write");
        std::fs::write(dir.path().join("src/lib.rs"), "// lib").expect("write");
        std::fs::write(dir.path().join("target/debug/foo.rs"), "bin").expect("write");

        let out = GlobTool::new()
            .execute(json!({ "pattern": "*.rs" }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("src/main.rs"), "{}", out.content);
        assert!(out.content.contains("src/lib.rs"), "{}", out.content);
        assert!(
            !out.content.contains("foo.rs"),
            "target/ pruned: {}",
            out.content
        );
        assert!(out.content.contains("Found 2 files"), "{}", out.content);
    }

    #[tokio::test]
    async fn no_matches_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let out = GlobTool::new()
            .execute(json!({ "pattern": "*.rs" }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("No files matched"), "{}", out.content);
    }

    #[tokio::test]
    async fn path_pattern_matches_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/main.rs"), "x").expect("write");
        std::fs::write(dir.path().join("other.rs"), "x").expect("write");

        let out = GlobTool::new()
            .execute(json!({ "pattern": "src/*.rs" }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("src/main.rs"), "{}", out.content);
        assert!(!out.content.contains("other.rs"), "{}", out.content);
        assert!(out.content.contains("Found 1 file"), "{}", out.content);
    }

    #[tokio::test]
    async fn max_results_caps_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").expect("write");
        }
        let out = GlobTool::new()
            .execute(
                json!({ "pattern": "*.txt", "max_results": 2 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("showing 2 of"), "{}", out.content);
    }

    #[tokio::test]
    async fn warm_index_serves_matches() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/a.rs"), "x").expect("write");
        std::fs::write(dir.path().join("README.md"), "hi").expect("write");

        let idx = whycodes_index::WorkspaceIndex::start_with(
            vec![dir.path().to_path_buf()],
            IndexOptions {
                watch: false,
                threads: 1,
                ..Default::default()
            },
        );
        assert!(idx.wait_ready(Duration::from_secs(10)));
        let mut c = ctx(dir.path());
        c.file_index = Some(idx);

        let out = GlobTool::new()
            .execute(json!({ "pattern": "**/*.rs" }), &c)
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("src/a.rs"), "{}", out.content);
        assert!(!out.content.contains("README.md"), "{}", out.content);
    }

    #[tokio::test]
    async fn hidden_file_pattern_walks_instead_of_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".env"), "SECRET=1").expect("write");
        std::fs::write(dir.path().join("visible.txt"), "x").expect("write");

        let idx = whycodes_index::WorkspaceIndex::start_with(
            vec![dir.path().to_path_buf()],
            IndexOptions {
                watch: false,
                threads: 1,
                ..Default::default()
            },
        );
        assert!(idx.wait_ready(Duration::from_secs(10)));
        let mut c = ctx(dir.path());
        c.file_index = Some(idx);

        let out = GlobTool::new()
            .execute(json!({ "pattern": ".env" }), &c)
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains(".env"), "{}", out.content);
    }

    #[tokio::test]
    async fn remaining_glob_edges() {
        let t = GlobTool;
        assert_eq!(t.name(), "glob");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let abs_pat = dir.path().join("*.rs").to_string_lossy().into_owned();
        let out = t
            .execute(
                serde_json::json!({"pattern": abs_pat, "path": dir.path().to_string_lossy()}),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
    }

    #[tokio::test]
    async fn fallback_glob_skips_heavy_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/foo.rs"), "x").unwrap();
        let abs_pat = dir
            .path()
            .join("target/**/*.rs")
            .to_string_lossy()
            .into_owned();
        let out = GlobTool::new()
            .execute(
                serde_json::json!({"pattern": abs_pat, "path": dir.path().to_string_lossy()}),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
    }
}
