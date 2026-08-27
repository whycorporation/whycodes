use rustc_hash::FxHashMap;
use whycodes_core::types::{PermissionSet, ToolCall, ToolResult};

use super::tool::{Tool, ToolContext};
use crate::{
    apply_patch, background, blame, browser, checkpoint, code_mode, commit, diff, edit,
    external_directory, fetch, glob, grep, issue, list, log, lsp, memory, panel, plan, pr,
    question, read, schedule, search, shell, skill, status, swarm, swarm_message, task, todo_read,
    todo_write, tool_search, truncate, worktree, write,
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
    ) -> Vec<whycodes_core::types::ToolDefinition> {
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
    ) -> Vec<whycodes_core::types::ToolDefinition> {
        self.get_definitions_profile_extra(permissions, profile, &[])
    }

    /// Core/full profile plus extra activated deferred tool names (tool_search).
    pub fn get_definitions_profile_extra(
        &self,
        permissions: &PermissionSet,
        profile: crate::profile::ToolProfile,
        extra: &[String],
    ) -> Vec<whycodes_core::types::ToolDefinition> {
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

    /// Load shell plugins from `plugins.toml` then `plugin.json` trees.
    ///
    /// Order (later same `name` wins): global toml → project toml →
    /// `$CONFIG/plugins/*/plugin.json` → `<project>/.whycodes/plugins/*/plugin.json`.
    pub fn register_config_plugins(&mut self, project_dir: Option<&std::path::Path>) -> usize {
        let mut by_name = std::collections::BTreeMap::new();

        let toml = match project_dir {
            Some(dir) => whycodes_skill::PluginRegistry::load_layered(dir).unwrap_or_else(|e| {
                tracing::debug!(error = %e, "plugins.toml load skipped");
                whycodes_skill::PluginRegistry::new()
            }),
            None => whycodes_skill::PluginRegistry::load_from_config().unwrap_or_else(|e| {
                tracing::debug!(error = %e, "global plugins.toml load skipped");
                whycodes_skill::PluginRegistry::new()
            }),
        };
        for cfg in toml.plugins {
            if cfg.name.trim().is_empty() || cfg.command.trim().is_empty() {
                continue;
            }
            by_name.insert(cfg.name.clone(), cfg);
        }

        let mut mgr = whycodes_plugin::PluginManager::new();
        mgr.discover_standard(project_dir);
        for spec in mgr.shell_specs() {
            if spec.name.trim().is_empty() || spec.command.trim().is_empty() {
                continue;
            }
            by_name.insert(
                spec.name.clone(),
                whycodes_skill::PluginConfig {
                    name: spec.name,
                    command: spec.command,
                    description: spec.description,
                    parameters: spec.parameters,
                    working_dir: Some(spec.working_dir.to_string_lossy().into_owned()),
                },
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use whycodes_core::types::{PermissionAction, ToolResult};

    /// Minimal fake tool for registration/definition tests.
    struct FakeTool {
        name: String,
        desc: &'static str,
        allowed: bool,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            self.desc
        }

        fn parameters(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }

        async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
            ToolResult {
                tool_call_id: "fake".into(),
                content: format!("fake-executed:{}", args),
                is_error: false,
            }
        }

        fn is_allowed(&self, _permissions: &PermissionSet) -> bool {
            self.allowed
        }
    }

    fn fake(name: &str, allowed: bool) -> Box<dyn Tool> {
        Box::new(FakeTool {
            name: name.to_string(),
            desc: "a fake tool",
            allowed,
        })
    }

    #[test]
    fn new_registers_builtin_tools() {
        let ex = ToolExecutor::new();
        let names = ex.tool_names();
        // Sorted.
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        // File + search core.
        for n in [
            "read",
            "write",
            "edit",
            "grep",
            "glob",
            "list",
            "apply_patch",
            "bash",
            "shell",
            "todowrite",
            "todoread",
            "todo",
        ] {
            assert!(names.contains(&n.to_string()), "missing {n}");
        }
        // Web / github / lsp / plan-family.
        for n in [
            "webfetch",
            "browser",
            "github_issue",
            "github_pr",
            "lsp",
            "panel",
            "plan",
            "checkpoint",
            "rewind",
        ] {
            assert!(names.contains(&n.to_string()), "missing {n}");
        }
        // No duplicates from alias registration.
        assert_eq!(
            names.len(),
            names.iter().collect::<std::collections::HashSet<_>>().len()
        );
        assert!(
            names.len() >= 30,
            "expected >= 30 tools, got {}",
            names.len()
        );
    }

    #[test]
    fn default_matches_new() {
        let a = ToolExecutor::new();
        let b = ToolExecutor::default();
        assert_eq!(a.tool_names(), b.tool_names());
    }

    #[test]
    fn register_and_register_as() {
        let mut ex = ToolExecutor::new();
        ex.register(fake("fake_tool", true));
        assert!(ex.get("fake_tool").is_some());
        assert_eq!(ex.get("fake_tool").unwrap().description(), "a fake tool");

        // register_as overrides the tool's own name.
        ex.register_as("alias_name", fake("real_name", true));
        assert!(ex.get("alias_name").is_some());
        assert!(ex.get("real_name").is_none());
    }

    #[test]
    fn get_unknown_returns_none() {
        let ex = ToolExecutor::new();
        assert!(ex.get("definitely_not_a_tool").is_none());
    }

    #[test]
    fn definitions_sorted_by_default() {
        let ex = ToolExecutor::new();
        let defs = ex.get_definitions(&PermissionSet::default());
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(defs.iter().any(|d| d.name == "read"));
        assert!(defs.iter().all(|d| !d.description.is_empty()));
        assert!(defs.iter().all(|d| d.parameters["type"] == "object"));
        // Default PermissionSet denies shell/network/write categories.
        assert!(
            defs.iter()
                .all(|d| d.name != "bash" && d.name != "webfetch" && d.name != "write")
        );
    }

    #[test]
    fn definitions_include_category_tools_when_permissive() {
        let ex = ToolExecutor::new();
        let perms = PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..PermissionSet::default()
        };
        let defs = ex.get_definitions(&perms);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"write"));
        assert!(names.contains(&"webfetch"));
        // Browser resolves to Ask, which still keeps it in the schema.
        assert!(names.contains(&"browser"));
    }

    #[test]
    fn definitions_filtered_by_permission_rules() {
        let ex = ToolExecutor::new();
        // Exact deny rule.
        let mut perms = PermissionSet::default();
        perms
            .rules
            .insert("webfetch".into(), PermissionAction::Deny);
        let defs = ex.get_definitions(&perms);
        assert!(defs.iter().all(|d| d.name != "webfetch"));
        assert!(defs.iter().any(|d| d.name == "read"));
        // Glob deny `github_*`.
        let mut perms = PermissionSet::default();
        perms
            .rules
            .insert("github_*".to_string(), PermissionAction::Deny);
        let defs = ex.get_definitions(&perms);
        assert!(defs.iter().all(|d| !d.name.starts_with("github_")));
        // Legacy allow list: only listed tools survive (writes need the flag).
        let perms = PermissionSet {
            allowed_tools: Some(vec!["read".into(), "write".into()]),
            allow_file_writes: true,
            ..PermissionSet::default()
        };
        let defs = ex.get_definitions(&perms);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
        // Category flag: no shell.
        let perms = PermissionSet {
            allow_shell: false,
            ..PermissionSet::default()
        };
        let defs = ex.get_definitions(&perms);
        assert!(defs.iter().all(|d| d.name != "bash" && d.name != "shell"));
    }

    #[test]
    fn definitions_core_profile_limits_surface() {
        let ex = ToolExecutor::new();
        // Permissive perms so only the profile decides what shows up.
        let perms = PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..PermissionSet::default()
        };
        let core = ex.get_definitions_profile(&perms, crate::profile::ToolProfile::Core);
        let full = ex.get_definitions_profile(&perms, crate::profile::ToolProfile::Full);
        let core_names: Vec<&str> = core.iter().map(|d| d.name.as_str()).collect();
        assert!(core_names.contains(&"read"));
        assert!(core_names.contains(&"bash"));
        assert!(!core_names.contains(&"webfetch"));
        assert!(!core_names.contains(&"browser"));
        assert!(!core_names.contains(&"lsp"));
        assert!(core.len() < full.len());
        assert!(full.iter().any(|d| d.name == "webfetch"));
        // Core keeps todo aliases under real names.
        assert!(core_names.contains(&"todowrite"));
        assert!(core_names.contains(&"todo"));
        assert!(!core_names.contains(&"todo_write"));
        assert!(!core_names.contains(&"apply_patch"));
        assert!(core_names.contains(&"bg"));
        assert!(!core_names.contains(&"shell"));
        assert!(!core_names.contains(&"memory"));
        assert!(!core_names.contains(&"swarm"));
    }

    #[test]
    fn definitions_profile_extra_activates_deferred_tools() {
        let ex = ToolExecutor::new();
        let core = ex
            .get_definitions_profile(&PermissionSet::default(), crate::profile::ToolProfile::Core);
        assert!(core.iter().all(|d| d.name != "worktree"));
        let extra = ex.get_definitions_profile_extra(
            &PermissionSet::default(),
            crate::profile::ToolProfile::Core,
            &["worktree".to_string()],
        );
        assert!(extra.iter().any(|d| d.name == "worktree"));
    }

    #[test]
    fn deferred_catalog_lists_non_core_tools() {
        let ex = ToolExecutor::new();
        let perms = PermissionSet {
            allow_network: true,
            ..PermissionSet::default()
        };
        let cat = ex.deferred_catalog(&perms);
        let names: Vec<&str> = cat.iter().map(|(n, _)| n.as_str()).collect();
        // Sorted and deduped.
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(names.contains(&"webfetch"));
        assert!(names.contains(&"worktree"));
        assert!(!names.contains(&"read"));
        assert!(!names.contains(&"bash"));
        // Every entry has a description.
        assert!(cat.iter().all(|(_, d)| !d.is_empty()));
        // Permission filtering applies (network off hides webfetch again).
        let cat = ex.deferred_catalog(&PermissionSet::default());
        assert!(cat.iter().all(|(n, _)| n != "webfetch"));
    }

    #[test]
    fn is_allowed_used_for_definition_filtering() {
        // A tool whose is_allowed is false disappears even with default perms.
        let mut ex = ToolExecutor::new();
        ex.register(fake("ghost", false));
        let defs = ex.get_definitions(&PermissionSet::default());
        assert!(defs.iter().all(|d| d.name != "ghost"));
    }

    #[tokio::test]
    async fn execute_unknown_tool_errors() {
        let ex = ToolExecutor::new();
        let call = ToolCall {
            id: "t1".into(),
            name: "no_such_tool".into(),
            arguments: json!({}),
        };
        let ctx = ToolContext::new("/tmp");
        let res = ex.execute(&call, &ctx, &PermissionSet::default()).await;
        assert!(res.is_error);
        assert_eq!(res.tool_call_id, "t1");
        assert!(res.content.contains("Unknown tool"));
        assert!(res.content.contains("Available tools"));
        assert!(res.content.contains("read"));
    }

    #[tokio::test]
    async fn execute_denied_tool_errors() {
        let ex = ToolExecutor::new();
        let call = ToolCall {
            id: "t2".into(),
            name: "read".into(),
            arguments: json!({}),
        };
        let perms = PermissionSet {
            rules: [("read".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
            ..PermissionSet::default()
        };
        let ctx = ToolContext::new("/tmp");
        let res = ex.execute(&call, &ctx, &perms).await;
        assert!(res.is_error);
        assert!(res.content.contains("not allowed"));
    }

    #[tokio::test]
    async fn execute_runs_allowed_tool() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "line one\nline two\n").unwrap();
        let ex = ToolExecutor::new();
        let call = ToolCall {
            id: "t3".into(),
            name: "read".into(),
            arguments: json!({ "path": "hello.txt" }),
        };
        let ctx = ToolContext::new(dir.path().display().to_string());
        let res = ex.execute(&call, &ctx, &PermissionSet::default()).await;
        assert!(!res.is_error, "{}", res.content);
        assert!(res.content.contains("line one"));
        assert!(res.content.contains("line two"));
    }

    #[tokio::test]
    async fn execute_unknown_in_isolated_executor_lists_registered() {
        // A custom-registered tool must show up in the unknown-tool suggestion list.
        let mut ex = ToolExecutor::new();
        ex.register(fake("my_custom_tool", true));
        let call = ToolCall {
            id: "t4".into(),
            name: "my_custom_tool".into(),
            arguments: json!({ "x": 1 }),
        };
        let ctx = ToolContext::new("/tmp");
        let res = ex.execute(&call, &ctx, &PermissionSet::default()).await;
        assert!(!res.is_error);
        assert!(res.content.contains("fake-executed"));
    }

    #[test]
    fn register_config_plugins_project_toml() {
        // Isolate global config so the developer machine's plugins.toml cannot leak in.
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
        let dir = tempfile::tempdir().unwrap();
        let why = dir.path().join(".whycodes");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"[[plugins]]
name = "shout"
command = "echo SHOUT"
description = "Shouts back"

[[plugins]]
name = "empty-cmd"
command = ""
description = "Has no command"
"#,
        )
        .unwrap();
        let mut ex = ToolExecutor::new();
        let n = ex.register_config_plugins(Some(dir.path()));
        assert!(n >= 1, "expected at least the shout plugin, got {n}");
        // PluginShellTool registers under a `plugin_` prefixed name.
        let shout = ex.get("plugin_shout");
        assert!(shout.is_some(), "shout plugin tool not registered");
        assert_eq!(shout.unwrap().description(), "Shouts back");
        // Empty-command plugins are skipped.
        assert!(ex.get("plugin_empty-cmd").is_none());
        unsafe { std::env::remove_var("WHYCODES_HOME") };
    }

    #[test]
    fn register_config_plugins_skips_without_project_dir() {
        // No project dir: falls back to isolated (empty) global config only.
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
        let mut ex = ToolExecutor::new();
        let n = ex.register_config_plugins(None);
        assert_eq!(n, 0);
        unsafe { std::env::remove_var("WHYCODES_HOME") };
    }
}
