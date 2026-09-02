//! Load, save, env overlay, and custom-command markdown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::types::{
    CONFIG_SCHEMA_VERSION, CommandConfig, Config, CustomCommandConfig, nonempty_opt,
    parse_notify_on_csv,
};
use whycodes_core::network;
use whycodes_core::types::{AgentInfo, ModelConfig, ProviderConfig};
use whycodes_core::{Error, Result};

pub(crate) fn toml_err(msg: String) -> Error {
    Error::Config(msg)
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub(crate) fn map_toml_ser(r: std::result::Result<String, toml::ser::Error>) -> Result<String> {
    match r {
        Ok(s) => Ok(s),
        Err(e) => Err(toml_err(e.to_string())),
    }
}

pub(crate) fn encode_toml(cfg: &Config) -> Result<String> {
    map_toml_ser(toml::to_string_pretty(cfg))
}

fn warn_project_config(kind: &str, path: &Path, e: impl std::fmt::Display) {
    let msg = format!("Failed to {kind} project config at {}: {e}", path.display());
    tracing::warn!("{msg}");
}

impl Config {
    // ── Loading / Saving ────────────────────────────────────────────────

    /// Load config from the default location
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        if !path.exists() {
            let cfg = Self::default();
            return Ok(cfg);
        }
        let content = std::fs::read_to_string(&path)?;
        let mut cfg: Config = toml::from_str(&content).map_err(|e| toml_err(e.to_string()))?;
        // When a table is keyed `[providers.foo]` but omits `name`, use the key.
        for (key, provider) in &mut cfg.providers {
            if provider.name.is_empty() {
                provider.name = key.clone();
            }
        }
        cfg.expand_notify_secrets();
        cfg.migrate_schema()?;
        Ok(cfg)
    }

    /// Rewrite an older `config.toml` in place so later loads see the current
    /// schema. Missing `schema_version` is treated as `0`.
    pub fn migrate_schema(&mut self) -> Result<bool> {
        if self.schema_version >= CONFIG_SCHEMA_VERSION {
            return Ok(false);
        }
        self.schema_version = CONFIG_SCHEMA_VERSION;
        // Best-effort: a read-only home should still run with the in-memory
        // defaults for new fields. The next writable save persists the bump.
        match self.save() {
            Ok(()) => Ok(true),
            Err(e) => {
                tracing::warn!("config schema migrate could not rewrite config.toml: {e}");
                Ok(true)
            }
        }
    }

    /// Save config to the default location
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path()?;
        ensure_parent_dir(&path)?;
        let content = encode_toml(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get default config path
    pub fn default_path() -> Result<PathBuf> {
        Ok(whycodes_core::paths::config_file())
    }

    /// Get data directory for sessions, caches, etc.
    pub fn data_dir() -> Result<PathBuf> {
        Ok(whycodes_core::paths::data_dir())
    }

    // ── Layered config loading ──────────────────────────────────────────

    /// Load config using priority-based layering:
    /// 1. Built-in defaults
    /// 2. Global config (~/.config/whycodes/config.toml)
    /// 3. Project config (`<project>/.whycodes/config.toml`)
    /// 4. Environment variables (WHYCODES_*)
    ///
    /// Returns the merged configuration.
    pub fn load_layered(project_dir: &Path) -> Result<Self> {
        let mut config = Self::default();

        // Layer 2: global config
        match Self::load() {
            Ok(global) => {
                config = config.merge_with(&global);
            }
            Err(e) => {
                // Don't silently drop a broken config.toml — that falls back to
                // hard-coded anthropic and looks like "default model ignored".
                tracing::error!("Failed to load global config: {e}");
                return Err(e);
            }
        }

        // Layer 3: project config
        let project_config_path = whycodes_core::project_dir(project_dir).join("config.toml");
        if project_config_path.exists() {
            match std::fs::read_to_string(&project_config_path) {
                Ok(content) => match toml::from_str::<Config>(&content) {
                    Ok(mut project) => {
                        if project.schema_version < CONFIG_SCHEMA_VERSION {
                            project.schema_version = CONFIG_SCHEMA_VERSION;
                        }
                        config = config.merge_with(&project);
                    }
                    Err(e) => warn_project_config("parse", &project_config_path, e),
                },
                Err(e) => warn_project_config("read", &project_config_path, e),
            }
        }

        // Layer 4: environment variables
        config.apply_env_overrides();
        config.expand_notify_secrets();

        Ok(config)
    }

    /// Apply environment variable overrides to this config in-place.
    pub fn apply_env_overrides(&mut self) {
        // WHYCODES_PROVIDER — set as the default provider (first entry) and
        // also populate a basic ProviderConfig if one doesn't exist.
        if let Ok(provider_name) = std::env::var("WHYCODES_PROVIDER") {
            if !self.providers.contains_key(&provider_name) {
                // Auto-create a basic provider entry
                let api_key = std::env::var(format!("{}_API_KEY", provider_name.to_uppercase()))
                    .ok()
                    .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                    .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());

                self.providers.insert(
                    provider_name.clone(),
                    ProviderConfig {
                        name: provider_name.clone(),
                        api_key,
                        api_base: None,
                        base_url: None,
                        headers: None,
                        models: Vec::new(),
                        tool_arguments: None,
                        extra: HashMap::new(),
                    },
                );
            }

            // If no default model is set, try to pick it up from WHYCODES_MODEL
            if self.default_model.is_none()
                && let Ok(model_name) = std::env::var("WHYCODES_MODEL")
            {
                self.default_model = Some(ModelConfig {
                    model_id: model_name,
                    provider_id: provider_name,
                    max_tokens: None,
                    context_window: None,
                    temperature: None,
                    top_p: None,
                    thinking: None,
                    supports_tools: None,
                    supports_images: None,
                });
            }
        } else if let Ok(model_name) = std::env::var("WHYCODES_MODEL") {
            // Model set without provider — update default model's model_id if
            // one exists, otherwise create a minimal entry.
            if let Some(ref mut default_model) = self.default_model {
                default_model.model_id = model_name;
            } else {
                self.default_model = Some(ModelConfig {
                    model_id: model_name,
                    provider_id: String::new(),
                    max_tokens: None,
                    context_window: None,
                    temperature: None,
                    top_p: None,
                    thinking: None,
                    supports_tools: None,
                    supports_images: None,
                });
            }
        }

        // WHYCODES_MAX_TURNS
        if let Ok(val) = std::env::var("WHYCODES_MAX_TURNS")
            && let Ok(n) = val.parse::<usize>()
        {
            self.session.max_context_tokens = n;
        }

        // WHYCODES_LOG_LEVEL
        if let Ok(val) = std::env::var("WHYCODES_LOG_LEVEL") {
            self.general.log_level = Some(val);
        }

        // WHYCODES_PROJECT_DIR
        if let Ok(val) = std::env::var("WHYCODES_PROJECT_DIR") {
            self.general.project_path = Some(PathBuf::from(val));
        }

        if let Ok(val) = std::env::var("WHYCODES_SANDBOX") {
            self.security.sandbox = val;
        }
        if let Ok(val) = std::env::var("WHYCODES_SANDBOX_NETWORK") {
            self.security.sandbox_network = matches!(
                val.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
        if let Ok(val) = std::env::var("WHYCODES_SANDBOX_FALLBACK") {
            self.security.sandbox_fallback = val;
        }
        if let Ok(val) = std::env::var("WHYCODES_NETWORK_ALLOWLIST") {
            self.security.network_allowlist = network::parse_domain_list(&val);
        }
        if let Ok(val) = std::env::var("WHYCODES_NETWORK_DENYLIST") {
            self.security.network_denylist = network::parse_domain_list(&val);
        }

        // WHYCODES_NO_MEMORY=1 disables cross-session memory inject/write.
        if let Ok(val) = std::env::var("WHYCODES_NO_MEMORY")
            && matches!(
                val.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        {
            self.memory.enabled = false;
        }
        if let Ok(val) = std::env::var("WHYCODES_NO_AUTO_UPDATE")
            && matches!(
                val.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        {
            self.general.auto_update = false;
        }
        if let Ok(val) = std::env::var("WHYCODES_AUTO_UPDATE") {
            match val.to_ascii_lowercase().as_str() {
                "0" | "false" | "no" | "off" => self.general.auto_update = false,
                "1" | "true" | "yes" | "on" => self.general.auto_update = true,
                _ => {}
            }
        }
        if let Ok(val) = std::env::var("WHYCODES_APPROVAL_MODE")
            && let Some(mode) = whycodes_core::types::ApprovalMode::parse(&val)
        {
            self.general.approval_mode = Some(mode);
        }
        if let Ok(val) = std::env::var("WHYCODES_MEMORY") {
            match val.to_ascii_lowercase().as_str() {
                "0" | "false" | "no" | "off" => self.memory.enabled = false,
                "1" | "true" | "yes" | "on" => self.memory.enabled = true,
                _ => {}
            }
        }

        // WHYCODES_SWARM=0/1 toggles parallel multi-agent.
        if let Ok(val) = std::env::var("WHYCODES_SWARM") {
            match val.to_ascii_lowercase().as_str() {
                "0" | "false" | "no" | "off" => self.swarm.enabled = false,
                "1" | "true" | "yes" | "on" => self.swarm.enabled = true,
                _ => {}
            }
        }
        if let Ok(val) = std::env::var("WHYCODES_SWARM_MAX_AGENTS")
            && let Ok(n) = val.parse::<usize>()
        {
            self.swarm.max_agents = n.clamp(1, 8);
        }
        if let Ok(val) = std::env::var("WHYCODES_SWARM_WORKTREES") {
            match val.to_ascii_lowercase().as_str() {
                "0" | "false" | "no" | "off" => self.swarm.worktrees = false,
                "1" | "true" | "yes" | "on" => self.swarm.worktrees = true,
                _ => {}
            }
        }

        if let Ok(val) = std::env::var("WHYCODES_NOTIFY_ON") {
            self.notify.on = parse_notify_on_csv(&val);
        }
        if let Ok(val) = std::env::var("WHYCODES_NOTIFY_DISCORD_WEBHOOK") {
            self.notify.discord_webhook = Some(val);
        }
        if let Ok(val) = std::env::var("WHYCODES_NOTIFY_TELEGRAM_BOT_TOKEN") {
            self.notify.telegram_bot_token = Some(val);
        }
        if let Ok(val) = std::env::var("WHYCODES_NOTIFY_TELEGRAM_CHAT_ID") {
            self.notify.telegram_chat_id = Some(val);
        }
        if let Ok(val) = std::env::var("WHYCODES_NOTIFY_TIMEOUT_SECS")
            && let Ok(n) = val.parse::<u64>()
        {
            self.notify.timeout_secs = n.clamp(1, 60);
        }
    }

    /// Load custom commands from markdown files (OpenCode paths adapted):
    /// - global: `~/.config/.../commands/*.md`
    /// - project: `<project>/.whycodes/commands/*.md`
    pub fn load_command_files(&mut self, project_dir: &Path) {
        if let Ok(global_dir) = Self::default_path()
            && let Some(parent) = global_dir.parent()
        {
            load_commands_from_dir(&mut self.commands, &parent.join("commands"));
        }
        load_commands_from_dir(
            &mut self.commands,
            &whycodes_core::project_dir(project_dir).join("commands"),
        );
        // also accept OpenCode-style .opencode/commands
        load_commands_from_dir(
            &mut self.commands,
            &project_dir.join(".opencode").join("commands"),
        );
        // Built-ins last only for missing keys — user/project markdown wins.
        self.ensure_builtin_prompt_commands();
    }

    /// Claude Code–style PromptCommands: fixed prompts that kick a turn.
    /// Does not overwrite user-defined `commands.*` or `commands/*.md`.
    pub fn ensure_builtin_prompt_commands(&mut self) {
        for (name, cmd) in builtin_prompt_commands() {
            self.commands.entry(name.to_string()).or_insert(cmd);
        }
    }
}

/// Built-in workflow slash prompts (A1). Keys without leading `/`.
fn builtin_prompt_commands() -> Vec<(&'static str, CustomCommandConfig)> {
    vec![
        (
            "review",
            CustomCommandConfig {
                description: Some("AI code review of git changes".into()),
                agent: Some("plan".into()),
                model: None,
                subtask: None,
                template: r#"Review the current working tree changes for this project.

1. Run git_status and git_diff (unstaged and staged) to see what changed.
2. Read only the files that matter for the diff.
3. Produce a structured review:
   - Summary (2–4 sentences)
   - Strengths
   - Issues (severity: high / medium / low) with file:line when possible
   - Suggested follow-ups
4. Do NOT write or edit files unless the user explicitly asked to apply fixes.
5. Prefer read-only tools (git_*, read, grep, glob, list).

Extra context from the user: $ARGUMENTS"#
                    .into(),
            },
        ),
        (
            "security-review",
            CustomCommandConfig {
                description: Some("Security-focused review of changes".into()),
                agent: Some("plan".into()),
                model: None,
                subtask: None,
                template: r#"Perform a security-focused review of the current git changes.

1. Inspect git_status / git_diff and relevant files.
2. Look for: secrets/credentials, injection (SQL/command/path), authz gaps,
   unsafe shell, SSRF, insecure deserialization, dependency risks, XSS.
3. Output:
   - Executive summary
   - Findings table: severity | location | issue | remediation
   - What looks safe
4. Do not modify files unless asked. Prefer read-only tools.

Focus / notes: $ARGUMENTS"#
                    .into(),
            },
        ),
        (
            "commit",
            CustomCommandConfig {
                description: Some("Draft a git commit message from the diff".into()),
                agent: None,
                model: None,
                subtask: None,
                template: r#"Prepare a git commit for the current changes.

1. Run git_status and git_diff (include staged if any).
2. Draft a concise commit message (subject ≤72 chars; body if needed).
3. If the user asked to commit (or says "commit now" / "yes"), stage relevant
   files and use git_commit. Otherwise show the proposed message and ask.
4. Never force-push, never amend published history, never commit secrets (.env, keys).

User notes: $ARGUMENTS"#
                    .into(),
            },
        ),
    ]
}

fn load_commands_from_dir(into: &mut HashMap<String, CustomCommandConfig>, dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("cmd")
            .to_string();
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Some(cmd) = parse_command_markdown(&content)
        {
            into.insert(name, cmd);
        }
    }
}

/// Parse OpenCode-style command markdown with YAML frontmatter.
pub(crate) fn parse_command_markdown(content: &str) -> Option<CustomCommandConfig> {
    let content = content.trim();
    if !content.starts_with("---") {
        return Some(CustomCommandConfig {
            template: content.to_string(),
            description: None,
            agent: None,
            model: None,
            subtask: None,
        });
    }
    let (front, body) = content.strip_prefix("---")?.split_once("---")?;
    let front = front.trim();
    let body = body.trim().to_string();

    let mut description = None;
    let mut agent = None;
    let mut model = None;
    let mut subtask = None;
    for line in front.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            match k {
                "description" => description = Some(v),
                "agent" => agent = Some(v),
                "model" => model = Some(v),
                "subtask" => {
                    subtask = Some(matches!(
                        v.to_ascii_lowercase().as_str(),
                        "true" | "yes" | "1"
                    ))
                }
                _ => {}
            }
        }
    }

    Some(CustomCommandConfig {
        template: body,
        description,
        agent,
        model,
        subtask,
    })
}

impl Config {
    /// Get per-command configuration overrides for the given command name.
    pub fn get_command_config(&self, command: &str) -> Option<&CommandConfig> {
        self.command_configs.get(command)
    }

    /// Expand `${VAR}` / `$VAR` in Discord / Telegram secret fields.
    ///
    /// Call after layered load so `config.toml` can store
    /// `discord_webhook = "${DISCORD_WEBHOOK_URL}"` without committing the URL.
    pub fn expand_notify_secrets(&mut self) {
        if let Some(url) = self.notify.discord_webhook.take() {
            let expanded = Self::substitute_vars(&url);
            self.notify.discord_webhook = nonempty_opt(expanded);
        }
        if let Some(token) = self.notify.telegram_bot_token.take() {
            let expanded = Self::substitute_vars(&token);
            self.notify.telegram_bot_token = nonempty_opt(expanded);
        }
        if let Some(chat) = self.notify.telegram_chat_id.take() {
            let expanded = Self::substitute_vars(&chat);
            self.notify.telegram_chat_id = nonempty_opt(expanded);
        }
    }

    // ── Variable substitution ───────────────────────────────────────────

    /// Replace `${VAR_NAME}` and `$VAR_NAME` patterns with environment
    /// variable values. Unknown variables are left as-is.
    pub fn substitute_vars(value: &str) -> String {
        let mut result = String::with_capacity(value.len());
        let mut chars = value.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '$' {
                match chars.peek() {
                    Some('{') => {
                        // ${VAR_NAME} form
                        chars.next(); // consume '{'
                        let mut var_name = String::new();
                        while let Some(&c) = chars.peek() {
                            if c == '}' {
                                chars.next(); // consume '}'
                                break;
                            }
                            var_name.push(c);
                            chars.next();
                        }
                        // Replace if env var exists, otherwise leave literal
                        if let Ok(val) = std::env::var(&var_name) {
                            result.push_str(&val);
                        } else {
                            result.push_str(&format!("${{{}}}", var_name));
                        }
                    }
                    Some(c) if c.is_alphanumeric() || *c == '_' => {
                        // $VAR_NAME form
                        let mut var_name = String::new();
                        while let Some(&c) = chars.peek() {
                            if c.is_alphanumeric() || c == '_' {
                                var_name.push(c);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        if let Ok(val) = std::env::var(&var_name) {
                            result.push_str(&val);
                        } else {
                            result.push_str(&format!("${}", var_name));
                        }
                    }
                    _ => {
                        // lone '$' — pass through
                        result.push('$');
                    }
                }
            } else {
                result.push(ch);
            }
        }

        result
    }

    /// Get the configured project path or current directory
    pub fn project_path(&self) -> PathBuf {
        self.general
            .project_path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Get provider by name
    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    /// Get model config
    pub fn get_model(&self, provider: &str, model: &str) -> Option<&ModelConfig> {
        self.models
            .values()
            .find(|m| m.provider_id == provider && m.model_id == model)
    }

    /// Explicit `context_window` from config for this provider/model, if any.
    ///
    /// Does not consult the built-in catalog — call
    /// `whycodes_llm::resolve_context_window` for the full chain.
    pub fn configured_context_window(&self, provider: &str, model: &str) -> Option<u32> {
        self.get_model(provider, model)
            .and_then(|m| m.context_window)
            .or_else(|| {
                self.default_model.as_ref().and_then(|m| {
                    if m.model_id == model
                        && (m.provider_id.is_empty() || m.provider_id == provider)
                    {
                        m.context_window
                    } else {
                        None
                    }
                })
            })
    }

    /// Get agent by name
    pub fn get_agent(&self, name: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Get the default agent
    pub fn default_agent(&self) -> Option<&AgentInfo> {
        self.get_agent(&self.default_agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Config;
    use std::path::Path;

    #[test]
    fn helpers_markdown_and_parent_dir() {
        assert!(toml_err("x".into()).to_string().contains("x"));
        let cfg = Config::default();
        let encoded = encode_toml(&cfg).unwrap();
        assert!(!encoded.is_empty());
        assert!(map_toml_ser(Ok("ok".into())).unwrap() == "ok");

        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b.toml");
        ensure_parent_dir(&nested).unwrap();
        assert!(nested.parent().unwrap().is_dir());
        ensure_parent_dir(Path::new("no-parent")).unwrap();

        let plain = parse_command_markdown("echo hi").unwrap();
        assert_eq!(plain.template, "echo hi");
        assert!(plain.description.is_none());

        let with_front = parse_command_markdown(
            "---\ndescription: d\nagent: build\nmodel: gpt\nsubtask: true\nunknown: x\n---\nbody",
        )
        .unwrap();
        assert_eq!(with_front.template, "body");
        assert_eq!(with_front.description.as_deref(), Some("d"));
        assert_eq!(with_front.agent.as_deref(), Some("build"));
        assert_eq!(with_front.model.as_deref(), Some("gpt"));
        assert_eq!(with_front.subtask, Some(true));

        let yes = parse_command_markdown("---\nsubtask: yes\n---\nx").unwrap();
        assert_eq!(yes.subtask, Some(true));
        let one = parse_command_markdown("---\nsubtask: 1\n---\nx").unwrap();
        assert_eq!(one.subtask, Some(true));
        let no = parse_command_markdown("---\nsubtask: false\n---\nx").unwrap();
        assert_eq!(no.subtask, Some(false));
        assert!(parse_command_markdown("---\nno-end").is_none());

        assert!(cfg.get_command_config("missing").is_none());
    }
}
