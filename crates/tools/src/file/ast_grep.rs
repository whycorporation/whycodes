//! `ast_grep`: structural search with metavariables over parsed source.

use rayon::prelude::*;
use serde_json::json;

use super::ast::{
    LANGUAGE_NAMES, Target, captures, collect_targets, compile_patterns, pattern_for, read_source,
    scope_from_args,
};
use super::line_tag::{MIN_LEN, format_grep_line, tag_of};
use crate::tool::{Tool, ToolContext};
use ast_grep_core::Pattern;
use ast_grep_core::tree_sitter::LanguageExt;
use whycodes_core::types::ToolResult;

const DEFAULT_MAX_RESULTS: usize = 50;
const HARD_MAX_RESULTS: usize = 500;

#[derive(Default)]
pub struct AstGrepTool;

impl Tool for AstGrepTool {
    fn name(&self) -> &str {
        "ast_grep"
    }

    fn description(&self) -> &str {
        "Structural code search: match syntax trees, not text. The pattern is \
         code in the target language with metavariables: `$NAME` matches one \
         node, `$$$NAME` (or `$$$`) any number of nodes, `$_` one node without \
         capturing; a name used twice must match the same code. Example: \
         `$A.clone()`, `fn $F($$$ARGS) -> Result<$T> { $$$ }`. Languages: \
         rust, python, go, javascript, typescript, tsx (from the file \
         extension unless `language` is set). Hits are `path:line tag:text` \
         (tag usable by `edit`) followed by the captured metavariables."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Code pattern with $META / $$$META placeholders; must parse as one node"
                },
                "language": {
                    "type": "string",
                    "description": format!("Only search this language ({LANGUAGE_NAMES}); default: by file extension")
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: project root)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches (default: 50, max: 500)"
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
        let working_dir = ctx.working_dir.clone();
        Box::pin(async move {
            crate::blocking::tool(Box::new(move || search(&args, &working_dir))).await
        })
    }
}

fn search(args: &serde_json::Value, working_dir: &str) -> ToolResult {
    match run(args, working_dir) {
        Ok(content) => ToolResult {
            tool_call_id: String::new(),
            content,
            is_error: false,
        },
        Err(e) => ToolResult {
            tool_call_id: String::new(),
            content: format!("Error: {e}"),
            is_error: true,
        },
    }
}

fn run(args: &serde_json::Value, working_dir: &str) -> Result<String, String> {
    let scope = scope_from_args(args, working_dir)?;
    let max_results = args["max_results"]
        .as_u64()
        .map_or(DEFAULT_MAX_RESULTS, |n| n as usize)
        .clamp(1, HARD_MAX_RESULTS);
    let targets = collect_targets(&scope)?;
    let patterns = compile_patterns(&scope, &targets)?;
    let per_file: Vec<Vec<String>> = targets
        .par_iter()
        .map(|t| match pattern_for(&patterns, t.lang) {
            Some(p) => file_hits(t, p, max_results),
            None => Vec::new(),
        })
        .collect();
    let mut hits = Vec::new();
    let mut truncated = false;
    for file in per_file {
        if hits.len() + file.len() > max_results {
            hits.extend(file.into_iter().take(max_results - hits.len()));
            truncated = true;
            break;
        }
        hits.extend(file);
    }
    let langs: Vec<&str> = patterns.iter().map(|(l, _)| l.name()).collect();
    Ok(card(
        &scope.pattern,
        &hits,
        targets.len(),
        &langs,
        truncated,
    ))
}

/// One entry per match (`path:line tag:text` plus indented captures), at
/// most `cap + 1` so the caller can tell the cap was hit.
fn file_hits(target: &Target, pattern: &Pattern, cap: usize) -> Vec<String> {
    let Some(src) = read_source(&target.path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = src.lines().collect();
    let root = target.lang.ast_grep(&src);
    let mut out = Vec::new();
    for m in root.root().find_all(pattern).take(cap + 1) {
        let n = m.start_pos().line();
        let line = lines.get(n).copied().unwrap_or("");
        let mut entry = format_grep_line(
            &target.shown,
            n as u64 + 1,
            &tag_of(line, MIN_LEN),
            &super::ast::one_line(line),
            ':',
        );
        for cap in captures(&m, &src) {
            entry.push_str("\n    ");
            entry.push_str(&cap);
        }
        out.push(entry);
    }
    out
}

fn card(pattern: &str, hits: &[String], files: usize, langs: &[&str], truncated: bool) -> String {
    let files = format!("{files} file{} parsed", if files == 1 { "" } else { "s" });
    let langs = if langs.is_empty() {
        "no supported files".to_string()
    } else {
        langs.join(", ")
    };
    if hits.is_empty() {
        return format!(
            "No matches found. ast_grep · pattern `{pattern}` · {files} ({langs})\n\
             next: loosen the pattern with $META / $$$ placeholders, set `language`, \
             or use `grep` for text"
        );
    }
    let n = hits.len();
    let shown = if truncated {
        format!("shown {n} (cap hit; more may exist)")
    } else {
        format!("shown {n} of {n}")
    };
    let mut out = format!("ast_grep · pattern `{pattern}` · {shown} · {files} ({langs})\n");
    out.push_str(&hits.join("\n"));
    if truncated {
        out.push_str("\nnext: raise max_results or narrow path/language");
    }
    out
}

#[cfg(test)]
#[path = "ast_grep_tests.rs"]
mod tests;
