use rustc_hash::FxHashMap;
use whycode_core::types::{PermissionSet, ToolCall, ToolResult};

use super::tool::{Tool, ToolContext};
use crate::{
    apply_patch, bg, browser, code_mode, edit, external_directory, git_blame, git_commit, git_diff,
    git_log, git_status, github_issue, github_pr, glob, grep, list, lsp_tool, memory_tool, panel,
    plan, question, read, schedule, shell, skill_tool, swarm, swarm_msg, task, todo_read,
    todo_write, tool_search, truncate_tool, webfetch, websearch, worktree, write,
};

/// Central executor that manages all available tools
pub struct ToolExecutor {
    /// Tool name → implementation. FxHash: names are local/trusted, not
    /// adversarial map keys from the network.
    tools: FxHashMap<String, Box<dyn Tool>>,
}

impl ToolExecutor {
    /// Create a new executor with all built-in tools
    pub fn new() -> Self {
        let mut executor = Self {
            tools: FxHashMap::default(),
        };

        executor.register(Box::new(read::ReadTool::new()));
        executor.register(Box::new(write::WriteTool::new()));
        executor.register(Box::new(edit::EditTool::new()));
        executor.register(Box::new(grep::GrepTool::new()));
        executor.register(Box::new(glob::GlobTool::new()));
        executor.register(Box::new(list::ListTool::new()));
        // Primary name matches OpenCode (`bash`); `shell` kept as legacy alias
        executor.register(Box::new(shell::ShellTool::new()));
        executor.register(Box::new(shell::ShellTool::as_shell()));
        executor.register(Box::new(browser::BrowserTool::new()));
        executor.register(Box::new(webfetch::WebFetchTool::new()));
        executor.register(Box::new(websearch::WebSearchTool::new()));
        executor.register(Box::new(github_issue::GithubIssueTool::new()));
        executor.register(Box::new(github_pr::GitHubPrTool::new()));
        executor.register(Box::new(task::TaskTool::new()));
        executor.register(Box::new(swarm::SwarmTool::new()));
        executor.register(Box::new(swarm_msg::SwarmMsgTool::new()));
        executor.register(Box::new(bg::BgTool::new()));
        executor.register(Box::new(schedule::ScheduleTool::new()));
        executor.register(Box::new(tool_search::ToolSearchTool::new()));
        executor.register(Box::new(worktree::WorktreeTool::new()));
        executor.register(Box::new(git_diff::GitDiffTool::new()));
        executor.register(Box::new(git_log::GitLogTool::new()));
        executor.register(Box::new(git_status::GitStatusTool::new()));
        executor.register(Box::new(git_blame::GitBlameTool::new()));
        executor.register(Box::new(git_commit::GitCommitTool::new()));
        executor.register(Box::new(apply_patch::ApplyPatchTool::new()));
        executor.register(Box::new(todo_write::TodoWriteTool::new()));
        executor.register(Box::new(todo_read::TodoReadTool::new()));
        executor.register(Box::new(memory_tool::MemoryTool::new()));
        executor.register(Box::new(question::QuestionTool::new()));
        executor.register(Box::new(panel::PanelTool::new()));
        executor.register(Box::new(plan::PlanTool::new()));
        executor.register(Box::new(code_mode::CodeModeTool::new()));
        executor.register(Box::new(external_directory::ExternalDirectoryTool::new()));
        executor.register(Box::new(truncate_tool::TruncateTool::new()));
        executor.register(Box::new(skill_tool::SkillTool::new()));
        executor.register(Box::new(lsp_tool::LspTool::new()));

        // Alias for common model tool names
        executor.register(Box::new(todo_write::TodoWriteTool::as_todo()));

        executor
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Register a tool with a custom name (alias), ignoring the tool's own name
    pub fn register_as(&mut self, name: &str, tool: Box<dyn Tool>) {
        self.tools.insert(name.to_string(), tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Get all tool definitions for LLM requests, filtered by permissions.
    ///
    /// Sorted by tool name so the tools array is byte-stable across process
    /// restarts (FxHashMap order is not). Stable order helps provider prompt
    /// caches (prefix match) and makes multi-turn TTFT more predictable.
    pub fn get_definitions(
        &self,
        permissions: &PermissionSet,
    ) -> Vec<whycode_core::types::ToolDefinition> {
        self.get_definitions_profile(permissions, crate::profile::ToolProfile::Full)
    }

    /// Like [`get_definitions`] but limited to a [`crate::profile::ToolProfile`].
    ///
    /// `Core` shrinks the tools JSON prefix (~12 tools) for faster TTFT while
    /// still allowing execute of non-core tools if the model invents a name
    /// (execute path is not profile-gated — only the schema sent to the LLM).
    pub fn get_definitions_profile(
        &self,
        permissions: &PermissionSet,
        profile: crate::profile::ToolProfile,
    ) -> Vec<whycode_core::types::ToolDefinition> {
        self.get_definitions_profile_extra(permissions, profile, &[])
    }

    /// Core/full profile plus extra activated deferred tool names (tool_search).
    pub fn get_definitions_profile_extra(
        &self,
        permissions: &PermissionSet,
        profile: crate::profile::ToolProfile,
        extra: &[String],
    ) -> Vec<whycode_core::types::ToolDefinition> {
        let mut defs: Vec<_> = self
            .tools
            .values()
            .filter(|t| {
                t.is_allowed(permissions)
                    && (profile.includes(t.name()) || extra.iter().any(|n| n == t.name()))
            })
            .map(|t| t.definition())
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Deferred catalogue: tools not in the core profile (for tool_search).
    pub fn deferred_catalog(&self, permissions: &PermissionSet) -> Vec<(String, String)> {
        let mut out: Vec<_> = self
            .tools
            .values()
            .filter(|t| {
                t.is_allowed(permissions) && !crate::profile::ToolProfile::Core.includes(t.name())
            })
            .map(|t| (t.name().to_string(), t.description().to_string()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// All registered tool names (sorted).
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Load shell plugins from global + optional project `plugins.toml`.
    pub fn register_config_plugins(&mut self, project_dir: Option<&std::path::Path>) -> usize {
        let mut configs = Vec::new();
        match whycode_skill::PluginRegistry::load_from_config() {
            Ok(r) => configs.extend(r.plugins),
            Err(e) => tracing::debug!(error = %e, "global plugins.toml load skipped"),
        }
        if let Some(dir) = project_dir {
            let path = dir.join(".whycode").join("plugins.toml");
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(reg) = whycode_skill::PluginRegistry::parse_toml(&content)
            {
                configs.extend(reg.plugins);
            }
        }
        let mut n = 0;
        let mut seen = std::collections::HashSet::new();
        for cfg in configs {
            if cfg.name.trim().is_empty() || cfg.command.trim().is_empty() {
                continue;
            }
            if !seen.insert(cfg.name.clone()) {
                continue; // project overrides global when same name (last wins if we reverse)
            }
            self.register(Box::new(crate::plugin_tool::PluginShellTool::from_config(
                cfg,
            )));
            n += 1;
        }
        n
    }

    /// Execute a single tool call
    pub async fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        permissions: &PermissionSet,
    ) -> ToolResult {
        match self.get(&call.name) {
            Some(tool) => {
                if !tool.is_allowed(permissions) {
                    ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "Tool '{}' is not allowed with current permissions.",
                            call.name
                        ),
                        is_error: true,
                    }
                } else {
                    tool.execute(call.arguments.clone(), ctx).await
                }
            }
            None => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!(
                    "Unknown tool: '{}'. Available tools: {}",
                    call.name,
                    self.tools
                        .keys()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                is_error: true,
            },
        }
    }
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}
