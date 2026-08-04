//! Built-in tools for Whycode.
//!
//! Domain modules group related tools; flat re-exports keep historical paths
//! (`whycode_tools::read`, …) working for tests and callers.

pub mod agent_tools;
pub mod display;
pub mod executor;
pub mod file;
pub mod git;
pub mod github;
pub mod lsp_tool;
pub mod mcp_tool;
pub mod shell;
pub mod tool;
pub mod web;

// Flat re-exports (stable paths)
pub use agent_tools::{
    code_mode, plan, question, skill_tool, task, todo_read, todo_write,
};
pub use file::{
    apply_patch, edit, external_directory, glob, grep, list, read, truncate_tool, truncation_dir,
    write,
};
pub use git::{git_blame, git_commit, git_diff, git_log, git_status};
pub use github::{github_api, github_issue, github_pr};
pub use web::{mcp_websearch, webfetch, websearch};

pub use executor::ToolExecutor;
pub use mcp_tool::{McpCaller, McpToolBridge};
pub use tool::Tool;
