//! `ast_edit`: structural rewrite. A dry run (the default) returns the
//! unified diff; `dry_run: false` writes every changed file atomically and
//! runs the project formatter on it, like `edit`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

use super::ast::{
    LANGUAGE_NAMES, Replace, Target, collect_targets, compile_patterns, pattern_for, read_source,
    scope_from_args, unified_diff,
};
use crate::tool::{Tool, ToolContext};
use ast_grep_core::Pattern;
use ast_grep_core::tree_sitter::LanguageExt;
use whycodes_core::types::ToolResult;

pub struct AstEditTool {
    /// Shared with the executor (`[tools.format] enabled`).
    format_after_write: Arc<AtomicBool>,
}

impl AstEditTool {
    pub fn new(format_after_write: Arc<AtomicBool>) -> Self {
        Self { format_after_write }
    }
}

impl Tool for AstEditTool {
    fn name(&self) -> &str {
        "ast_edit"
    }

    fn description(&self) -> &str {
        "Structural rewrite: replace every syntax-tree match of `pattern` \
         (same metavariable syntax as `ast_grep`) with `rewrite`, where \
         `$NAME` / `$$$NAME` insert the captured code. Dry run by default: \
         returns the unified diff and writes nothing. Pass `dry_run: false` \
         to write the files (atomically; the project formatter runs after, \
         like `edit`). Run `ast_grep` first to check what matches."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Code pattern with $META / $$$META placeholders; must parse as one node"
                },
                "rewrite": {
                    "type": "string",
                    "description": "Replacement code; $META / $$$META insert what the pattern captured"
                },
                "language": {
                    "type": "string",
                    "description": format!("Only rewrite this language ({LANGUAGE_NAMES}); default: by file extension")
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to rewrite (default: project root)"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Return the diff without writing (default: true)"
                }
            },
            "required": ["pattern", "rewrite"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            if let Some(hit) = whycodes_core::harmony::scan_json(&args) {
                return error(format!(
                    "Error: {} — refusing to write leaked Harmony protocol text",
                    hit.summary()
                ));
            }
            let dry_run = args["dry_run"].as_bool().unwrap_or(true);
            let job_ctx = ctx.clone();
            let outcome =
                crate::blocking::tool(Box::new(move || rewrite(&args, &job_ctx, dry_run))).await;
            if dry_run || !self.format_after_write.load(Ordering::Relaxed) {
                return outcome;
            }
            format_written(outcome, &ctx.working_dir).await
        })
    }
}

fn error(content: String) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content,
        is_error: true,
    }
}

/// One file's planned rewrite.
struct FilePlan<'t> {
    target: &'t Target,
    src: String,
    edits: Vec<Replace>,
}

fn rewrite(args: &serde_json::Value, ctx: &ToolContext, dry_run: bool) -> ToolResult {
    match plan_and_apply(args, ctx, dry_run) {
        Ok(result) => result,
        Err(e) => error(format!("Error: {e}")),
    }
}

fn plan_and_apply(
    args: &serde_json::Value,
    ctx: &ToolContext,
    dry_run: bool,
) -> Result<ToolResult, String> {
    let scope = scope_from_args(args, &ctx.working_dir)?;
    let Some(template) = args["rewrite"].as_str() else {
        return Err("Missing required parameter `rewrite`.".into());
    };
    let targets = collect_targets(&scope)?;
    let patterns = compile_patterns(&scope, &targets)?;
    let plans: Vec<FilePlan<'_>> = targets
        .iter()
        .filter_map(|t| plan_file(t, pattern_for(&patterns, t.lang)?, template))
        .collect();
    let head = format!(
        "ast_edit · pattern `{}` → `{template}`",
        scope.pattern.trim()
    );
    if plans.is_empty() {
        return Ok(ToolResult {
            tool_call_id: String::new(),
            content: format!(
                "No changes. {head} · {} file{} parsed, nothing matched\n\
                 next: check the pattern with `ast_grep`",
                targets.len(),
                if targets.len() == 1 { "" } else { "s" }
            ),
            is_error: false,
        });
    }
    let replacements: usize = plans.iter().map(|p| p.edits.len()).sum();
    let mut diff = String::new();
    let mut failures = Vec::new();
    let mut written = Vec::new();
    for plan in &plans {
        let (text, _added, _removed) = unified_diff(&plan.target.shown, &plan.src, &plan.edits);
        diff.push_str(&text);
        if dry_run {
            continue;
        }
        match write_plan(plan, ctx) {
            Ok(()) => written.push(plan.target.shown.clone()),
            Err(e) => failures.push(format!("{}: {e}", plan.target.shown)),
        }
    }
    let files = format!(
        "{replacements} replacement{} in {} file{}",
        if replacements == 1 { "" } else { "s" },
        plans.len(),
        if plans.len() == 1 { "" } else { "s" }
    );
    let mut content = if dry_run {
        format!("{head} · {files} · dry run, nothing written\n{diff}")
    } else {
        format!("{head} · {files} · {} written\n{diff}", written.len())
    };
    if dry_run {
        content.push_str("next: call again with dry_run: false to write these changes");
    }
    if !failures.is_empty() {
        content.push_str("\nnot written:\n");
        content.push_str(&failures.join("\n"));
    }
    for path in &written {
        content.push_str(&format!("\nwrote {path}"));
    }
    Ok(ToolResult {
        tool_call_id: String::new(),
        content,
        is_error: !failures.is_empty(),
    })
}

fn plan_file<'t>(target: &'t Target, pattern: &Pattern, template: &str) -> Option<FilePlan<'t>> {
    let src = read_source(&target.path)?;
    let root = target.lang.ast_grep(&src);
    let mut edits: Vec<Replace> = root
        .root()
        .replace_all(pattern, template)
        .into_iter()
        .map(|e| Replace {
            start: e.position,
            end: e.position + e.deleted_length,
            text: String::from_utf8_lossy(&e.inserted_text).into_owned(),
        })
        .collect();
    edits.sort_by_key(|e| e.start);
    edits.retain(|e| src[e.start..e.end] != e.text);
    if edits.is_empty() {
        return None;
    }
    Some(FilePlan { target, src, edits })
}

fn write_plan(plan: &FilePlan<'_>, ctx: &ToolContext) -> Result<(), String> {
    ctx.check_file_write(&plan.target.path)?;
    let modified = super::ast::apply_replacements(&plan.src, &plan.edits);
    crate::file::atomic::write_atomic(&plan.target.path, modified).map_err(io_error)
}

fn io_error(e: std::io::Error) -> String {
    e.to_string()
}

/// Run the project formatter on every `wrote <path>` file and note it once
/// per formatter.
async fn format_written(mut result: ToolResult, working_dir: &str) -> ToolResult {
    let paths: Vec<String> = result
        .content
        .lines()
        .filter_map(|l| l.strip_prefix("wrote "))
        .map(String::from)
        .collect();
    let mut notes: Vec<String> = Vec::new();
    for path in paths {
        let mut scratch = ToolResult {
            tool_call_id: String::new(),
            content: String::new(),
            is_error: false,
        };
        crate::formatter::format_written_file(Some(&path), working_dir, &mut scratch).await;
        let note = scratch.content.trim().to_string();
        if !note.is_empty() && !notes.contains(&note) {
            notes.push(note);
        }
    }
    for note in notes {
        result.content.push_str("\n\n");
        result.content.push_str(&note);
    }
    result
}

#[cfg(test)]
#[path = "ast_edit_tests.rs"]
mod tests;
