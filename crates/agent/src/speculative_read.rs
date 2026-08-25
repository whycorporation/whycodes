//! Speculative early `read` while tool arguments are still streaming.
//!
//! When the model starts a `read` tool call, path (and optional offset/limit)
//! often appear in the JSON long before the stream ends. Starting the file I/O
//! then shaves wall-clock off tool-heavy turns — the result is only used if the
//! finalized arguments still match what we speculated on.

use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::task::JoinHandle;
use whycodes_core::ToolContext;
use whycodes_core::types::ToolResult;
use whycodes_tools::Tool;
use whycodes_tools::file::paths::resolve_path;
use whycodes_tools::file::read::ReadTool;

/// Default / hard limits must match `crates/tools/src/file/read.rs`.
const DEFAULT_LIMIT: usize = 400;
const HARD_LIMIT: usize = 2000;

/// One in-flight speculative read keyed by tool-call id.
pub struct SpeculativeRead {
    pub call_id: String,
    /// Canonical absolute path we started reading.
    pub path: PathBuf,
    pub offset: usize,
    pub limit: usize,
    pub handle: JoinHandle<ToolResult>,
}

/// Try to extract a complete enough `path` (+ optional window) from a partial
/// JSON argument buffer. Returns `None` until `path` is a closed string.
pub fn try_parse_read_args(buf: &str) -> Option<(String, usize, usize)> {
    let path = extract_json_string_field(buf, "path")?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let offset = extract_json_u64_field(buf, "offset")
        .map(|n| n.max(1) as usize)
        .unwrap_or(1);
    let limit = extract_json_u64_field(buf, "limit")
        .map(|n| (n as usize).clamp(1, HARD_LIMIT))
        .unwrap_or(DEFAULT_LIMIT);
    Some((path.to_string(), offset, limit))
}

/// Pull `"field": "…"` from incomplete JSON (no full parse required).
fn extract_json_string_field(buf: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let idx = buf.find(&key)?;
    let after_key = &buf[idx + key.len()..];
    let colon = after_key.find(':')?;
    let mut rest = after_key[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    rest = &rest[1..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(n) = chars.next() else {
                    // Escape not finished — path still streaming.
                    return None;
                };
                match n {
                    '"' | '\\' | '/' => out.push(n),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        // Incomplete unicode escape — wait.
                        return None;
                    }
                    other => out.push(other),
                }
            }
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    // Closing quote not seen yet.
    None
}

fn extract_json_u64_field(buf: &str, field: &str) -> Option<u64> {
    let key = format!("\"{field}\"");
    let idx = buf.find(&key)?;
    let after_key = &buf[idx + key.len()..];
    let colon = after_key.find(':')?;
    let rest = after_key[colon + 1..].trim_start();
    let mut digits = String::new();
    for c in rest.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else if digits.is_empty() {
            return None;
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Spawn a background read that runs the real [`ReadTool`] (identical format).
pub fn spawn_speculative_read(
    call_id: String,
    path: String,
    offset: usize,
    limit: usize,
    ctx: &ToolContext,
) -> Option<SpeculativeRead> {
    // Virtual `skill://` / `agent://` paths are not files on disk.
    if path.contains("://") {
        return None;
    }
    let full_path = resolve_path(&ctx.working_dir, &path);
    // Only speculate on paths that already exist as files — avoids racing
    // permission / missing-file noise for half-typed names.
    if !full_path.is_file() {
        return None;
    }
    let tool_ctx = ctx.clone();
    let args = json!({
        "path": path,
        "offset": offset,
        "limit": limit,
    });
    let id_for_result = call_id.clone();
    let handle = tokio::spawn(async move {
        let tool = ReadTool::new();
        let mut result = tool.execute(args, &tool_ctx).await;
        result.tool_call_id = id_for_result;
        result
    });

    Some(SpeculativeRead {
        call_id,
        path: full_path,
        offset,
        limit,
        handle,
    })
}

/// If a finished `read` tool call matches a speculative job, await it.
pub async fn take_matching(
    jobs: &mut Vec<SpeculativeRead>,
    call_id: &str,
    path: &str,
    offset: usize,
    limit: usize,
    working_dir: &str,
) -> Option<ToolResult> {
    let want = resolve_path(working_dir, path);
    let pos = jobs.iter().position(|j| {
        j.call_id == call_id && j.path == want && j.offset == offset && j.limit == limit
    })?;
    let job = jobs.swap_remove(pos);
    match job.handle.await {
        Ok(mut result) => {
            result.tool_call_id = call_id.to_string();
            Some(result)
        }
        Err(e) => {
            tracing::debug!(error = %e, "speculative read join failed");
            None
        }
    }
}

/// Drop all outstanding speculative jobs (cancel is cooperative via abort).
pub fn abort_all(jobs: &mut Vec<SpeculativeRead>) {
    for job in jobs.drain(..) {
        job.handle.abort();
    }
}

/// Resolve offset/limit from a final JSON value the same way the tool does.
pub fn window_from_args(args: &Value) -> (usize, usize) {
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, HARD_LIMIT);
    (offset, limit)
}

/// Maybe start a speculative read after a delta extended the arg buffer.
pub fn maybe_start(
    jobs: &mut Vec<SpeculativeRead>,
    call_id: &str,
    tool_name: &str,
    arg_buf: &str,
    ctx: &ToolContext,
) {
    if tool_name != "read" {
        return;
    }
    if jobs.iter().any(|j| j.call_id == call_id) {
        return;
    }
    // Also accept a complete object already in the buffer as pretty-printed JSON.
    let parsed = try_parse_read_args(arg_buf).or_else(|| {
        let v: Value = serde_json::from_str(arg_buf).ok()?;
        let path = v.get("path")?.as_str()?.trim().to_string();
        if path.is_empty() {
            return None;
        }
        let (o, l) = window_from_args(&v);
        Some((path, o, l))
    });
    let Some((path, offset, limit)) = parsed else {
        return;
    };
    if let Some(job) = spawn_speculative_read(call_id.to_string(), path, offset, limit, ctx) {
        tracing::debug!(
            id = %job.call_id,
            path = %job.path.display(),
            offset = job.offset,
            limit = job.limit,
            "speculative early read started"
        );
        jobs.push(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_path_only() {
        let (p, o, l) = try_parse_read_args(r#"{"path": "crates/foo/src/lib.rs"}"#).unwrap();
        assert_eq!(p, "crates/foo/src/lib.rs");
        assert_eq!(o, 1);
        assert_eq!(l, DEFAULT_LIMIT);
    }

    #[test]
    fn parse_with_window() {
        let (p, o, l) = try_parse_read_args(r#"{"path":"a.rs","offset":10,"limit":50}"#).unwrap();
        assert_eq!(p, "a.rs");
        assert_eq!(o, 10);
        assert_eq!(l, 50);
    }

    #[test]
    fn incomplete_path_returns_none() {
        assert!(try_parse_read_args(r#"{"path": "crates/fo"#).is_none());
    }

    #[test]
    fn escaped_path() {
        let (p, _, _) = try_parse_read_args(r#"{"path": "dir\\file.rs"}"#).unwrap();
        assert_eq!(p, "dir\\file.rs");
    }

    #[test]
    fn field_order_path_last() {
        let (p, o, l) =
            try_parse_read_args(r#"{"offset": 2, "limit": 10, "path": "z.rs"}"#).unwrap();
        assert_eq!(p, "z.rs");
        assert_eq!(o, 2);
        assert_eq!(l, 10);
    }

    #[test]
    fn window_clamped_to_hard_limit() {
        let (_, _, l) = try_parse_read_args(r#"{"path": "a.rs", "limit": 99999}"#).unwrap();
        assert_eq!(l, HARD_LIMIT);
        let (_, _, l) = try_parse_read_args(r#"{"path": "a.rs", "limit": 0}"#).unwrap();
        assert_eq!(l, 1);
        let (_, o, _) = try_parse_read_args(r#"{"path": "a.rs", "offset": 0}"#).unwrap();
        assert_eq!(o, 1, "offset floors at 1");
    }

    #[test]
    fn unicode_escape_incomplete_waits() {
        // `\u` escape unfinished → path still streaming.
        assert!(try_parse_read_args(r#"{"path": "caf\u"#).is_none());
    }

    #[test]
    fn trailing_backslash_waits_for_escape() {
        assert!(try_parse_read_args(r#"{"path": "dir\"#).is_none());
    }

    #[test]
    fn non_string_path_returns_none() {
        assert!(try_parse_read_args(r#"{"path": 123}"#).is_none());
    }

    #[test]
    fn window_from_args_matches_tool_semantics() {
        let v = serde_json::json!({"path": "a.rs"});
        assert_eq!(window_from_args(&v), (1, DEFAULT_LIMIT));
        let v = serde_json::json!({"path": "a.rs", "offset": 5, "limit": 100});
        assert_eq!(window_from_args(&v), (5, 100));
        let v = serde_json::json!({"offset": 0, "limit": 0});
        let (o, l) = window_from_args(&v);
        assert_eq!(o, 1);
        assert_eq!(l, 1);
    }

    #[test]
    fn abort_all_drains_jobs() {
        let mut jobs = Vec::new();
        abort_all(&mut jobs);
        assert!(jobs.is_empty());
    }

    #[test]
    fn spawn_skips_internal_schemes() {
        let ctx = whycodes_core::ToolContext::new("/tmp");
        assert!(spawn_speculative_read("c1".into(), "skill://demo".into(), 1, 10, &ctx).is_none());
        assert!(
            spawn_speculative_read("c1".into(), "agent://task-1".into(), 1, 10, &ctx).is_none()
        );
    }

    #[test]
    fn maybe_start_ignores_non_read_tools() {
        let mut jobs = Vec::new();
        let ctx = whycodes_core::ToolContext {
            working_dir: "/work/proj".into(),
            session_id: None,
            sandbox: whycodes_core::SandboxSettings::off(),
            network: whycodes_core::NetworkPolicy::unrestricted(),
            file_claims: None,
            agent_id: None,
            agent_label: None,
            file_index: None,
            panel: None,
            todo_sink: None,
            swarm_hub: None,
        };
        maybe_start(&mut jobs, "tc-1", "grep", r#"{"pattern": "x"}"#, &ctx);
        assert!(jobs.is_empty());
    }
}
