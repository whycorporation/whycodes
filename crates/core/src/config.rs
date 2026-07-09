use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::types::{AgentInfo, ModelConfig, ProviderConfig};

/// Per-command configuration overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommandConfig {
    /// Override the model for this command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelConfig>,

    /// Override the agent for this command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,

    /// Override the max turns for this command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// API provider configurations
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Model configurations
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,

    /// Agent definitions
    #[serde(default)]
    pub agents: Vec<AgentInfo>,

    /// Default agent name
    #[serde(default = "default_agent")]
    pub default_agent: String,

    /// Default model
    #[serde(default)]
    pub default_model: Option<ModelConfig>,

    /// Per-command configuration overrides
    #[serde(default)]
    pub command_configs: HashMap<String, CommandConfig>,

    /// Tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Session configuration
    #[serde(default)]
    pub session: SessionConfig,

    /// TUI configuration
    #[serde(default)]
    pub tui: TuiConfig,

    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
}

impl Default for Config {
    fn default() -> Self {
        use crate::types::{AgentInfo, AgentMode, PermissionSet};

        let build_agent = AgentInfo {
            name: "build".to_string(),
            description: "Primary coding agent with full tool access — write, edit, shell, and network.".to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
            },
            model: None,
            system_prompt: None, // Loaded from prompts/build.txt at runtime
            temperature: None,
            top_p: None,
        };

        let plan_agent = AgentInfo {
            name: "plan".to_string(),
            description: "Read-only planning agent — analyzes code and proposes changes but does not modify files or run commands.".to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: Some(vec![
                    "read".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "webfetch".to_string(),
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "shell".to_string(),
                    "apply_patch".to_string(),
                    "todo_write".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
            },
            model: None,
            system_prompt: Some("You are Whycode in planning mode. You are READ-ONLY. Analyze code and propose changes but do NOT edit files or run commands. Output a structured plan.".to_string()),
            temperature: None,
            top_p: None,
        };

        let explore_agent = AgentInfo {
            name: "explore".to_string(),
            description: "Read-only exploration agent — reads code, searches the web, understands codebases. No file modifications.".to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: Some(vec![
                    "read".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "webfetch".to_string(),
                    "websearch".to_string(),
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "shell".to_string(),
                    "apply_patch".to_string(),
                    "todo_write".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
            },
            model: None,
            system_prompt: Some("You are Whycode in exploration mode. Read code, search the web, understand codebases. No file modifications.".to_string()),
            temperature: None,
            top_p: None,
        };

        let general_agent = AgentInfo {
            name: "general".to_string(),
            description: "General-purpose subagent for complex internal searches and background tasks.".to_string(),
            mode: AgentMode::Subagent,
            permission: PermissionSet {
                allowed_tools: Some(vec![
                    "read".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "webfetch".to_string(),
                    "websearch".to_string(),
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "shell".to_string(),
                    "apply_patch".to_string(),
                    "todo_write".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
            },
            model: None,
            system_prompt: Some("You are a general-purpose subagent for complex searches and background tasks.".to_string()),
            temperature: None,
            top_p: None,
        };

        Config {
            providers: HashMap::new(),
            models: HashMap::new(),
            agents: vec![build_agent, plan_agent, explore_agent, general_agent],
            default_agent: "build".to_string(),
            default_model: None,
            command_configs: HashMap::new(),
            tools: ToolsConfig::default(),
            session: SessionConfig::default(),
            tui: TuiConfig::default(),
            general: GeneralConfig::default(),
        }
    }
}

fn default_agent() -> String {
    "build".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default = "default_true")]
    pub enable_read: bool,
    #[serde(default = "default_true")]
    pub enable_write: bool,
    #[serde(default = "default_true")]
    pub enable_edit: bool,
    #[serde(default = "default_true")]
    pub enable_glob: bool,
    #[serde(default = "default_true")]
    pub enable_grep: bool,
    #[serde(default = "default_true")]
    pub enable_shell: bool,
    #[serde(default = "default_true")]
    pub enable_webfetch: bool,
    #[serde(default = "default_true")]
    pub enable_websearch: bool,
    pub disabled_tools: Vec<String>,
    pub custom_tools: HashMap<String, CustomToolConfig>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    #[serde(default = "default_max_tokens")]
    pub max_context_tokens: usize,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: usize,
    pub store_path: Option<PathBuf>,
}

fn default_max_tokens() -> usize {
    200_000
}

fn default_compaction_threshold() -> usize {
    150_000
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TuiConfig {
    pub theme: Option<String>,
    pub key_bindings: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneralConfig {
    pub project_path: Option<PathBuf>,
    pub log_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomToolConfig {
    pub command: String,
    pub description: String,
    pub parameters: Option<serde_json::Value>,
}

impl Config {
    // ── Loading / Saving ────────────────────────────────────────────────

    /// Load config from the default location
    pub fn load() -> crate::Result<Self> {
        let path = Self::default_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&content).map_err(|e| crate::Error::Config(e.to_string()))?)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to the default location
    pub fn save(&self) -> crate::Result<()> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content =
            toml::to_string_pretty(self).map_err(|e| crate::Error::Config(e.to_string()))?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get default config path
    pub fn default_path() -> crate::Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
            .ok_or_else(|| crate::Error::Config("Cannot find config directory".to_string()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get data directory for sessions, caches, etc.
    pub fn data_dir() -> crate::Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
            .ok_or_else(|| crate::Error::Config("Cannot find data directory".to_string()))?;
        Ok(dirs.data_local_dir().to_path_buf())
    }

    // ── Layered config loading ──────────────────────────────────────────

    /// Load config using priority-based layering:
    /// 1. Built-in defaults
    /// 2. Global config (~/.config/com.whycorporation.whycode/config.toml)
    /// 3. Project config (<project_dir>/.whycode/config.toml)
    /// 4. Environment variables (WHYCODE_*)
    ///
    /// Returns the merged configuration.
    pub fn load_layered(project_dir: &Path) -> crate::Result<Self> {
        let mut config = Self::default();

        // Layer 2: global config
        if let Ok(global) = Self::load() {
            config = config.merge_with(&global);
        }

        // Layer 3: project config
        let project_config_path = project_dir.join(".whycode").join("config.toml");
        if project_config_path.exists() {
            match std::fs::read_to_string(&project_config_path) {
                Ok(content) => {
                    match toml::from_str::<Config>(&content) {
                        Ok(project) => {
                            config = config.merge_with(&project);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to parse project config at {}: {}",
                                project_config_path.display(),
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to read project config at {}: {}",
                        project_config_path.display(),
                        e
                    );
                }
            }
        }

        // Layer 4: environment variables
        config.apply_env_overrides();

        Ok(config)
    }

    /// Apply environment variable overrides to this config in-place.
    pub fn apply_env_overrides(&mut self) {
        // WHYCODE_PROVIDER — set as the default provider (first entry) and
        // also populate a basic ProviderConfig if one doesn't exist.
        if let Ok(provider_name) = std::env::var("WHYCODE_PROVIDER") {
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
                        extra: HashMap::new(),
                    },
                );
            }

            // If no default model is set, try to pick it up from WHYCODE_MODEL
            if self.default_model.is_none() {
                if let Ok(model_name) = std::env::var("WHYCODE_MODEL") {
                    self.default_model = Some(ModelConfig {
                        model_id: model_name,
                        provider_id: provider_name,
                        max_tokens: None,
                        temperature: None,
                        top_p: None,
                        thinking: None,
                        supports_tools: None,
                        supports_images: None,
                    });
                }
            }
        } else if let Ok(model_name) = std::env::var("WHYCODE_MODEL") {
            // Model set without provider — update default model's model_id if
            // one exists, otherwise create a minimal entry.
            if let Some(ref mut default_model) = self.default_model {
                default_model.model_id = model_name;
            } else {
                self.default_model = Some(ModelConfig {
                    model_id: model_name,
                    provider_id: String::new(),
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    thinking: None,
                    supports_tools: None,
                    supports_images: None,
                });
            }
        }

        // WHYCODE_MAX_TURNS
        if let Ok(val) = std::env::var("WHYCODE_MAX_TURNS") {
            if let Ok(n) = val.parse::<usize>() {
                self.session.max_context_tokens = n;
            }
        }

        // WHYCODE_LOG_LEVEL
        if let Ok(val) = std::env::var("WHYCODE_LOG_LEVEL") {
            self.general.log_level = Some(val);
        }

        // WHYCODE_PROJECT_DIR
        if let Ok(val) = std::env::var("WHYCODE_PROJECT_DIR") {
            self.general.project_path = Some(PathBuf::from(val));
        }
    }

    // ── Merging ─────────────────────────────────────────────────────────

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

        // TUI
        if other.tui.theme.is_some() {
            merged.tui.theme = other.tui.theme.clone();
        }
        if other.tui.key_bindings.is_some() {
            merged.tui.key_bindings = other.tui.key_bindings.clone();
        }

        // General
        if other.general.project_path.is_some() {
            merged.general.project_path = other.general.project_path.clone();
        }
        if other.general.log_level.is_some() {
            merged.general.log_level = other.general.log_level.clone();
        }

        merged
    }

    // ── Command config ──────────────────────────────────────────────────

    /// Get per-command configuration overrides for the given command name.
    pub fn get_command_config(&self, command: &str) -> Option<&CommandConfig> {
        self.command_configs.get(command)
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

    // ── Validation ──────────────────────────────────────────────────────

    /// Validate the configuration and return any issues.
    ///
    /// Checks required fields and emits warnings for common misconfigurations.
    pub fn validate(&self) -> crate::Result<()> {
        let mut issues: Vec<String> = Vec::new();

        // Check that at least one provider is configured or default model exists
        if self.providers.is_empty() && self.default_model.is_none() {
            issues.push(
                "No providers configured and no default model set. \
                 Configure at least one provider or set WHYCODE_PROVIDER / WHYCODE_MODEL."
                    .to_string(),
            );
        }

        // Check default_model has a provider_id if set
        if let Some(ref dm) = self.default_model {
            if dm.provider_id.is_empty() && !self.providers.is_empty() {
                issues.push(format!(
                    "Default model '{}' has no provider_id but {} provider(s) are configured. \
                     Specify a provider_id for the default model.",
                    dm.model_id,
                    self.providers.len()
                ));
            }
            if dm.model_id.is_empty() {
                issues.push("Default model has an empty model_id.".to_string());
            }
        }

        // Check providers for common issues
        for (name, provider) in &self.providers {
            if provider.api_key.is_none() {
                // Check env var: <NAME>_API_KEY or WHYCODE_<NAME>_API_KEY
                let key_from_env = std::env::var(format!("{}_API_KEY", name.to_uppercase()))
                    .or_else(|_| {
                        std::env::var(format!("WHYCODE_{}_API_KEY", name.to_uppercase()))
                    })
                    .or_else(|_| std::env::var("OPENAI_API_KEY"))
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"));

                if key_from_env.is_err() {
                    issues.push(format!(
                        "Provider '{}' has no api_key configured and no matching \
                         environment variable found ({}_API_KEY, OPENAI_API_KEY, or ANTHROPIC_API_KEY).",
                        name,
                        name.to_uppercase()
                    ));
                }
            }

            // Warn if base_url is set to a known developer/local endpoint
            if let Some(ref url) = provider.base_url {
                if url.contains("localhost") || url.contains("127.0.0.1") {
                    issues.push(format!(
                        "Provider '{}' base_url points to localhost ({}). \
                         This is fine for local development but will not work in production.",
                        name, url
                    ));
                }
            }
        }

        // Check session config
        if self.session.max_context_tokens == 0 {
            issues.push("session.max_context_tokens is set to 0. This will disable context.".to_string());
        }

        // Check agents
        if self.agents.is_empty() {
            issues.push(
                "No agents configured. At least one agent is recommended for proper operation."
                    .to_string(),
            );
        }

        // Check that default_agent resolves to an existing agent
        if !self.default_agent.is_empty() && !self.agents.iter().any(|a| a.name == self.default_agent)
        {
            issues.push(format!(
                "Default agent '{}' not found in the agents list.",
                self.default_agent
            ));
        }

        // Report issues
        if issues.is_empty() {
            tracing::info!("Configuration validated successfully.");
            Ok(())
        } else {
            for issue in &issues {
                if issue.contains("localhost") || issue.contains("127.0.0.1") {
                    tracing::warn!("{}", issue);
                } else {
                    tracing::warn!("Config issue: {}", issue);
                }
            }
            // Return the first real error if any; otherwise it's just warnings
            let errors: Vec<&String> = issues
                .iter()
                .filter(|i| {
                    !i.contains("localhost")
                        && !i.contains("127.0.0.1")
                })
                .collect();

            if errors.is_empty() {
                // Only localhost warnings — still ok
                Ok(())
            } else if errors.len() == 1 {
                Err(crate::Error::Config(errors[0].clone()))
            } else {
                Err(crate::Error::Config(errors.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ")))
            }
        }
    }

    // ── Accessors ───────────────────────────────────────────────────────

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

    /// Get agent by name
    pub fn get_agent(&self, name: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Get the default agent
    pub fn default_agent(&self) -> Option<&AgentInfo> {
        self.get_agent(&self.default_agent)
    }
}

// ── ToolsConfig merging helper ────────────────────────────────────────

impl ToolsConfig {
    fn merge_with(&self, other: &ToolsConfig) -> ToolsConfig {
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

        if !other.disabled_tools.is_empty() {
            merged.disabled_tools = other.disabled_tools.clone();
        }
        for (name, tool) in &other.custom_tools {
            merged.custom_tools.entry(name.clone()).or_insert_with(|| tool.clone());
        }

        merged
    }
}
