use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkContext, SinkMatch};
use rayon::prelude::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::paths::{
    MAX_GREP_FILE_BYTES, display_path, file_len, resolve_path, visit_index, walk_files,
};
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
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with regex (ripgrep engine, in-process — no `rg` binary). \
         Respects .gitignore; skips binaries and heavy dirs (target, node_modules, .git, …). \
         Prefer over shell grep for project code search. Hits are \
         `path:line tag:text` so `edit` can name the line by tag."
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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

            grep_from_blocking(result.map_err(|e| e.to_string()))
        })
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
            .map_err(|e| invalid_regex(&e.to_string()))?;

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
        let files_searched;
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

            // Collect candidate files (index visit is cheap; content search is not).
            let mut files: Vec<(PathBuf, String)> = Vec::new();
            let mut collect = |file: &Path, rel: &str| -> bool {
                if let Some(g) = glob.as_ref() {
                    let name = file
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if !g.matches(&name) && !g.matches(rel) {
                        return true;
                    }
                }
                files.push((file.to_path_buf(), rel.to_string()));
                true
            };

            let used_index = if targets_hidden {
                false
            } else if let Some(idx) = file_index {
                visit_index(idx, path, &mut |file, rel, is_dir, _size| {
                    if is_dir {
                        return true;
                    }
                    collect(file, rel)
                })
                .is_some()
            } else {
                false
            };
            if !used_index {
                walk_files(path, &mut |file, rel| collect(file, rel));
            }

            files_searched = files.len();
            let stop = AtomicBool::new(false);
            let remaining = AtomicUsize::new(max_results);
            let per_file: Vec<(usize, Vec<String>)> = files
                .par_iter()
                .enumerate()
                .map(|(i, (file, rel))| {
                    grep_file_task(i, file, rel, &matcher, context, &stop, &remaining)
                })
                .collect();
            // Restore discovery order so tests and the model see a stable listing.
            let mut ordered: Vec<(usize, Vec<String>)> = per_file;
            ordered.sort_by_key(|(i, _)| *i);
            for (_, local) in ordered {
                if merge_file_matches(&mut matches, local, max_results) {
                    truncated = true;
                    break;
                }
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
        if search_file_at_cap(matches.len(), max_results) {
            return skip_search_at_cap();
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
        handle_search_err(
            file,
            searcher
                .search_path(matcher, file, &mut sink)
                .map_err(|e| e.to_string()),
        );
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

/// Collects ripgrep sink events into `path:line tag:text` (context uses `-`).
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
        sink_push_match(
            self.matches,
            self.max_results,
            self.display,
            mat.line_number().unwrap_or(0),
            mat.bytes(),
        )
    }

    fn context(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        sink_push_context(
            self.matches,
            self.max_results,
            self.display,
            ctx.line_number().unwrap_or(0),
            ctx.bytes(),
        )
    }

    fn context_break(&mut self, _searcher: &grep_searcher::Searcher) -> Result<bool, Self::Error> {
        sink_context_break(self.matches, self.max_results)
    }
}

fn grep_file_task(
    i: usize,
    file: &Path,
    rel: &str,
    matcher: &grep_regex::RegexMatcher,
    context: usize,
    stop: &AtomicBool,
    remaining: &AtomicUsize,
) -> (usize, Vec<String>) {
    if grep_should_stop(stop, remaining) {
        return skip_stopped_file(i);
    }
    let cap = remaining.load(Ordering::Relaxed);
    let mut local = Vec::new();
    GrepTool::search_file(file, rel, matcher, context, &mut local, cap);
    if !local.is_empty() {
        let n = local.len();
        let prev = remaining.fetch_sub(n.min(cap), Ordering::Relaxed);
        if prev <= n {
            stop.store(true, Ordering::Relaxed);
        }
    }
    (i, local)
}

fn sink_push_match(
    matches: &mut Vec<String>,
    max_results: usize,
    display: &str,
    lineno: u64,
    bytes: &[u8],
) -> Result<bool, std::io::Error> {
    if sink_at_cap(matches.len(), max_results) {
        return sink_stop();
    }
    let line = utf8_line(bytes);
    let tag = super::line_tag::tag_of(&line, super::line_tag::MIN_LEN);
    matches.push(super::line_tag::format_grep_line(
        display,
        lineno,
        &tag,
        &clip_line(&line, 500),
        ':',
    ));
    Ok(matches.len() < max_results)
}

fn sink_push_context(
    matches: &mut Vec<String>,
    max_results: usize,
    display: &str,
    lineno: u64,
    bytes: &[u8],
) -> Result<bool, std::io::Error> {
    if sink_at_cap(matches.len(), max_results) {
        return sink_stop();
    }
    let line = utf8_line(bytes);
    let tag = super::line_tag::tag_of(&line, super::line_tag::MIN_LEN);
    matches.push(super::line_tag::format_grep_line(
        display,
        lineno,
        &tag,
        &clip_line(&line, 500),
        '-',
    ));
    Ok(true)
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

fn grep_from_blocking(result: Result<Result<String, String>, String>) -> ToolResult {
    match result {
        Ok(Ok(output)) => grep_ok(output),
        Ok(Err(e)) => grep_err(&e),
        Err(e) => grep_join_error(&e),
    }
}

fn grep_join_error(e: &str) -> ToolResult {
    grep_err(&format!("grep task failed: {e}"))
}

fn grep_ok(output: String) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: if output.is_empty() {
            "No matches found.".to_string()
        } else {
            output
        },
        is_error: false,
    }
}

fn grep_err(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error: {e}"),
        is_error: true,
    }
}

fn invalid_regex(e: &str) -> String {
    format!("invalid regex: {e}")
}

fn sink_at_cap(matches: usize, max_results: usize) -> bool {
    matches >= max_results
}

fn sink_stop() -> Result<bool, std::io::Error> {
    Ok(false)
}

fn push_context_break(matches: &mut Vec<String>, max_results: usize) {
    if matches.len() < max_results && matches.last().is_none_or(|s| s != "--") {
        matches.push("--".into());
    }
}

fn empty_file_matches(i: usize) -> (usize, Vec<String>) {
    (i, Vec::new())
}

fn skip_stopped_file(i: usize) -> (usize, Vec<String>) {
    empty_file_matches(i)
}

fn search_file_at_cap(matches: usize, max_results: usize) -> bool {
    matches >= max_results
}

fn skip_search_at_cap() {}

fn handle_search_err(file: &Path, result: Result<(), String>) {
    if let Err(err) = result {
        skip_unsearchable(file, &err);
    }
}

fn sink_context_break(
    matches: &mut Vec<String>,
    max_results: usize,
) -> Result<bool, std::io::Error> {
    push_context_break(matches, max_results);
    Ok(true)
}

fn skip_unsearchable(file: &Path, err: &str) {
    skip_unsearchable_msg(&file.display().to_string(), err);
}

fn skip_unsearchable_msg(path: &str, err: &str) {
    tracing::debug!(
        path = %path,
        error = %err,
        "skipping file that could not be searched"
    );
}

fn grep_should_stop(stop: &AtomicBool, remaining: &AtomicUsize) -> bool {
    if stop.load(Ordering::Relaxed) {
        return true;
    }
    if remaining.load(Ordering::Relaxed) == 0 {
        stop.store(true, Ordering::Relaxed);
        return true;
    }
    false
}

fn merge_file_matches(
    matches: &mut Vec<String>,
    mut local: Vec<String>,
    max_results: usize,
) -> bool {
    if matches.len() >= max_results {
        return true;
    }
    let room = max_results - matches.len();
    let truncated = local.len() > room;
    if truncated {
        local.truncate(room);
    }
    matches.append(&mut local);
    truncated
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
