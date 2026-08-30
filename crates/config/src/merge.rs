//! Layered config merge.

use crate::types::{
    Config, MagicKeywordsConfig, QuestionToolConfig, ToolsConfig, default_agent,
    default_auto_index_chunks, default_auto_index_files, default_code_min_score,
    default_code_top_k, default_compaction_llm, default_compaction_threshold,
    default_consolidate_max, default_intent_guidance, default_max_background_jobs,
    default_max_tokens, default_memory_backend, default_memory_embed_dim,
    default_memory_index_bytes, default_memory_index_lines, default_memory_min_score,
    default_memory_scope, default_memory_token_budget, default_memory_top_k, default_model_race,
    default_prompt_cache, default_race_after_ms, default_response_cache, default_retain_every_n,
    default_retain_max_facts, default_risk_threshold, default_sandbox_fallback,
    default_sandbox_mode, default_session_min_score, default_session_top_k,
    default_swarm_max_agents, default_tool_profile,
};
use whycodes_core::types::PermissionSet;

impl Config {
    /// Deep-merge `other` into `self`, producing a new Config.
    ///
    /// Fields in `other` that are `Some(_)` or non-empty collections
    /// override corresponding fields in `self`. This implements the
    /// layering semantics where higher-priority layers win.
    pub fn merge_with(&self, other: &Config) -> Config {
        let mut merged = self.clone();

        // Providers: merge per-provider
        for (key, provider) in &other.providers {
            merged
                .providers
                .entry(key.clone())
                .and_modify(|existing| {
                    if provider.api_key.is_some() {
                        existing.api_key = provider.api_key.clone();
                    }
                    if provider.api_base.is_some() {
                        existing.api_base = provider.api_base.clone();
                    }
                    if provider.base_url.is_some() {
                        existing.base_url = provider.base_url.clone();
                    }
                    if provider.headers.is_some() {
                        existing.headers = provider.headers.clone();
                    }
                    if !provider.models.is_empty() {
                        existing.models = provider.models.clone();
                    }
                    if provider.tool_arguments.is_some() {
                        existing.tool_arguments = provider.tool_arguments;
                    }
                    if !provider.extra.is_empty() {
                        existing.extra = provider.extra.clone();
                    }
                })
                .or_insert_with(|| provider.clone());
        }

        // Models: merge per model_id+provider_id key
        for (key, model) in &other.models {
            merged
                .models
                .entry(key.clone())
                .and_modify(|existing| {
                    if model.max_tokens.is_some() {
                        existing.max_tokens = model.max_tokens;
                    }
                    if model.context_window.is_some() {
                        existing.context_window = model.context_window;
                    }
                    if model.temperature.is_some() {
                        existing.temperature = model.temperature;
                    }
                    if model.top_p.is_some() {
                        existing.top_p = model.top_p;
                    }
                    if model.thinking.is_some() {
                        existing.thinking = model.thinking;
                    }
                    if model.supports_tools.is_some() {
                        existing.supports_tools = model.supports_tools;
                    }
                    if model.supports_images.is_some() {
                        existing.supports_images = model.supports_images;
                    }
                })
                .or_insert_with(|| model.clone());
        }

        // Agents: append unique (by name)
        for agent in &other.agents {
            if !merged.agents.iter().any(|a| a.name == agent.name) {
                merged.agents.push(agent.clone());
            }
        }

        // Default agent (non-empty override wins)
        if other.default_agent != default_agent() || !other.default_agent.is_empty() {
            // only override if explicitly set (not the default value from
            // deserialization). Since we can't tell, always take other's value
            // when merging (other is the higher-priority layer).
            merged.default_agent = other.default_agent.clone();
        }

        // Default model
        if other.default_model.is_some() {
            merged.default_model = other.default_model.clone();
        }

        // Command configs: merge per-command
        for (cmd, cmd_cfg) in &other.command_configs {
            merged
                .command_configs
                .entry(cmd.clone())
                .and_modify(|existing| {
                    if cmd_cfg.model.is_some() {
                        existing.model = cmd_cfg.model.clone();
                    }
                    if cmd_cfg.agent.is_some() {
                        existing.agent = cmd_cfg.agent.clone();
                    }
                    if cmd_cfg.max_turns.is_some() {
                        existing.max_turns = cmd_cfg.max_turns;
                    }
                })
                .or_insert_with(|| cmd_cfg.clone());
        }

        // Tools
        merged.tools = self.tools.merge_with(&other.tools);

        // Session
        if other.session.max_context_tokens != default_max_tokens() {
            merged.session.max_context_tokens = other.session.max_context_tokens;
        }
        if other.session.compaction_threshold != default_compaction_threshold() {
            merged.session.compaction_threshold = other.session.compaction_threshold;
        }
        if other.session.store_path.is_some() {
            merged.session.store_path = other.session.store_path.clone();
        }
        // auto_title defaults true; only an explicit false in a higher layer wins
        // when the lower layer left the default — merge by taking `other` always
        // for bools that appear in TOML (serde always deserializes them).
        if !other.session.auto_title {
            merged.session.auto_title = false;
        }
        if other.session.title_model.is_some() {
            merged.session.title_model = other.session.title_model.clone();
        }
        if other.session.tool_profile != default_tool_profile() {
            merged.session.tool_profile = other.session.tool_profile.clone();
        }
        if other.session.prompt_cache != default_prompt_cache() {
            merged.session.prompt_cache = other.session.prompt_cache.clone();
        }
        if other.session.compaction_llm != default_compaction_llm() {
            merged.session.compaction_llm = other.session.compaction_llm.clone();
        }
        if other.session.reasoning_effort.is_some() {
            merged.session.reasoning_effort = other.session.reasoning_effort.clone();
        }
        if other.session.model_fast.is_some() {
            merged.session.model_fast = other.session.model_fast.clone();
        }
        if other.session.model_smol.is_some() {
            merged.session.model_smol = other.session.model_smol.clone();
        }
        if other.session.model_plan.is_some() {
            merged.session.model_plan = other.session.model_plan.clone();
        }
        if !other.session.stream_rules.is_empty() {
            merged.session.stream_rules = other.session.stream_rules.clone();
        }
        if other.session.model_race != default_model_race() {
            merged.session.model_race = other.session.model_race.clone();
        }
        if other.session.race_after_ms != default_race_after_ms() {
            merged.session.race_after_ms = other.session.race_after_ms;
        }
        if other.session.response_cache != default_response_cache() {
            merged.session.response_cache = other.session.response_cache.clone();
        }
        if other.session.intent_guidance != default_intent_guidance() {
            merged.session.intent_guidance = other.session.intent_guidance.clone();
        }
        if other.session.magic_keywords != MagicKeywordsConfig::default() {
            merged.session.magic_keywords = other.session.magic_keywords.clone();
        }

        // TUI
        if other.tui.theme.is_some() {
            merged.tui.theme = other.tui.theme.clone();
        }
        if other.tui.key_bindings.is_some() {
            merged.tui.key_bindings = other.tui.key_bindings.clone();
        }
        // Higher layer can only turn the sidebar on (default is off).
        merged.tui.show_sidebar |= other.tui.show_sidebar;
        for (name, spec) in &other.tui.agent_colors {
            merged.tui.agent_colors.insert(name.clone(), spec.clone());
        }

        // General
        if other.general.project_path.is_some() {
            merged.general.project_path = other.general.project_path.clone();
        }
        if other.general.log_level.is_some() {
            merged.general.log_level = other.general.log_level.clone();
        }
        if other.general.default_gcp_project.is_some() {
            merged.general.default_gcp_project = other.general.default_gcp_project.clone();
        }
        // auto_update defaults true; only an explicit false in a higher layer wins.
        merged.general.auto_update &= other.general.auto_update;
        if other.schema_version > merged.schema_version {
            merged.schema_version = other.schema_version;
        }

        // MCP servers: higher-priority entries override by name
        for (name, server) in &other.mcp_servers {
            merged.mcp_servers.insert(name.clone(), server.clone());
        }

        // Global permissions
        for (k, v) in &other.permission {
            merged.permission.insert(k.clone(), *v);
        }

        // Custom commands
        for (k, v) in &other.commands {
            merged.commands.insert(k.clone(), v.clone());
        }

        // Security: a layer that set a non-default field overrides the one below.
        if other.security.bash_risk_threshold != default_risk_threshold() {
            merged.security.bash_risk_threshold = other.security.bash_risk_threshold.clone();
        }
        if other.security.sandbox != default_sandbox_mode() {
            merged.security.sandbox = other.security.sandbox.clone();
        }
        if !other.security.sandbox_network {
            merged.security.sandbox_network = false;
        }
        if other.security.sandbox_fallback != default_sandbox_fallback() {
            merged.security.sandbox_fallback = other.security.sandbox_fallback.clone();
        }
        // Network lists: non-empty higher layer replaces (project can restrict).
        if !other.security.network_allowlist.is_empty() {
            merged.security.network_allowlist = other.security.network_allowlist.clone();
        }
        if !other.security.network_denylist.is_empty() {
            merged.security.network_denylist = other.security.network_denylist.clone();
        }

        // Hooks: non-empty higher layer replaces (project can define its own set).
        if !other.hooks.is_empty() {
            merged.hooks = other.hooks.clone();
        }

        // Memory: higher layer wins on explicit non-default knobs; enabled can
        // only be turned off by a higher layer (default is on).
        if !other.memory.enabled {
            merged.memory.enabled = false;
        }
        if !other.memory.auto_inject {
            merged.memory.auto_inject = false;
        }
        if !other.memory.auto_retain {
            merged.memory.auto_retain = false;
        }
        if !other.memory.retain_llm {
            merged.memory.retain_llm = false;
        }
        if other.memory.retain_llm_always {
            merged.memory.retain_llm_always = true;
        }
        if other.memory.retain_every_n != default_retain_every_n() {
            merged.memory.retain_every_n = other.memory.retain_every_n;
        }
        if other.memory.retain_max_facts != default_retain_max_facts() {
            merged.memory.retain_max_facts = other.memory.retain_max_facts;
        }
        if other.memory.max_index_lines != default_memory_index_lines() {
            merged.memory.max_index_lines = other.memory.max_index_lines;
        }
        if other.memory.max_index_bytes != default_memory_index_bytes() {
            merged.memory.max_index_bytes = other.memory.max_index_bytes;
        }
        if other.memory.recall_top_k != default_memory_top_k() {
            merged.memory.recall_top_k = other.memory.recall_top_k;
        }
        if (other.memory.recall_min_score - default_memory_min_score()).abs() > f32::EPSILON {
            merged.memory.recall_min_score = other.memory.recall_min_score;
        }
        if other.memory.recall_token_budget != default_memory_token_budget() {
            merged.memory.recall_token_budget = other.memory.recall_token_budget;
        }
        if other.memory.embed_dim != default_memory_embed_dim() {
            merged.memory.embed_dim = other.memory.embed_dim;
        }
        if other.memory.scope != default_memory_scope() {
            merged.memory.scope = other.memory.scope.clone();
        }
        if other.memory.embed_backend != default_memory_backend() {
            merged.memory.embed_backend = other.memory.embed_backend.clone();
        }
        if !other.memory.code_inject {
            merged.memory.code_inject = false;
        }
        if other.memory.code_top_k != default_code_top_k() {
            merged.memory.code_top_k = other.memory.code_top_k;
        }
        if (other.memory.code_min_score - default_code_min_score()).abs() > f32::EPSILON {
            merged.memory.code_min_score = other.memory.code_min_score;
        }
        if !other.memory.subagent_banks {
            merged.memory.subagent_banks = false;
        }
        if !other.memory.auto_index {
            merged.memory.auto_index = false;
        }
        if other.memory.auto_index_max_files != default_auto_index_files() {
            merged.memory.auto_index_max_files = other.memory.auto_index_max_files;
        }
        if other.memory.auto_index_max_chunks != default_auto_index_chunks() {
            merged.memory.auto_index_max_chunks = other.memory.auto_index_max_chunks;
        }
        if !other.memory.session_inject {
            merged.memory.session_inject = false;
        }
        if other.memory.session_top_k != default_session_top_k() {
            merged.memory.session_top_k = other.memory.session_top_k;
        }
        if (other.memory.session_min_score - default_session_min_score()).abs() > f32::EPSILON {
            merged.memory.session_min_score = other.memory.session_min_score;
        }
        if !other.memory.consolidate {
            merged.memory.consolidate = false;
        }
        if other.memory.consolidate_max != default_consolidate_max() {
            merged.memory.consolidate_max = other.memory.consolidate_max;
        }

        // Swarm: higher layer can disable; max_agents overrides when non-default.
        if !other.swarm.enabled {
            merged.swarm.enabled = false;
        }
        if other.swarm.max_agents != default_swarm_max_agents() {
            merged.swarm.max_agents = other.swarm.max_agents;
        }
        if !other.swarm.worktrees {
            merged.swarm.worktrees = false;
        }
        if other.swarm.isolation.is_some() {
            merged.swarm.isolation = other.swarm.isolation.clone();
        }

        if other.automation.max_background_jobs != default_max_background_jobs() {
            merged.automation.max_background_jobs = other.automation.max_background_jobs;
        }

        merged.notify = self.notify.merge_with(&other.notify);

        merged
    }

    /// Merge global `permission` map into an agent's PermissionSet (agent rules win).
    pub fn effective_permission(&self, agent: &PermissionSet) -> PermissionSet {
        let mut out = agent.clone();
        for (k, v) in &self.permission {
            out.rules.entry(k.clone()).or_insert(*v);
        }
        // agent rules already on out.rules take precedence (entry::or_insert)
        // but agent may have set rules that should win — re-apply agent rules last
        for (k, v) in &agent.rules {
            out.rules.insert(k.clone(), *v);
        }
        out
    }
}

// ── ToolsConfig merging helper ────────────────────────────────────────

impl ToolsConfig {
    pub(crate) fn merge_with(&self, other: &ToolsConfig) -> ToolsConfig {
        let mut merged = self.clone();

        // Boolean flags: if other was explicitly deserialized (non-default),
        // the value from other takes priority. Since we can't determine
        // "was it explicitly set?" without a sentinel, we use a heuristic:
        // if other differs from the default, take other's value.
        let defaults = ToolsConfig::default();
        if other.enable_read != defaults.enable_read {
            merged.enable_read = other.enable_read;
        }
        if other.enable_write != defaults.enable_write {
            merged.enable_write = other.enable_write;
        }
        if other.enable_edit != defaults.enable_edit {
            merged.enable_edit = other.enable_edit;
        }
        if other.enable_glob != defaults.enable_glob {
            merged.enable_glob = other.enable_glob;
        }
        if other.enable_grep != defaults.enable_grep {
            merged.enable_grep = other.enable_grep;
        }
        if other.enable_shell != defaults.enable_shell {
            merged.enable_shell = other.enable_shell;
        }
        if other.enable_webfetch != defaults.enable_webfetch {
            merged.enable_webfetch = other.enable_webfetch;
        }
        if other.enable_websearch != defaults.enable_websearch {
            merged.enable_websearch = other.enable_websearch;
        }

        let qdef = QuestionToolConfig::default();
        if other.question.timeout_enabled != qdef.timeout_enabled {
            merged.question.timeout_enabled = other.question.timeout_enabled;
        }
        if other.question.timeout_secs != qdef.timeout_secs {
            merged.question.timeout_secs = other.question.timeout_secs;
        }

        if !other.disabled_tools.is_empty() {
            merged.disabled_tools = other.disabled_tools.clone();
        }
        for (name, tool) in &other.custom_tools {
            merged
                .custom_tools
                .entry(name.clone())
                .or_insert_with(|| tool.clone());
        }

        merged
    }
}
