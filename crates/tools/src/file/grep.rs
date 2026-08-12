use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::paths::{
    BINARY_SNIFF_LEN, MAX_GREP_FILE_BYTES, display_path, file_len, is_binary_bytes, resolve_path,
    walk_files,
};
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

const DEFAULT_MAX_RESULTS: usize = 50;
const HARD_MAX_RESULTS: usize = 500;

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
        "Search file contents with regex (in-process, no ripgrep required). \
         Respects .gitignore; skips binaries and heavy dirs (target, node_modules, .git, …). \
         Prefer over shell grep for project code search."
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
                    "description": "Directory or file to search in (default: project root)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob filter (e.g. '*.rs', '*.{ts,tsx}')"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive match (default: false)"
                },
                "context": {
                    "type": "integer",
                    "description": "Lines of context before/after each match (default: 0, max: 5)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines (default: 50)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let pattern = args["pattern"].as_str().unwrap_or("").to_string();
        if pattern.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Missing required parameter `pattern`.".into(),
                is_error: true,
            };
        }

        let working_dir = ctx.working_dir.clone();
        let search_path = args["path"]
            .as_str()
            .map(|s| resolve_path(&working_dir, s))
            .unwrap_or_else(|| Path::new(&working_dir).to_path_buf());
        let file_glob = args["include"].as_str().map(|s| s.to_string());
        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let context = args["context"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(0)
            .min(5);
        let max_results = args["max_results"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, HARD_MAX_RESULTS);

        let file_index = ctx.file_index.clone();
        // FS walk + regex on a blocking pool so parallel tool batches do not
        // pin Tokio workers (stream drain / permission UI stay responsive).
        let result = tokio::task::spawn_blocking(move || {
            Self::search(
                &pattern,
                &search_path,
                file_glob.as_deref(),
                case_insensitive,
                context,
                max_results,
                &working_dir,
                file_index.as_deref(),
            )
        })
        .await;

        match result {
            Ok(Ok(output)) => ToolResult {
                tool_call_id: String::new(),
                content: if output.is_empty() {
                    "No matches found.".to_string()
                } else {
                    output
                },
                is_error: false,
            },
            Ok(Err(e)) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error: {}", e),
                is_error: true,
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error: grep task failed: {e}"),
                is_error: true,
            },
        }
    }
}

impl GrepTool {
    #[allow(clippy::too_many_arguments)]
    fn search(
        pattern: &str,
        path: &Path,
        file_glob: Option<&str>,
        case_insensitive: bool,
        context: usize,
        max_results: usize,
        working_dir: &str,
        file_index: Option<&whycode_index::WorkspaceIndex>,
    ) -> Result<String, String> {
        let mut builder = regex::RegexBuilder::new(pattern);
        builder.case_insensitive(case_insensitive);
        // Avoid catastrophic backtracking hanging the agent
        builder.size_limit(1 << 20);
        builder.dfa_size_limit(1 << 20);
        let re = builder
            .build()
            .map_err(|e| format!("invalid regex: {}", e))?;

        let glob = match file_glob {
            Some(g) => Some(glob::Pattern::new(g).map_err(|e| format!("invalid glob: {}", e))?),
            None => None,
        };

        if !path.exists() {
            return Err(format!(
                "path not found: {}",
                display_path(path, working_dir)
            ));
        }

        let mut matches: Vec<String> = Vec::new();
        let mut files_searched = 0usize;
        let mut truncated = false;

        if path.is_file() {
            files_searched = 1;
            Self::search_file(
                path,
                &display_path(path, working_dir),
                &re,
                context,
                &mut matches,
                max_results,
            );
        } else {
            // Fast path: enumerate from the warm workspace index (no walk).
            // Dotfile-targeting includes bypass it (index skips hidden files).
            let targets_hidden = file_glob.is_some_and(|g| g.starts_with('.') || g.contains("/."));
            let indexed = if targets_hidden {
                None
            } else {
                file_index.and_then(|idx| super::paths::index_entries(idx, path))
            };

            let mut visit_one = |file: &Path, rel: &str| -> bool {
                if matches.len() >= max_results {
                    truncated = true;
                    return false;
                }
                if let Some(g) = glob.as_ref() {
                    let name = file
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    // Match basename or full relative path
                    if !g.matches(&name) && !g.matches(rel) {
                        return true;
                    }
                }
                files_searched += 1;
                Self::search_file(file, rel, &re, context, &mut matches, max_results);
                matches.len() < max_results
            };

            if let Some(entries) = indexed {
                for (file, rel, is_dir, _size) in entries {
                    if is_dir {
                        continue;
                    }
                    if !visit_one(&file, &rel) {
                        break;
                    }
                }
            } else {
                walk_files(path, &mut |file, rel| visit_one(file, rel));
            }
            if matches.len() >= max_results {
                truncated = true;
            }
        }

        if matches.is_empty() {
            return Ok(String::new());
        }

        let mut out = matches.join("\n");
        out.push_str(&format!(
            "\n\n({} match{} in {} file{}; pattern `{}`)",
            matches.len(),
            if matches.len() == 1 { "" } else { "es" },
            files_searched,
            if files_searched == 1 { "" } else { "s" },
            pattern
        ));
        if truncated {
            out.push_str(&format!(
                "\n[truncated at {} matches — narrow path/include or raise max_results]",
                max_results
            ));
        }
        Ok(out)
    }

    /// Append matching lines as `path:line:content` (optionally with context).
    fn search_file(
        file: &Path,
        display: &str,
        re: &regex::Regex,
        context: usize,
        matches: &mut Vec<String>,
        max_results: usize,
    ) {
        if matches.len() >= max_results {
            return;
        }

        // Size gate
        if file_len(file).is_some_and(|n| n > MAX_GREP_FILE_BYTES) {
            return;
        }

        let Ok(f) = fs::File::open(file) else {
            return;
        };
        let mut reader = BufReader::with_capacity(64 * 1024, f);

        // Sniff binary from the first chunk without loading the whole file.
        let mut head = [0u8; BINARY_SNIFF_LEN];
        use std::io::Read;
        let Ok(n) = reader.read(&mut head) else {
            return;
        };
        if is_binary_bytes(&head[..n]) {
            return;
        }

        // Re-open for line iteration (simpler than mix of read+seek on all platforms)
        let Ok(f) = fs::File::open(file) else {
            return;
        };
        let reader = BufReader::with_capacity(64 * 1024, f);
        let lines: Vec<String> = if context > 0 {
            // Need random access for context windows
            reader.lines().map_while(Result::ok).collect()
        } else {
            // Stream without materializing everything when no context
            for (idx, line) in reader.lines().enumerate() {
                if matches.len() >= max_results {
                    return;
                }
                let Ok(line) = line else { continue };
                if re.is_match(&line) {
                    let clipped = clip_line(&line, 500);
                    matches.push(format!("{}:{}:{}", display, idx + 1, clipped));
                }
            }
            return;
        };

        for (idx, line) in lines.iter().enumerate() {
            if matches.len() >= max_results {
                return;
            }
            if re.is_match(line) {
                if context == 0 {
                    matches.push(format!("{}:{}:{}", display, idx + 1, clip_line(line, 500)));
                } else {
                    let from = idx.saturating_sub(context);
                    let to = (idx + context + 1).min(lines.len());
                    for (j, ctx_line) in lines[from..to].iter().enumerate() {
                        if matches.len() >= max_results {
                            return;
                        }
                        let lineno = from + j + 1;
                        let mark = if from + j == idx { ':' } else { '-' };
                        matches.push(format!(
                            "{}:{}{}{}",
                            display,
                            lineno,
                            mark,
                            clip_line(ctx_line, 500)
                        ));
                    }
                    if matches.len() < max_results {
                        matches.push("--".into());
                    }
                }
            }
        }
    }
}

fn clip_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        let t: String = line.chars().take(max_chars).collect();
        format!("{t}…")
    }
}
