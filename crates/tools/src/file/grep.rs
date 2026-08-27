use async_trait::async_trait;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkContext, SinkMatch};
use serde_json::json;
use std::path::Path;

use super::paths::{MAX_GREP_FILE_BYTES, display_path, file_len, resolve_path, walk_files};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

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
        "Search file contents with regex (ripgrep engine, in-process — no `rg` binary). \
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
        file_index: Option<&whycodes_index::WorkspaceIndex>,
    ) -> Result<String, String> {
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(case_insensitive)
            .line_terminator(Some(b'\n'))
            .build(pattern)
            .map_err(|e| format!("invalid regex: {e}"))?;

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
                &matcher,
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
                Self::search_file(file, rel, &matcher, context, &mut matches, max_results);
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
        matcher: &grep_regex::RegexMatcher,
        context: usize,
        matches: &mut Vec<String>,
        max_results: usize,
    ) {
        if matches.len() >= max_results {
            return;
        }
        if file_len(file).is_some_and(|n| n > MAX_GREP_FILE_BYTES) {
            return;
        }

        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\0'))
            .line_number(true)
            .before_context(context)
            .after_context(context)
            .build();
        let before = matches.len();
        let mut sink = CollectSink {
            display,
            matches,
            max_results,
        };
        if let Err(err) = searcher.search_path(matcher, file, &mut sink) {
            tracing::debug!(
                path = %file.display(),
                error = %err,
                "skipping file that could not be searched"
            );
        }
        // Preserve the historical `path:line-…` / `--` context separator after
        // each file so existing tests and model-facing output stay stable.
        if context > 0
            && matches.len() > before
            && matches.len() < max_results
            && matches.last().is_none_or(|s| s != "--")
        {
            matches.push("--".into());
        }
    }
}

/// Collects ripgrep sink events into the existing `path:line:text` format.
struct CollectSink<'a> {
    display: &'a str,
    matches: &'a mut Vec<String>,
    max_results: usize,
}

impl Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if self.matches.len() >= self.max_results {
            return Ok(false);
        }
        let lineno = mat.line_number().unwrap_or(0);
        let line = utf8_line(mat.bytes());
        self.matches.push(format!(
            "{}:{}:{}",
            self.display,
            lineno,
            clip_line(&line, 500)
        ));
        Ok(self.matches.len() < self.max_results)
    }

    fn context(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        if self.matches.len() >= self.max_results {
            return Ok(false);
        }
        let lineno = ctx.line_number().unwrap_or(0);
        let line = utf8_line(ctx.bytes());
        self.matches.push(format!(
            "{}:{}-{}",
            self.display,
            lineno,
            clip_line(&line, 500)
        ));
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &grep_searcher::Searcher) -> Result<bool, Self::Error> {
        if self.matches.len() < self.max_results && self.matches.last().is_none_or(|s| s != "--") {
            self.matches.push("--".into());
        }
        Ok(true)
    }
}

fn utf8_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.trim_end_matches(['\n', '\r']).to_string()
}

fn clip_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        let t: String = line.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn clip_line_short_and_long() {
        assert_eq!(clip_line("short", 10), "short");
        assert_eq!(clip_line("short", 5), "short");
        let clipped = clip_line("abcdefghij", 4);
        assert_eq!(clipped, "abcd…");
        // Multibyte chars count as chars
        let clipped = clip_line("türkçe uzun satır", 4);
        assert_eq!(clipped, "türk…");
    }

    #[test]
    fn search_single_file_matches() {
        let dir = TempDir::new().unwrap();
        let f = write(&dir, "a.txt", "hello world\nfoo bar\nhello again\n");
        let out = GrepTool::search("hello", &f, None, false, 0, 50, "/", None).unwrap();
        assert!(out.contains("a.txt:1:hello world"));
        assert!(out.contains("a.txt:3:hello again"));
        assert!(out.contains("(2 matches in 1 file"));
    }

    #[test]
    fn search_single_file_no_matches() {
        let dir = TempDir::new().unwrap();
        let f = write(&dir, "a.txt", "hello\n");
        let out = GrepTool::search("zzz", &f, None, false, 0, 50, "/", None).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn search_directory_recursive() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        write(&dir, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
        write(&dir, "src/deep/mod.rs", "// helper\nfn helper() {}\n");
        write(&dir, "README.md", "no match here\n");

        let out = GrepTool::search("fn ", dir.path(), None, false, 0, 50, "/", None).unwrap();
        assert!(out.contains("src/main.rs:1"));
        assert!(out.contains("src/deep/mod.rs:2"));
        assert!(!out.contains("README.md"));
    }

    #[test]
    fn search_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let f = write(&dir, "a.txt", "Hello World\n");
        let out = GrepTool::search("hello", &f, None, true, 0, 50, "/", None).unwrap();
        assert!(out.contains("a.txt:1"));
    }

    #[test]
    fn search_include_glob_filters() {
        let dir = TempDir::new().unwrap();
        write(&dir, "keep.rs", "fn keep() {}\n");
        write(&dir, "skip.py", "fn keep() {}\n");

        let out =
            GrepTool::search("keep", dir.path(), Some("*.rs"), false, 0, 50, "/", None).unwrap();
        assert!(out.contains("keep.rs"));
        assert!(!out.contains("skip.py"));
    }

    #[test]
    fn search_context_lines() {
        let dir = TempDir::new().unwrap();
        let f = write(&dir, "a.txt", "before\nmatch\nafter\n");
        let out = GrepTool::search("match", &f, None, false, 1, 50, "/", None).unwrap();
        assert!(out.contains("a.txt:1-before"));
        assert!(out.contains("a.txt:2:match"));
        assert!(out.contains("a.txt:3-after"));
        assert!(out.contains("--"));
    }

    #[test]
    fn search_max_results_truncates_in_directory() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            write(&dir, &format!("f{i}.txt"), "match here\n");
        }
        let out = GrepTool::search("match", dir.path(), None, false, 0, 3, "/", None).unwrap();
        assert!(out.contains("[truncated at 3 matches"));
    }

    #[test]
    fn search_single_file_respects_max_results_without_notice() {
        // Single-file searches stop at max_results but do not append the
        // truncation notice (only directory walks set the flag).
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..10 {
            content.push_str(&format!("line {i} match\n"));
        }
        let f = write(&dir, "a.txt", &content);
        let out = GrepTool::search("match", &f, None, false, 0, 3, "/", None).unwrap();
        assert!(!out.contains("[truncated"));
        assert!(out.contains("(3 matches in 1 file"));
    }

    #[test]
    fn search_skips_binary_files() {
        let dir = TempDir::new().unwrap();
        write(&dir, "bin.dat", "text\x00with nul\n");
        let out = GrepTool::search("nul", dir.path(), None, false, 0, 50, "/", None).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn search_path_not_found() {
        let err = GrepTool::search(
            "x",
            Path::new("/nonexistent-xyz"),
            None,
            false,
            0,
            50,
            "/",
            None,
        )
        .unwrap_err();
        assert!(err.contains("path not found"));
    }

    #[test]
    fn search_invalid_regex() {
        let dir = TempDir::new().unwrap();
        let f = write(&dir, "a.txt", "x");
        let err = GrepTool::search("([", &f, None, false, 0, 50, "/", None).unwrap_err();
        assert!(err.contains("invalid regex"));
    }

    #[test]
    fn search_invalid_glob() {
        let dir = TempDir::new().unwrap();
        let f = write(&dir, "a.txt", "x");
        let err = GrepTool::search("x", &f, Some("["), false, 0, 50, "/", None).unwrap_err();
        assert!(err.contains("invalid glob"));
    }

    #[test]
    fn search_skips_heavy_dirs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        write(&dir, "target/debug/foo.rs", "match me\n");
        write(&dir, "keep.rs", "match me\n");
        let out = GrepTool::search("match", dir.path(), None, false, 0, 50, "/", None).unwrap();
        assert!(out.contains("keep.rs"));
        assert!(!out.contains("foo.rs"));
    }

    #[test]
    fn search_respects_gitignore() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".gitignore"), "secret.rs\n").unwrap();
        write(&dir, "keep.rs", "match me\n");
        write(&dir, "secret.rs", "match me\n");
        let out = GrepTool::search("match", dir.path(), None, false, 0, 50, "/", None).unwrap();
        assert!(out.contains("keep.rs"), "{out}");
        assert!(!out.contains("secret.rs"), "{out}");
    }

    #[test]
    fn search_large_file_skipped() {
        let dir = TempDir::new().unwrap();
        let big = vec![b'a'; (MAX_GREP_FILE_BYTES + 1) as usize];
        let f = write(&dir, "big.txt", "");
        fs::write(&f, big).unwrap();
        let out = GrepTool::search("a", &f, None, false, 0, 50, "/", None).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn search_context_includes_markers_only_between() {
        let dir = TempDir::new().unwrap();
        // Two matches with context should not end with a lone `--` at EOF boundary
        let f = write(&dir, "a.txt", "x\n\n\n");
        let out = GrepTool::search("x", &f, None, false, 1, 50, "/", None).unwrap();
        assert!(out.contains("a.txt:1:x"));
    }

    #[tokio::test]
    async fn execute_missing_pattern_is_error() {
        let ctx = ToolContext::new("/");
        let result = GrepTool::new().execute(serde_json::json!({}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter"));
    }
}
