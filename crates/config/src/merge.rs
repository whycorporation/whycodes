//! Layered config merge.

use crate::types::{
    BashConfig, Config, MagicKeywordsConfig, QuestionToolConfig, SlopConfig, ToolsConfig,
    default_agent, default_auto_background_after_secs, default_auto_index_chunks,
    default_auto_index_files, default_code_min_score, default_code_top_k, default_compaction_llm,
    default_compaction_threshold, default_consolidate_max, default_intent_guidance,
    default_max_background_jobs, default_max_tokens, default_memory_backend,
    default_memory_embed_dim, default_memory_index_bytes, default_memory_index_lines,
    default_memory_min_score, default_memory_scope, default_memory_token_budget,
    default_memory_top_k, default_model_race, default_prompt_cache, default_race_after_ms,
    default_response_cache, default_retain_every_n, default_retain_max_facts,
    default_risk_threshold, default_sandbox_fallback, default_sandbox_mode,
    default_session_min_score, default_session_top_k, default_swarm_max_agents,
    default_tool_profile,
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
        merged.merge_providers(other);
        merged.merge_models(other);
        merged.merge_agents(other);
        merged.merge_defaults(other);
        merged.merge_command_configs(other);
        merged.tools = self.tools.merge_with(&other.tools);
        merged.merge_session(other);
        merged.merge_tui(other);
        merged.merge_general(other);
        merged.merge_servers(other);
        merged.merge_maps(other);
        merged.merge_security(other);
        merged.merge_hooks(other);
        merged.merge_memory(other);
        merged.merge_memory_index(other);
        merged.merge_swarm(other);
        merged.merge_tail(other);
        merged
    }

    fn merge_providers(&mut self, other: &Config) {
        // Providers: merge per-provider
        for (key, provider) in &other.providers {
            self.providers
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
                    if !provider.credentials.is_empty() {
                        existing.credentials = provider.credentials.clone();
                    }
                })
                .or_insert_with(|| provider.clone());
        }
    }

    fn merge_models(&mut self, other: &Config) {
        // Models: merge per model_id+provider_id key
        for (key, model) in &other.models {
            self.models
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
    }

    fn merge_agents(&mut self, other: &Config) {
        // Agents: append unique (by name)
        for agent in &other.agents {
            if !self.agents.iter().any(|a| a.name == agent.name) {
                self.agents.push(agent.clone());
            }
        }
    }

    fn merge_defaults(&mut self, other: &Config) {
        // Default agent (non-empty override wins)
        if other.default_agent != default_agent() || !other.default_agent.is_empty() {
            // only override if explicitly set (not the default value from
            // deserialization). Since we can't tell, always take other's value
            // when merging (other is the higher-priority layer).
            self.default_agent = other.default_agent.clone();
        }

        // Default model
        if other.default_model.is_some() {
            self.default_model = other.default_model.clone();
        }
    }

    fn merge_command_configs(&mut self, other: &Config) {
        // Command configs: merge per-command
        for (cmd, cmd_cfg) in &other.command_configs {
            self.command_configs
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
    }

    fn merge_session(&mut self, other: &Config) {
        // Session
        if other.session.max_context_tokens != default_max_tokens() {
            self.session.max_context_tokens = other.session.max_context_tokens;
        }
        if other.session.compaction_threshold != default_compaction_threshold() {
            self.session.compaction_threshold = other.session.compaction_threshold;
        }
        if other.session.store_path.is_some() {
            self.session.store_path = other.session.store_path.clone();
        }
        // auto_title defaults true; only an explicit false in a higher layer wins
        // when the lower layer left the default — merge by taking `other` always
        // for bools that appear in TOML (serde always deserializes them).
        if !other.session.auto_title {
            self.session.auto_title = false;
        }
        if other.session.title_model.is_some() {
            self.session.title_model = other.session.title_model.clone();
        }
        if other.session.tool_profile != default_tool_profile() {
            self.session.tool_profile = other.session.tool_profile.clone();
        }
        if other.session.prompt_cache != default_prompt_cache() {
            self.session.prompt_cache = other.session.prompt_cache.clone();
        }
        if other.session.compaction_llm != default_compaction_llm() {
            self.session.compaction_llm = other.session.compaction_llm.clone();
        }
        if other.session.reasoning_effort.is_some() {
            self.session.reasoning_effort = other.session.reasoning_effort.clone();
        }
        if other.session.model_fast.is_some() {
            self.session.model_fast = other.session.model_fast.clone();
        }
        if other.session.model_smol.is_some() {
            self.session.model_smol = other.session.model_smol.clone();
        }
        if other.session.model_plan.is_some() {
            self.session.model_plan = other.session.model_plan.clone();
        }
        if !other.session.stream_rules.is_empty() {
            self.session.stream_rules = other.session.stream_rules.clone();
        }
        if other.session.model_race != default_model_race() {
            self.session.model_race = other.session.model_race.clone();
        }
        if other.session.race_after_ms != default_race_after_ms() {
            self.session.race_after_ms = other.session.race_after_ms;
        }
        if other.session.response_cache != default_response_cache() {
            self.session.response_cache = other.session.response_cache.clone();
        }
        if other.session.intent_guidance != default_intent_guidance() {
            self.session.intent_guidance = other.session.intent_guidance.clone();
        }
        if other.session.magic_keywords != MagicKeywordsConfig::default() {
            self.session.magic_keywords = other.session.magic_keywords.clone();
        }
        if other.session.headless_ask.is_some() {
            self.session.headless_ask = other.session.headless_ask;
        }
    }

    fn merge_tui(&mut self, other: &Config) {
        // TUI
        if other.tui.theme.is_some() {
            self.tui.theme = other.tui.theme.clone();
        }
        if other.tui.key_bindings.is_some() {
            self.tui.key_bindings = other.tui.key_bindings.clone();
        }
        // Higher layer can only turn the sidebar on (default is off).
        self.tui.show_sidebar |= other.tui.show_sidebar;
        self.tui.skip_openrouter_key_prompt |= other.tui.skip_openrouter_key_prompt;
        for (name, spec) in &other.tui.agent_colors {
            self.tui.agent_colors.insert(name.clone(), spec.clone());
        }
    }

    fn merge_general(&mut self, other: &Config) {
        // General
        if other.general.project_path.is_some() {
            self.general.project_path = other.general.project_path.clone();
        }
        if other.general.log_level.is_some() {
            self.general.log_level = other.general.log_level.clone();
        }
        if other.general.default_gcp_project.is_some() {
            self.general.default_gcp_project = other.general.default_gcp_project.clone();
        }
        // auto_update defaults true; only an explicit false in a higher layer wins.
        self.general.auto_update &= other.general.auto_update;
        if other.general.approval_mode.is_some() {
            self.general.approval_mode = other.general.approval_mode;
        }
        if other.schema_version > self.schema_version {
            self.schema_version = other.schema_version;
        }
    }

    fn merge_servers(&mut self, other: &Config) {
        // MCP servers: higher-priority entries override by name
        for (name, server) in &other.mcp_servers {
            self.mcp_servers.insert(name.clone(), server.clone());
        }

        // LSP overlay: higher-priority idle timeout wins; servers overlay by name.
        if other.lsp.idle_timeout_ms.is_some() {
            self.lsp.idle_timeout_ms = other.lsp.idle_timeout_ms;
        }
        for (name, server) in &other.lsp.servers {
            self.lsp.servers.insert(name.clone(), server.clone());
        }
    }

    fn merge_maps(&mut self, other: &Config) {
        // Global permissions
        for (k, v) in &other.permission {
            self.permission.insert(k.clone(), *v);
        }

        // Custom commands
        for (k, v) in &other.commands {
            self.commands.insert(k.clone(), v.clone());
        }
    }

    fn merge_security(&mut self, other: &Config) {
        // Security: a layer that set a non-default field overrides the one below.
        if other.security.bash_risk_threshold != default_risk_threshold() {
            self.security.bash_risk_threshold = other.security.bash_risk_threshold.clone();
        }
        if other.security.sandbox != default_sandbox_mode() {
            self.security.sandbox = other.security.sandbox.clone();
        }
        if !other.security.sandbox_network {
            self.security.sandbox_network = false;
        }
        if other.security.sandbox_fallback != default_sandbox_fallback() {
            self.security.sandbox_fallback = other.security.sandbox_fallback.clone();
        }
        // Network lists: non-empty higher layer replaces (project can restrict).
        if !other.security.network_allowlist.is_empty() {
            self.security.network_allowlist = other.security.network_allowlist.clone();
        }
        if !other.security.network_denylist.is_empty() {
            self.security.network_denylist = other.security.network_denylist.clone();
        }
    }

    fn merge_hooks(&mut self, other: &Config) {
        // Hooks: non-empty higher layer replaces (project can define its own set).
        if !other.hooks.is_empty() {
            self.hooks = other.hooks.clone();
        }
    }

    fn merge_memory(&mut self, other: &Config) {
        // Memory: higher layer wins on explicit non-default knobs; enabled can
        // only be turned off by a higher layer (default is on).
        if !other.memory.enabled {
            self.memory.enabled = false;
        }
        if !other.memory.auto_inject {
            self.memory.auto_inject = false;
        }
        if !other.memory.auto_retain {
            self.memory.auto_retain = false;
        }
        if !other.memory.retain_llm {
            self.memory.retain_llm = false;
        }
        if other.memory.retain_llm_always {
            self.memory.retain_llm_always = true;
        }
        if other.memory.retain_every_n != default_retain_every_n() {
            self.memory.retain_every_n = other.memory.retain_every_n;
        }
        if other.memory.retain_max_facts != default_retain_max_facts() {
            self.memory.retain_max_facts = other.memory.retain_max_facts;
        }
        if other.memory.max_index_lines != default_memory_index_lines() {
            self.memory.max_index_lines = other.memory.max_index_lines;
        }
        if other.memory.max_index_bytes != default_memory_index_bytes() {
            self.memory.max_index_bytes = other.memory.max_index_bytes;
        }
        if other.memory.recall_top_k != default_memory_top_k() {
            self.memory.recall_top_k = other.memory.recall_top_k;
        }
        if (other.memory.recall_min_score - default_memory_min_score()).abs() > f32::EPSILON {
            self.memory.recall_min_score = other.memory.recall_min_score;
        }
        if other.memory.recall_token_budget != default_memory_token_budget() {
            self.memory.recall_token_budget = other.memory.recall_token_budget;
        }
        if other.memory.embed_dim != default_memory_embed_dim() {
            self.memory.embed_dim = other.memory.embed_dim;
        }
        if other.memory.scope != default_memory_scope() {
            self.memory.scope = other.memory.scope.clone();
        }
        if other.memory.embed_backend != default_memory_backend() {
            self.memory.embed_backend = other.memory.embed_backend.clone();
        }
    }

    fn merge_memory_index(&mut self, other: &Config) {
        if !other.memory.code_inject {
            self.memory.code_inject = false;
        }
        if other.memory.code_top_k != default_code_top_k() {
            self.memory.code_top_k = other.memory.code_top_k;
        }
        if (other.memory.code_min_score - default_code_min_score()).abs() > f32::EPSILON {
            self.memory.code_min_score = other.memory.code_min_score;
        }
        if !other.memory.subagent_banks {
            self.memory.subagent_banks = false;
        }
        if !other.memory.auto_index {
            self.memory.auto_index = false;
        }
        if other.memory.auto_index_max_files != default_auto_index_files() {
            self.memory.auto_index_max_files = other.memory.auto_index_max_files;
        }
        if other.memory.auto_index_max_chunks != default_auto_index_chunks() {
            self.memory.auto_index_max_chunks = other.memory.auto_index_max_chunks;
        }
        if !other.memory.session_inject {
            self.memory.session_inject = false;
        }
        if other.memory.session_top_k != default_session_top_k() {
            self.memory.session_top_k = other.memory.session_top_k;
        }
        if (other.memory.session_min_score - default_session_min_score()).abs() > f32::EPSILON {
            self.memory.session_min_score = other.memory.session_min_score;
        }
        if !other.memory.consolidate {
            self.memory.consolidate = false;
        }
        if other.memory.consolidate_max != default_consolidate_max() {
            self.memory.consolidate_max = other.memory.consolidate_max;
        }
    }

    fn merge_swarm(&mut self, other: &Config) {
        // Swarm: higher layer can disable; max_agents overrides when non-default.
        if !other.swarm.enabled {
            self.swarm.enabled = false;
        }
        if other.swarm.max_agents != default_swarm_max_agents() {
            self.swarm.max_agents = other.swarm.max_agents;
        }
        if !other.swarm.worktrees {
            self.swarm.worktrees = false;
        }
        if other.swarm.isolation.is_some() {
            self.swarm.isolation = other.swarm.isolation.clone();
        }
    }

    fn merge_tail(&mut self, other: &Config) {
        if other.automation.max_background_jobs != default_max_background_jobs() {
            self.automation.max_background_jobs = other.automation.max_background_jobs;
        }

        if other.slop != SlopConfig::default() {
            self.slop = self.slop.merge_with(&other.slop);
        }

        self.notify = self.notify.merge_with(&other.notify);

        for (k, v) in &other.system_prompt_overlays.providers {
            self.system_prompt_overlays
                .providers
                .insert(k.clone(), v.clone());
        }
        for (k, v) in &other.system_prompt_overlays.models {
            self.system_prompt_overlays
                .models
                .insert(k.clone(), v.clone());
        }
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

        let bdef = BashConfig::default();
        if other.bash.auto_background != bdef.auto_background {
            merged.bash.auto_background = other.bash.auto_background;
        }
        if other.bash.auto_background_after_secs != default_auto_background_after_secs() {
            merged.bash.auto_background_after_secs = other.bash.auto_background_after_secs;
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

#[cfg(test)]
mod tests {
    use crate::types::Config;

    #[test]
    fn merge_overrides_default_agent_and_model() {
        let base = Config::default();
        let other = Config {
            default_agent: "review".into(),
            default_model: Some(whycodes_core::types::ModelConfig {
                model_id: "m".into(),
                provider_id: "p".into(),
                max_tokens: Some(1),
                context_window: None,
                temperature: None,
                top_p: None,
                thinking: None,
                supports_tools: None,
                supports_images: None,
            }),
            ..Config::default()
        };
        let merged = base.merge_with(&other);
        assert_eq!(merged.default_agent, "review");
        assert_eq!(merged.default_model.as_ref().unwrap().model_id, "m");
    }
}
