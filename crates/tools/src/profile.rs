//! Tool surface profiles for faster TTFT.
//!
//! Shipping every built-in tool on every request bloats the tools JSON prefix.
//! `Core` keeps the hot coding loop (~12 names); `Full` is the complete set
//! (github, web, lsp, mcp-style helpers). Matches jcode/Claude “curated tools”
//! and Anthropic deferred-loading spirit.

use serde::{Deserialize, Serialize};

/// Which tools to advertise to the model on each LLM request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    /// Hot path for coding: files, search, shell, patch, todos, subagent.
    #[default]
    Core,
    /// Every built-in tool (github, web, lsp, plan, …).
    Full,
}

impl ToolProfile {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" | "all" => Self::Full,
            _ => Self::Core,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Full => "full",
        }
    }

    /// Names included in the core profile (plus aliases registered under those names).
    pub fn core_names() -> &'static [&'static str] {
        CORE_TOOL_NAMES
    }

    /// Whether `name` is advertised under this profile.
    pub fn includes(self, name: &str) -> bool {
        match self {
            Self::Full => true,
            Self::Core => CORE_TOOL_NAMES.iter().any(|n| *n == name),
        }
    }
}

/// Stable, sorted-friendly core set. Keep ≤ ~12 primary names for TTFT.
const CORE_TOOL_NAMES: &[&str] = &[
    "apply_patch",
    "bash",
    "edit",
    "glob",
    "grep",
    "list",
    "read",
    "shell", // legacy alias of bash
    "task",
    "todo",       // alias
    "todo_read",
    "todo_write",
    "write",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_excludes_web_and_github() {
        assert!(ToolProfile::Core.includes("read"));
        assert!(ToolProfile::Core.includes("bash"));
        assert!(!ToolProfile::Core.includes("webfetch"));
        assert!(!ToolProfile::Core.includes("github_pr"));
        assert!(!ToolProfile::Core.includes("lsp"));
        assert!(ToolProfile::Full.includes("webfetch"));
    }
}
