//! Built-in tools for WhyCodes.
//!
//! Domain modules group related tools; crate-root re-exports keep short
//! paths (`whycodes_tools::read`, `whycodes_tools::blame`, …) for callers.

pub mod agent_tools;
pub mod display;
pub mod executor;
pub mod file;
pub mod git;
pub mod github;
pub mod lsp;
pub mod mcp;
pub mod plugin;
pub mod profile;
pub mod shell;
pub mod tool;
pub mod web;

// Flat re-exports (stable paths)
pub use agent_tools::{
    background, checkpoint, code_mode, memory, panel, plan, question, schedule, skill, swarm,
    swarm_message, task, todo_read, todo_write, tool_search, worktree,
};
pub use file::{
    apply_patch, edit, external_directory, glob, grep, list, read, truncate, truncation_dir, write,
};
pub use git::{blame, commit, diff, log, status};
pub use github::{api, issue, pr};
pub use web::{browser, fetch, mcp_search, search};

pub use executor::ToolExecutor;
pub use lsp::LspTool;
pub use mcp::{McpCaller, McpToolBridge};
pub use plugin::{ListedPlugin, PluginShellTool, list_shell_plugins};
pub use profile::ToolProfile;
pub use tool::Tool;
