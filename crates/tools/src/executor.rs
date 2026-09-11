use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use rustc_hash::{FxHashMap, FxHasher};
use whycodes_core::types::{PermissionSet, ToolCall, ToolDefinition, ToolResult};

use super::tool::{Tool, ToolContext};
use crate::{
    apply_patch, background, blame, browser, checkpoint, code_mode, commit, diff, edit,
    external_directory, fetch, glob, grep, issue, list, log, lsp, memory, panel, plan, pr,
    question, read, repomap, schedule, search, shell, skill, status, swarm, swarm_message, task,
    todo_read, todo_write, tool_search, truncate, worktree, write,
};

/// Central executor that manages all available tools
pub struct ToolExecutor {
    /// Tool name → implementation. FxHash: names are local/trusted, not
    /// adversarial map keys from the network.
    tools: FxHashMap<String, Box<dyn Tool>>,
    /// Memoized [`Tool::definition`] JSON for (permissions, profile, extra).
    /// Invalidated on [`Self::register`] / [`Self::register_as`].
    defs_cache: Mutex<FxHashMap<DefsCacheKey, Arc<[ToolDefinition]>>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct DefsCacheKey {
    perm: u64,
    profile: crate::profile::ToolProfile,
    extra: Box<[String]>,
}

fn permission_fingerprint(permissions: &PermissionSet) -> u64 {
    let mut hasher = FxHasher::default();
    permissions.allowed_tools.hash(&mut hasher);
    permissions.denied_tools.hash(&mut hasher);
    permissions.allow_file_writes.hash(&mut hasher);
    permissions.allow_network.hash(&mut hasher);
    permissions.allow_shell.hash(&mut hasher);
    permissions.allowed_paths.hash(&mut hasher);
    let mut rules: Vec<_> = permissions.rules.iter().collect();
    rules.sort_unstable_by(|a, b| a.0.cmp(b.0));
    rules.len().hash(&mut hasher);
    for (key, action) in rules {
        key.hash(&mut hasher);
        action.hash(&mut hasher);
    }
    hasher.finish()
}

impl ToolExecutor {
    /// Create a new executor with all built-in tools
    pub fn new() -> Self {
        let mut executor = Self {
            tools: FxHashMap::default(),
            defs_cache: Mutex::new(FxHashMap::default()),
        };

        executor.register(Box::new(read::ReadTool::new()));
        executor.register(Box::new(write::WriteTool::new()));
        executor.register(Box::new(edit::EditTool::new()));
        executor.register(Box::new(grep::GrepTool::new()));
        executor.register(Box::new(glob::GlobTool::new()));
        executor.register(Box::new(list::ListTool::new()));
        executor.register(Box::new(repomap::RepoMapTool::new()));
        // Primary name matches OpenCode (`bash`); `shell` kept as legacy alias
        executor.register(Box::new(shell::ShellTool::new()));
        executor.register(Box::new(shell::ShellTool::as_shell()));
        executor.register(Box::new(browser::BrowserTool::new()));
        executor.register(Box::new(fetch::WebFetchTool::new()));
        executor.register(Box::new(search::WebSearchTool::new()));
        executor.register(Box::new(issue::GithubIssueTool::new()));
        executor.register(Box::new(pr::GitHubPrTool::new()));
        executor.register(Box::new(task::TaskTool::new()));
        executor.register(Box::new(swarm::SwarmTool::new()));
        executor.register(Box::new(swarm_message::SwarmMsgTool::new()));
        executor.register(Box::new(background::BgTool::new()));
        executor.register(Box::new(schedule::ScheduleTool::new()));
        executor.register(Box::new(tool_search::ToolSearchTool::new()));
        executor.register(Box::new(worktree::WorktreeTool::new()));
        executor.register(Box::new(diff::GitDiffTool::new()));
        executor.register(Box::new(log::GitLogTool::new()));
        executor.register(Box::new(status::GitStatusTool::new()));
        executor.register(Box::new(blame::GitBlameTool::new()));
        executor.register(Box::new(commit::GitCommitTool::new()));
        executor.register(Box::new(apply_patch::ApplyPatchTool::new()));
        executor.register(Box::new(todo_write::TodoWriteTool::new()));
        executor.register(Box::new(todo_read::TodoReadTool::new()));
        executor.register(Box::new(memory::MemoryTool::new()));
        executor.register(Box::new(checkpoint::CheckpointTool::new()));
        executor.register(Box::new(checkpoint::RewindTool::new()));
        executor.register(Box::new(question::QuestionTool::new()));
        executor.register(Box::new(panel::PanelTool::new()));
        executor.register(Box::new(plan::PlanTool::new()));
        executor.register(Box::new(code_mode::CodeModeTool::new()));
        executor.register(Box::new(external_directory::ExternalDirectoryTool::new()));
        executor.register(Box::new(truncate::TruncateTool::new()));
        executor.register(Box::new(skill::SkillTool::new()));
        executor.register(Box::new(lsp::LspTool::new()));

        // Alias for common model tool names
        executor.register(Box::new(todo_write::TodoWriteTool::as_todo()));

        executor
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
        self.invalidate_defs_cache();
    }

    /// Register a tool with a custom name (alias), ignoring the tool's own name
    pub fn register_as(&mut self, name: &str, tool: Box<dyn Tool>) {
        self.tools.insert(name.to_string(), tool);
        self.invalidate_defs_cache();
    }

    fn invalidate_defs_cache(&self) {
        self.defs_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
    pub fn get_definitions(&self, permissions: &PermissionSet) -> Arc<[ToolDefinition]> {
        self.get_definitions_profile(permissions, crate::profile::ToolProfile::Full)
    }

    /// Like [`get_definitions`] but limited to a [`crate::profile::ToolProfile`].
    ///
    /// `Core` shrinks the tools JSON prefix (~15 tools) for faster TTFT while
    /// still allowing execute of non-core tools if the model invents a name
    /// (execute path is not profile-gated — only the schema sent to the LLM).
    pub fn get_definitions_profile(
        &self,
        permissions: &PermissionSet,
        profile: crate::profile::ToolProfile,
    ) -> Arc<[ToolDefinition]> {
        self.get_definitions_profile_extra(permissions, profile, &[])
    }

    /// Core/full profile plus extra activated deferred tool names (tool_search).
    ///
    /// Returns a shared slice so a turn does not rebuild JSON schema on every
    /// LLM step when permissions/profile/extra are unchanged.
    pub fn get_definitions_profile_extra(
        &self,
        permissions: &PermissionSet,
        profile: crate::profile::ToolProfile,
        extra: &[String],
    ) -> Arc<[ToolDefinition]> {
        let mut extra_key: Vec<String> = extra.to_vec();
        extra_key.sort();
        extra_key.dedup();
        let key = DefsCacheKey {
            perm: permission_fingerprint(permissions),
            profile,
            extra: extra_key.into_boxed_slice(),
        };
        {
            let cache = self.defs_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(hit) = cache.get(&key) {
                return Arc::clone(hit);
            }
        }
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
        let arc: Arc<[ToolDefinition]> = defs.into();
        let mut cache = self.defs_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(key, Arc::clone(&arc));
        arc
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

    /// Replace the built-in `lsp` tool with a configured overlay.
    pub fn configure_lsp(&mut self, overlay: &whycodes_lsp::LspSettings) {
        self.register(Box::new(lsp::LspTool::with_overlay(overlay)));
    }

    /// Load shell plugins from `plugins.toml` then `plugin.json` trees.
    ///
    /// Order (later same `name` wins): global toml → project toml →
    /// `$CONFIG/plugins/*/plugin.json` → `<project>/.whycodes/plugins/*/plugin.json`.
    pub fn register_config_plugins(&mut self, project_dir: Option<&std::path::Path>) -> usize {
        let mut by_name = std::collections::BTreeMap::new();

        let toml = load_plugin_toml(project_dir);
        for cfg in toml.plugins {
            if let Some(cfg) = keep_plugin_cfg(cfg) {
                by_name.insert(cfg.name.clone(), cfg);
            }
        }

        let mut mgr = whycodes_plugin::PluginManager::new();
        mgr.discover_standard(project_dir);
        for spec in mgr.shell_specs() {
            if let Some(cfg) = keep_plugin_spec(
                spec.name,
                spec.command,
                spec.description,
                spec.parameters,
                spec.working_dir,
            ) {
                by_name.insert(cfg.name.clone(), cfg);
            }
        }

        let n = by_name.len();
        for cfg in by_name.into_values() {
            self.register(Box::new(crate::plugin::PluginShellTool::from_config(cfg)));
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

fn load_plugin_toml(project_dir: Option<&std::path::Path>) -> whycodes_skill::PluginRegistry {
    match project_dir {
        Some(dir) => whycodes_skill::PluginRegistry::load_layered(dir)
            .unwrap_or_else(|e| skipped_plugin_toml(&e.to_string(), "plugins.toml load skipped")),
        None => whycodes_skill::PluginRegistry::load_from_config().unwrap_or_else(|e| {
            skipped_plugin_toml(&e.to_string(), "global plugins.toml load skipped")
        }),
    }
}

fn skipped_plugin_toml(e: &str, msg: &'static str) -> whycodes_skill::PluginRegistry {
    tracing::debug!(error = %e, "{msg}");
    whycodes_skill::PluginRegistry::new()
}

fn skip_empty_plugin_cfg(name: &str, command: &str) -> bool {
    name.trim().is_empty() || command.trim().is_empty()
}

fn keep_plugin_cfg(cfg: whycodes_skill::PluginConfig) -> Option<whycodes_skill::PluginConfig> {
    if skip_empty_plugin_cfg(&cfg.name, &cfg.command) {
        None
    } else {
        Some(cfg)
    }
}

fn keep_plugin_spec(
    name: String,
    command: String,
    description: String,
    parameters: Option<serde_json::Value>,
    working_dir: std::path::PathBuf,
) -> Option<whycodes_skill::PluginConfig> {
    if skip_empty_plugin_cfg(&name, &command) {
        return None;
    }
    Some(whycodes_skill::PluginConfig {
        name,
        command,
        description,
        parameters,
        working_dir: Some(working_dir.to_string_lossy().into_owned()),
    })
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
