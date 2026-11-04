//! Per-run tallies behind `/stats`: model requests and tool calls.
//!
//! Counters only move at points the turn loop already visits (a finished
//! stream, a finished tool batch), so they add no work to a turn. Like
//! [`crate::SessionCost`] they live in memory: a resumed session counts from
//! zero, while prompts, edits and todos are read from what is stored.

use std::collections::BTreeMap;

use whycodes_core::todo::{TodoItem, TodoStatus};
use whycodes_core::types::{ToolCall, ToolResult, Usage};

use crate::session::Session;

/// Requests and tokens for one `provider/model`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelTally {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Calls and failures for one tool name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolTally {
    pub calls: u64,
    pub errors: u64,
}

/// What this run asked of models and tools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunStats {
    pub models: BTreeMap<String, ModelTally>,
    pub tools: BTreeMap<String, ToolTally>,
}

impl RunStats {
    /// One finished model request; `usage` may be empty (silent provider).
    pub fn record_request(&mut self, provider: &str, model: &str, usage: &Usage) {
        let key = format!("{provider}/{model}");
        let tally = self.models.entry(key).or_default();
        tally.requests += 1;
        tally.input_tokens += usage.input_tokens;
        tally.output_tokens += usage.output_tokens;
    }

    /// One tool batch; a call fails when its result is marked as an error.
    pub fn record_tools(&mut self, calls: &[ToolCall], results: &[ToolResult]) {
        for call in calls {
            let failed = results
                .iter()
                .any(|r| r.tool_call_id == call.id && r.is_error);
            let tally = self.tools.entry(call.name.clone()).or_default();
            tally.calls += 1;
            tally.errors += u64::from(failed);
        }
    }

    pub fn requests(&self) -> u64 {
        self.models.values().map(|t| t.requests).sum()
    }

    pub fn tool_calls(&self) -> u64 {
        self.tools.values().map(|t| t.calls).sum()
    }

    pub fn tool_errors(&self) -> u64 {
        self.tools.values().map(|t| t.errors).sum()
    }
}

/// Subagent launches are calls to this tool.
const TASK_TOOL: &str = "task";

/// Plain-text `/stats` body for the TUI and the line REPL.
pub fn stats_report(
    session: &Session,
    todos: &[TodoItem],
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let run = &session.run_stats;
    let mut lines = vec![
        "Session stats".to_string(),
        format!(
            "  duration:  {} since the session started",
            duration_label(now - session.created_at)
        ),
        format!("  prompts:   {}", session.prompt_count()),
        format!("  requests:  {} this run", run.requests()),
    ];
    for (model, t) in &run.models {
        lines.push(format!(
            "    {model}: {} · {} in / {} out",
            plural(t.requests, "request"),
            compact(t.input_tokens),
            compact(t.output_tokens)
        ));
    }
    lines.push(format!(
        "  tools:     {} · {} failed this run",
        plural(run.tool_calls(), "call"),
        run.tool_errors()
    ));
    let mut by_use: Vec<(&String, &ToolTally)> = run.tools.iter().collect();
    by_use.sort_by_key(|a| std::cmp::Reverse(a.1.calls));
    if !by_use.is_empty() {
        let parts: Vec<String> = by_use
            .iter()
            .map(|(name, t)| match t.errors {
                0 => format!("{name} {}", t.calls),
                e => format!("{name} {} ({e} failed)", t.calls),
            })
            .collect();
        lines.push(format!("    {}", parts.join(" · ")));
    }
    let tasks = run.tools.get(TASK_TOOL).map_or(0, |t| t.calls);
    lines.push(format!("  subagents: {tasks} launched this run"));
    lines.push(todo_line(todos));
    lines.push(format!("  edits:     {}", session.edit_stats.label()));
    let u = &session.usage;
    lines.push(format!(
        "  tokens:    {} in / {} out",
        compact(u.input_tokens),
        compact(u.output_tokens)
    ));
    lines.join("\n")
}

fn todo_line(todos: &[TodoItem]) -> String {
    let count = |s: TodoStatus| todos.iter().filter(|t| t.status == s).count();
    format!(
        "  todos:     {}/{} done · {} in progress · {} pending · {} cancelled",
        count(TodoStatus::Completed),
        todos.len(),
        count(TodoStatus::InProgress),
        count(TodoStatus::Pending),
        count(TodoStatus::Cancelled)
    )
}

fn plural(n: u64, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// `1.2k`, `3.4M`; exact below a thousand.
fn compact(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => n.to_string(),
    }
}

fn duration_label(d: chrono::TimeDelta) -> String {
    let mins = d.num_minutes().max(0);
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
#[path = "run_stats_tests.rs"]
mod tests;
