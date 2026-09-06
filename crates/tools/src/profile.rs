//! Tool surface profiles for faster TTFT.
//!
//! Shipping every built-in tool on every request bloats the tools JSON prefix.
//! `Core` keeps the hot coding loop (~12 names); `Full` is the complete set
//! (github, web, lsp, mcp-style helpers). Matches jcode/Claude “curated tools”
//! and Anthropic deferred-loading spirit.

use serde::{Deserialize, Serialize};

/// Which tools to advertise to the model on each LLM request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    /// Hot path for coding: files, search, shell, todos, subagent, tool_search.
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
            Self::Core => CORE_TOOL_NAMES.contains(&name),
        }
    }
}

/// Stable, sorted-friendly core set. Keep ≤ ~15 primary names for TTFT
/// (`todo` is an alias of `todowrite`; `bg` is the counterpart of
/// `bash background=true` — not extra product surface).
/// Everything else (`apply_patch`, `memory`, `schedule`, `swarm`, …)
/// is deferred and loaded via `tool_search`.
/// Names must match `Tool::name()` registrations in `executor.rs`.
const CORE_TOOL_NAMES: &[&str] = &[
    "bash",
    "bg", // list/read/kill for `bash background=true`
    "edit",
    "glob",
    "grep",
    "list",
    "question",
    "read",
    "repomap",
    "task",
    "todo", // alias of todowrite
    "todoread",
    "todowrite",
    "tool_search",
    "write",
];

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
