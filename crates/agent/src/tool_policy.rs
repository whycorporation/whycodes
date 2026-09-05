//! Permission-prompt formatting and parallel-tool policy.
//!
//! Split out of `agent.rs` so the turn loop is not also a formatting crate.

use whycodes_core::types::ToolCall;

pub(crate) const SHELL_TOOLS: &[&str] = &["bash", "shell"];

/// Soft cap for permission prompt detail (TUI wraps; avoid megabyte dumps).
pub(crate) const PERMISSION_DETAIL_MAX: usize = 4_000;

/// Human-readable tool arguments for the permission dialog (not compact JSON).
pub(crate) fn format_permission_detail(args: &serde_json::Value) -> String {
    let text = match args {
        serde_json::Value::Object(map) if map.is_empty() => "(no arguments)".to_string(),
        serde_json::Value::Object(map) => {
            // Single `command` field → show the shell string alone (most common).
            if map.len() == 1
                && let Some(cmd) = map.get("command").and_then(|v| v.as_str())
            {
                return truncate_permission_detail(cmd);
            }
            let mut lines = Vec::with_capacity(map.len());
            for (key, value) in map {
                match value {
                    serde_json::Value::String(s) => {
                        if s.contains('\n') || s.chars().count() > 72 {
                            lines.push(format!("{key}:"));
                            for line in s.lines() {
                                lines.push(format!("  {line}"));
                            }
                        } else {
                            lines.push(format!("{key}: {s}"));
                        }
                    }
                    serde_json::Value::Null => lines.push(format!("{key}: null")),
                    serde_json::Value::Bool(b) => lines.push(format!("{key}: {b}")),
                    serde_json::Value::Number(n) => lines.push(format!("{key}: {n}")),
                    other => {
                        let pretty = serde_json::to_string_pretty(other)
                            .unwrap_or_else(|_| other.to_string());
                        if pretty.contains('\n') {
                            lines.push(format!("{key}:"));
                            for line in pretty.lines() {
                                lines.push(format!("  {line}"));
                            }
                        } else {
                            lines.push(format!("{key}: {pretty}"));
                        }
                    }
                }
            }
            lines.join("\n")
        }
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    truncate_permission_detail(&text)
}

pub(crate) fn format_shell_risk_detail(command: &str, reason: &str) -> String {
    let body = format!("Command:\n{command}\n\nRisk: {reason}");
    truncate_permission_detail(&body)
}

pub(crate) fn truncate_permission_detail(s: &str) -> String {
    if s.chars().count() <= PERMISSION_DETAIL_MAX {
        return s.to_string();
    }
    let kept: String = s.chars().take(PERMISSION_DETAIL_MAX).collect();
    format!("{kept}…")
}

/// Worktree names: short, no path separators / traversal.
pub(crate) fn is_safe_worktree_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Path argument for file mutators / readers (permission path globs).
pub(crate) fn path_outside_workspace(path: &str, working_dir: &str) -> bool {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        let cwd = std::path::Path::new(working_dir);
        return std::fs::canonicalize(p)
            .ok()
            .and_then(|abs| {
                std::fs::canonicalize(cwd)
                    .ok()
                    .map(|root| !abs.starts_with(&root))
            })
            .unwrap_or(true);
    }
    let first = path.split(['/', '\\']).find(|s| !s.is_empty());
    matches!(first, Some(".." | "~"))
}

pub(crate) fn file_tool_path(tc: &ToolCall) -> Option<String> {
    let key = match tc.name.as_str() {
        "read" | "write" | "edit" | "glob" | "list" | "repomap" => "path",
        "apply_patch" => "path", // may also use multi-file; path optional
        "grep" => "path",
        _ => return None,
    };
    tc.arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().replace('\\', "/"))
        .filter(|s| !s.is_empty())
}

/// Tools that must never fan out in parallel (side effects, races, or UI ask).
pub(crate) const SERIAL_TOOLS: &[&str] = &[
    "bash",
    "shell",
    "write",
    "edit",
    "apply_patch",
    "git_commit",
    "todowrite",
    "todo",
    "task",
    "swarm",
    "bg",
    "schedule",
    "tool_search",
    "worktree",
    "plan",
    "browser",
    "question",
    "code_mode",
    "skill",
    "external_directory",
    "memory",
];

/// Whether this tool can safely run beside other tools in the same step.
///
/// Industry pattern (OpenCode issue #24764, Codex parallel function calls):
/// fan out independent reads; keep mutators and permission-gated tools serial.
pub(crate) fn is_parallel_safe_tool(
    name: &str,
    _permission: &whycodes_core::types::PermissionSet,
) -> bool {
    // Mutators/shell stay serial. Permission Ask is fine in parallel now that
    // the TUI queues permission dialogs (VecDeque), not a single slot.
    !SERIAL_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use whycodes_core::types::{PermissionSet, ToolCall};

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn permission_detail_and_parallel_policy() {
        assert_eq!(format_permission_detail(&json!({})), "(no arguments)");
        assert_eq!(format_permission_detail(&json!({"command": "ls"})), "ls");
        let multi = format_permission_detail(&json!({"path": "a.rs", "limit": 1, "ok": true}));
        assert!(multi.contains("path:"));
        assert!(multi.contains("limit:"));
        let long_str = "x".repeat(80);
        let nested = format_permission_detail(&json!({"note": long_str, "meta": {"k": 1}}));
        assert!(nested.contains("note:"));
        assert!(format_permission_detail(&json!(["a", "b"])).contains("a"));
        let huge = "a".repeat(PERMISSION_DETAIL_MAX + 10);
        assert!(truncate_permission_detail(&huge).ends_with("…"));
        assert_eq!(truncate_permission_detail("short"), "short");
        let mixed =
            format_permission_detail(&json!({"n": 1, "ok": false, "z": null, "obj": {"a": 1}}));
        assert!(mixed.contains("n:"));
        assert!(mixed.contains("ok:"));
        assert!(mixed.contains("z:"));
        assert!(format_shell_risk_detail("rm -rf /", "destructive").contains("Risk:"));
        let multiline = format_permission_detail(&json!({"note": "line1\nline2"}));
        assert!(multiline.contains("note:"));

        assert!(is_safe_worktree_name("w0"));
        assert!(is_safe_worktree_name("ab-c_1"));
        assert!(!is_safe_worktree_name(""));
        assert!(!is_safe_worktree_name("../x"));
        assert!(!is_safe_worktree_name(&"x".repeat(65)));

        let cwd = std::env::current_dir().unwrap();
        let cwd_s = cwd.to_string_lossy();
        assert!(path_outside_workspace("../secret", &cwd_s));
        assert!(path_outside_workspace("~/.ssh", &cwd_s));
        assert!(!path_outside_workspace("src/lib.rs", &cwd_s));
        assert!(path_outside_workspace("/tmp/outside", &cwd_s));

        assert_eq!(
            file_tool_path(&call("read", json!({"path": "a.rs"}))).as_deref(),
            Some("a.rs")
        );
        assert_eq!(
            file_tool_path(&call("repomap", json!({"path": "src"}))).as_deref(),
            Some("src")
        );
        assert!(file_tool_path(&call("bash", json!({"command": "ls"}))).is_none());

        let perms = PermissionSet {
            allowed_tools: None,
            denied_tools: None,
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            allowed_paths: None,
            rules: Default::default(),
        };
        assert!(is_parallel_safe_tool("read", &perms));
        assert!(is_parallel_safe_tool("grep", &perms));
        assert!(is_parallel_safe_tool("glob", &perms));
        assert!(is_parallel_safe_tool("repomap", &perms));
        assert!(!is_parallel_safe_tool("bash", &perms));
        assert!(!is_parallel_safe_tool("write", &perms));
        assert!(!is_parallel_safe_tool("edit", &perms));
        assert!(!is_parallel_safe_tool("shell", &perms));
        assert!(!is_parallel_safe_tool("question", &perms));
        let deny_shell = PermissionSet {
            allowed_tools: None,
            denied_tools: None,
            allow_file_writes: true,
            allow_network: true,
            allow_shell: false,
            allowed_paths: None,
            rules: Default::default(),
        };
        assert!(!is_parallel_safe_tool("bash", &deny_shell));
    }
}
