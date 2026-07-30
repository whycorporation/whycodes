use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::Path;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Directories skipped during a recursive search, mirroring ripgrep's defaults.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".venv", "dist", "build"];

/// Bytes inspected when deciding whether a file is binary.
const BINARY_SNIFF_LEN: usize = 8192;

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
        "Search for a pattern in files using regex."
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

        match Self::search(pattern, &search_path, file_glob, max_results) {
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
    /// Search `path` for `pattern`, returning up to `max_results` lines
    /// formatted as `path:line:content`.
    ///
    /// Implemented in-process rather than shelling out to ripgrep or grep so
    /// the tool behaves identically on every platform.
    fn search(
        pattern: &str,
        path: &str,
        file_glob: Option<&str>,
        max_results: usize,
    ) -> Result<String, String> {
        let re = regex::Regex::new(pattern).map_err(|e| format!("invalid regex: {}", e))?;
        let glob = match file_glob {
            Some(g) => Some(glob::Pattern::new(g).map_err(|e| format!("invalid glob: {}", e))?),
            None => None,
        };

        let root = Path::new(path);
        if !root.exists() {
            return Err(format!("path not found: {}", path));
        }

        let mut matches = Vec::new();
        if root.is_file() {
            Self::search_file(root, root, &re, &mut matches, max_results);
        } else {
            Self::search_dir(root, root, &re, glob.as_ref(), &mut matches, max_results);
        }

        Ok(matches.join("\n"))
    }

    /// Recursively walk `dir`, searching every file that passes `glob`.
    fn search_dir(
        root: &Path,
        dir: &Path,
        re: &regex::Regex,
        glob: Option<&glob::Pattern>,
        matches: &mut Vec<String>,
        max_results: usize,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        // Sort so results are stable across platforms and filesystems.
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();

        for path in paths {
            if matches.len() >= max_results {
                return;
            }

            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if path.is_dir() {
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                Self::search_dir(root, &path, re, glob, matches, max_results);
            } else {
                if glob.is_some_and(|g| !g.matches(&name)) {
                    continue;
                }
                Self::search_file(root, &path, re, matches, max_results);
            }
        }
    }

    /// Append every matching line of `file` as `path:line:content`.
    fn search_file(
        root: &Path,
        file: &Path,
        re: &regex::Regex,
        matches: &mut Vec<String>,
        max_results: usize,
    ) {
        let Ok(bytes) = fs::read(file) else {
            return;
        };
        // Skip binaries the way `grep -I` does: a NUL byte near the start.
        if bytes.iter().take(BINARY_SNIFF_LEN).any(|b| *b == 0) {
            return;
        }

        // Paths are reported relative to the search root, matching ripgrep.
        let display = file.strip_prefix(root).unwrap_or(file).display();

        let text = String::from_utf8_lossy(&bytes);
        for (idx, line) in text.lines().enumerate() {
            if matches.len() >= max_results {
                return;
            }
            if re.is_match(line) {
                matches.push(format!("{}:{}:{}", display, idx + 1, line));
            }
        }
    }
}
