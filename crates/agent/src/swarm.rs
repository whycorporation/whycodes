//! Swarm coordination helpers: report formatting and task parsing.
//!
//! Execution lives on [`crate::agent::Agent::execute_swarm_tool`]; this module
//! holds pure helpers and size bounds (jcode-style TLDR for long reports).

/// Soft cap for a single worker body in the aggregated swarm report.
pub const MAX_SWARM_COMPLETION_REPORT_CHARS: usize = 12_000;

/// Bodies longer than this must open with a short `TLDR:` line (or we synthesize one).
pub const SWARM_TLDR_REQUIRED_OVER_CHARS: usize = 2_000;

/// Hard ceiling on concurrent workers (config may set a lower max).
pub const SWARM_HARD_MAX_AGENTS: usize = 8;

/// One unit of work parsed from the `swarm` tool arguments.
#[derive(Debug, Clone)]
pub struct SwarmWorkerSpec {
    pub goal: String,
    pub context: Option<String>,
    pub subagent_type: String,
    pub paths: Vec<String>,
    pub max_turns: usize,
}

/// Parse `swarm` tool JSON into worker specs.
pub fn parse_swarm_tasks(args: &serde_json::Value) -> Result<Vec<SwarmWorkerSpec>, String> {
    let tasks = args
        .get("tasks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "swarm requires a non-empty `tasks` array".to_string())?;
    if tasks.is_empty() {
        return Err("swarm requires at least one task".into());
    }
    if tasks.len() > SWARM_HARD_MAX_AGENTS {
        return Err(format!(
            "swarm supports at most {SWARM_HARD_MAX_AGENTS} tasks (got {})",
            tasks.len()
        ));
    }

    let mut out = Vec::with_capacity(tasks.len());
    for (i, t) in tasks.iter().enumerate() {
        let goal = t
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if goal.is_empty() {
            return Err(format!("tasks[{i}]: goal is required"));
        }
        let context = t
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let subagent_type = t
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general")
            .to_string();
        let paths = t
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let max_turns = t.get("max_turns").and_then(|v| v.as_u64()).unwrap_or(15) as usize;
        out.push(SwarmWorkerSpec {
            goal,
            context,
            subagent_type,
            paths,
            max_turns,
        });
    }
    Ok(out)
}

/// Ensure long bodies start with a short TLDR for TUI collapse / parent agent.
pub fn ensure_tldr(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() <= SWARM_TLDR_REQUIRED_OVER_CHARS {
        return trimmed.to_string();
    }
    if trimmed.lines().next().is_some_and(|l| {
        let u = l.trim().to_ascii_uppercase();
        u.starts_with("TLDR:") || u.starts_with("TL;DR:")
    }) {
        return truncate_chars(trimmed, MAX_SWARM_COMPLETION_REPORT_CHARS);
    }
    // Synthesize from first non-empty sentence-ish line.
    let first = trimmed
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("completed");
    let summary: String = first.chars().take(160).collect();
    let rest = truncate_chars(
        trimmed,
        MAX_SWARM_COMPLETION_REPORT_CHARS.saturating_sub(200),
    );
    format!("TLDR: {summary}\n\n{rest}")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}\n…(truncated)")
}

/// Format one worker section of the aggregated report.
pub fn format_worker_report(
    worker_id: &str,
    subagent_type: &str,
    success: bool,
    duration_secs: f64,
    goal: &str,
    body: &str,
) -> String {
    let mark = if success { "✓" } else { "✕" };
    let body = ensure_tldr(body);
    format!(
        "### {worker_id} ({subagent_type}) {mark} {duration_secs:.1}s\n\
         **Goal:** {goal}\n\n\
         {body}\n"
    )
}

/// Aggregate wall-clock summary header.
pub fn format_swarm_header(n: usize, ok: usize, wall_secs: f64) -> String {
    format!("## Swarm results ({n} agents, {ok} ok, {wall_secs:.1}s wall)\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_two_tasks() {
        let args = json!({
            "tasks": [
                {"goal": "audit A", "paths": ["a.rs"]},
                {"goal": "audit B", "subagent_type": "explore"}
            ]
        });
        let specs = parse_swarm_tasks(&args).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].paths, vec!["a.rs"]);
        assert_eq!(specs[1].subagent_type, "explore");
    }

    #[test]
    fn tldr_injected_for_long_body() {
        let body = "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 50);
        let out = ensure_tldr(&body);
        assert!(out.starts_with("TLDR:"), "{out}");
    }

    #[test]
    fn short_body_unchanged() {
        assert_eq!(ensure_tldr("done"), "done");
    }
}
