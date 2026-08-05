//! Whycode configuration: load, merge, validate.
//!
//! Leaf types (`Message`, `Tool`, sandbox policy) live in `whycode-core`.
//! This crate owns the user-facing `Config` tree and I/O.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use whycode_core::network::{self, NetworkPolicy};
use whycode_core::types::{
    AgentInfo, ModelConfig, PermissionAction, PermissionSet, ProviderConfig,
};
use whycode_core::{Error, Result};

// Re-export leaf sandbox types so callers that already import `whycode_config`
// can resolve sandbox policy without a second crate path.
pub use whycode_core::sandbox::{SandboxFallback, SandboxMode, SandboxSettings};

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

    /// MCP server configurations (OpenCode-compatible)
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Global tool permissions (OpenCode-style allow/ask/deny).
    /// Merged into each agent; agent-level `permission.rules` wins on conflict.
    #[serde(default)]
    pub permission: HashMap<String, PermissionAction>,

    /// Custom slash commands keyed by name (from config.toml + markdown files)
    #[serde(default)]
    pub commands: HashMap<String, CustomCommandConfig>,

    /// Shell command risk gating.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Pre/post tool hooks (shell commands). Empty by default.
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
}

/// When a hook runs relative to a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before the tool executes. Non-zero exit can block when `block_on_failure`.
    #[default]
    PreTool,
    /// After the tool finishes. Failures are logged only.
    PostTool,
}

/// A single shell hook attached to tool execution.
///
/// ```toml
/// [[hooks]]
/// event = "pre_tool"
/// match = "bash"           # tool name; `*` or `prefix*` / `*suffix`
/// command = "echo check"
/// block_on_failure = true  # pre_tool only: non-zero exit refuses the tool
/// timeout_secs = 30
/// ```
///
/// Environment for the command: `WHYCODE_HOOK_EVENT`, `WHYCODE_TOOL_NAME`,
/// `WHYCODE_TOOL_INPUT` (JSON), `WHYCODE_TOOL_ID`, `WHYCODE_SESSION_ID`,
/// `WHYCODE_WORKING_DIR`. Post-tool also sets `WHYCODE_TOOL_IS_ERROR` and
/// `WHYCODE_TOOL_OUTPUT` (truncated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// `pre_tool` or `post_tool`.
    #[serde(default)]
    pub event: HookEvent,

    /// Tool name pattern. Default `*` (all tools).
    #[serde(default = "default_hook_match", rename = "match")]
    pub tool_match: String,

    /// Shell command (run via `sh -c` / `cmd /C`).
    pub command: String,

    /// When true on `pre_tool`, a non-zero exit refuses the tool call.
    #[serde(default)]
    pub block_on_failure: bool,

    /// Kill the hook after this many seconds. Default 30.
    #[serde(default = "default_hook_timeout")]
    pub timeout_secs: u64,
}

fn default_hook_match() -> String {
    "*".to_string()
}

fn default_hook_timeout() -> u64 {
    30
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            event: HookEvent::PreTool,
            tool_match: default_hook_match(),
            command: String::new(),
            block_on_failure: false,
            timeout_secs: default_hook_timeout(),
        }
    }
}

/// Settings for shell safety: risk classification, OS sandbox, and network
/// domain policy for HTTP tools.
///
/// The risk classifier inspects the command *string* a model asked to run.
/// The sandbox (when enabled) confines the process that runs it. They stack;
/// neither replaces the other. See the README security section.
///
/// `network_allowlist` / `network_denylist` gate `webfetch`, `websearch`, and
/// GitHub API tools by host pattern. Shell network stays binary
/// (`sandbox_network`); domain filtering does not apply inside the shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Lowest risk level that requires confirmation: `caution`, `destructive`
    /// or `off`. Commands classified `catastrophic` are always refused and
    /// this setting cannot override that.
    #[serde(default = "default_risk_threshold")]
    pub bash_risk_threshold: String,

    /// OS sandbox for `bash` / `shell`: `off` | `workspace` (default).
    #[serde(default = "default_sandbox_mode")]
    pub sandbox: String,

    /// When `sandbox = "workspace"`, whether the sandboxed shell may use the
    /// network. Default `true` so `cargo` / `npm` / `git` keep working.
    #[serde(default = "default_true_bool")]
    pub sandbox_network: bool,

    /// When the requested sandbox cannot be applied (no `bwrap`, non-Linux):
    /// `allow` (default — warn and run on host) or `deny` (fail the tool call).
    #[serde(default = "default_sandbox_fallback")]
    pub sandbox_fallback: String,

    /// Host patterns allowed for outbound HTTP tools. Empty (default) means
    /// unrestricted. See [`NetworkPolicy`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_allowlist: Vec<String>,

    /// Host patterns always blocked for outbound HTTP tools (wins over
    /// allowlist).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_denylist: Vec<String>,
}

fn default_risk_threshold() -> String {
    "destructive".to_string()
}

fn default_sandbox_mode() -> String {
    "workspace".to_string()
}

fn default_sandbox_fallback() -> String {
    "allow".to_string()
}

fn default_true_bool() -> bool {
    true
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            bash_risk_threshold: default_risk_threshold(),
            sandbox: default_sandbox_mode(),
            sandbox_network: true,
            sandbox_fallback: default_sandbox_fallback(),
            network_allowlist: Vec::new(),
            network_denylist: Vec::new(),
        }
    }
}

impl SecurityConfig {
    /// Build the HTTP-tool network policy from config lists.
    pub fn network_policy(&self) -> NetworkPolicy {
        NetworkPolicy {
            allowlist: self.network_allowlist.clone(),
            denylist: self.network_denylist.clone(),
        }
    }

    /// Resolved OS sandbox policy for shell tools.
    pub fn sandbox_settings(&self) -> SandboxSettings {
        SandboxSettings::from_raw(&self.sandbox, self.sandbox_network, &self.sandbox_fallback)
    }
}

/// Custom slash command definition (OpenCode `/commands` parity)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomCommandConfig {
    /// Prompt template (`$ARGUMENTS`, `$1`, `$2`, …)
    pub template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Force subagent invocation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtask: Option<bool>,
}

/// How to reach an MCP server.
///
/// - `stdio` — spawn a local process (default when `command` is set)
/// - `http` — Streamable HTTP (spec 2025-03-26+); preferred remote transport
/// - `sse` — legacy HTTP+SSE (spec 2024-11-05)
/// - `auto` — for URL endpoints: try Streamable HTTP, fall back to legacy SSE
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportKind {
    #[default]
    Stdio,
    /// Streamable HTTP (single MCP endpoint, POST/GET + optional SSE)
    Http,
    /// Legacy dual-endpoint HTTP+SSE transport
    Sse,
    /// Probe Streamable HTTP first, then fall back to legacy SSE
    Auto,
}

/// MCP server definition stored in config.
///
/// Stdio: `command` + `args`. Remote: `url` (+ optional `type` / `headers`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Explicit transport (`stdio` | `http` | `sse` | `auto`).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub transport: Option<McpTransportKind>,
    /// Command to spawn (stdio transport)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Remote MCP endpoint URL (Streamable HTTP or legacy SSE)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
}

impl McpServerConfig {
    pub fn resolved_transport(&self) -> std::result::Result<McpTransportKind, String> {
        if let Some(kind) = self.transport {
            return match kind {
                McpTransportKind::Stdio if self.command.is_none() => {
                    Err("MCP transport 'stdio' requires `command`".into())
                }
                McpTransportKind::Http | McpTransportKind::Sse | McpTransportKind::Auto
                    if self.url.is_none() =>
                {
                    Err(format!("MCP transport '{kind:?}' requires `url`"))
                }
                other => Ok(other),
            };
        }
        match (&self.url, &self.command) {
            (Some(_), _) => Ok(McpTransportKind::Auto),
            (None, Some(_)) => Ok(McpTransportKind::Stdio),
            (None, None) => {
                Err("MCP server config needs either `command` (stdio) or `url` (http/sse)".into())
            }
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(
            self.resolved_transport().ok(),
            Some(McpTransportKind::Http | McpTransportKind::Sse | McpTransportKind::Auto)
        )
    }
}

impl Default for Config {
    fn default() -> Self {
        use whycode_core::types::{AgentInfo, AgentMode, PermissionSet};

        let build_agent = AgentInfo {
            name: "build".to_string(),
            description:
                "Primary coding agent with full tool access — write, edit, shell, and network."
                    .to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: None, // Loaded from prompts/build.txt at runtime
            temperature: None,
            top_p: None,
        };

        // Plan: primary (Tab-switchable), file edits denied — matches OpenCode
        let plan_agent = AgentInfo {
            name: "plan".to_string(),
            description: "Read-only planning agent — analyzes code and proposes changes but does not modify files. Bash requires care.".to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: Some(vec![
                    "read".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "list".to_string(),
                    "webfetch".to_string(),
                    "websearch".to_string(),
                    "lsp".to_string(),
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "bash".to_string(),
                    "shell".to_string(),
                    "apply_patch".to_string(),
                    "todowrite".to_string(),
                    "todo".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are Whycode in planning mode. You are READ-ONLY. Analyze code and propose changes but do NOT edit files or run shell commands. Output a structured plan.".to_string()),
            temperature: None,
            top_p: None,
        };

        // Explore: subagent (OpenCode) — fast read-only codebase search
        let explore_agent = AgentInfo {
            name: "explore".to_string(),
            description: "Fast read-only subagent for exploring codebases (find files, search keywords, answer questions).".to_string(),
            mode: AgentMode::Subagent,
            permission: PermissionSet {
                allowed_tools: Some(vec![
                    "read".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "list".to_string(),
                    "webfetch".to_string(),
                    "websearch".to_string(),
                    "lsp".to_string(),
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "bash".to_string(),
                    "shell".to_string(),
                    "apply_patch".to_string(),
                    "todowrite".to_string(),
                    "todo".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a read-only exploration subagent. Find files, search code, answer questions. No file modifications.".to_string()),
            temperature: None,
            top_p: None,
        };

        // General: subagent with full tool access except todo (OpenCode)
        let general_agent = AgentInfo {
            name: "general".to_string(),
            description: "General-purpose subagent for complex multi-step tasks. Full tools except todo. Use for parallel units of work.".to_string(),
            mode: AgentMode::Subagent,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: Some(vec![
                    "todowrite".to_string(),
                    "todo".to_string(),
                    "todoread".to_string(),
                ]),
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a general-purpose subagent for complex searches and multi-step tasks. Complete the goal thoroughly.".to_string()),
            temperature: None,
            top_p: None,
        };

        // Scout: subagent for external docs / dependency research (OpenCode)
        let scout_agent = AgentInfo {
            name: "scout".to_string(),
            description: "Read-only subagent for external docs and dependency research.".to_string(),
            mode: AgentMode::Subagent,
            permission: PermissionSet {
                allowed_tools: Some(vec![
                    "read".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "list".to_string(),
                    "webfetch".to_string(),
                    "websearch".to_string(),
                    "bash".to_string(), // limited: clone/inspect only ideally
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "apply_patch".to_string(),
                    "todowrite".to_string(),
                    "todo".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a scout subagent. Research external docs and dependencies. Prefer webfetch/websearch. Do not modify the workspace project files.".to_string()),
            temperature: None,
            top_p: None,
        };

        Config {
            providers: HashMap::new(),
            models: HashMap::new(),
            agents: vec![
                build_agent,
                plan_agent,
                explore_agent,
                general_agent,
                scout_agent,
            ],
            default_agent: "build".to_string(),
            default_model: None,
            command_configs: HashMap::new(),
            tools: ToolsConfig::default(),
            session: SessionConfig::default(),
            tui: TuiConfig::default(),
            general: GeneralConfig::default(),
            mcp_servers: HashMap::new(),
            permission: HashMap::new(),
            commands: HashMap::new(),
            security: SecurityConfig::default(),
            hooks: Vec::new(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_max_tokens")]
    pub max_context_tokens: usize,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: usize,
    pub store_path: Option<PathBuf>,
    /// Auto-name sessions: project placeholder → first-message heuristic →
    /// small-model refine after the first turn. Default on.
    #[serde(default = "default_true")]
    pub auto_title: bool,
    /// Optional title model: `provider/model` or bare model id (same provider).
    /// Empty = pick a known small/fast sibling (haiku / mini / flash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_model: Option<String>,
    /// Tools advertised to the model: `core` (default, ~12 tools, faster TTFT)
    /// or `full` (every built-in including github/web/lsp).
    #[serde(default = "default_tool_profile")]
    pub tool_profile: String,
    /// Anthropic-style prompt cache policy: `auto` (default) or `none`.
    #[serde(default = "default_prompt_cache")]
    pub prompt_cache: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: default_max_tokens(),
            compaction_threshold: default_compaction_threshold(),
            store_path: None,
            auto_title: true,
            title_model: None,
            tool_profile: default_tool_profile(),
            prompt_cache: default_prompt_cache(),
        }
    }
}

fn default_tool_profile() -> String {
    "core".into()
}

fn default_prompt_cache() -> String {
    "auto".into()
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
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let mut cfg: Config =
                toml::from_str(&content).map_err(|e| Error::Config(e.to_string()))?;
            // When a table is keyed `[providers.foo]` but omits `name`, use the key.
            for (key, provider) in &mut cfg.providers {
                if provider.name.is_empty() {
                    provider.name = key.clone();
                }
            }
            Ok(cfg)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to the default location
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get default config path
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
            .ok_or_else(|| Error::Config("Cannot find config directory".to_string()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get data directory for sessions, caches, etc.
    pub fn data_dir() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
            .ok_or_else(|| Error::Config("Cannot find data directory".to_string()))?;
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
        let project_config_path = project_dir.join(".whycode").join("config.toml");
        if project_config_path.exists() {
            match std::fs::read_to_string(&project_config_path) {
                Ok(content) => match toml::from_str::<Config>(&content) {
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
                },
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
                        tool_arguments: None,
                        extra: HashMap::new(),
                    },
                );
            }

            // If no default model is set, try to pick it up from WHYCODE_MODEL
            if self.default_model.is_none()
                && let Ok(model_name) = std::env::var("WHYCODE_MODEL")
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
                    context_window: None,
                    temperature: None,
                    top_p: None,
                    thinking: None,
                    supports_tools: None,
                    supports_images: None,
                });
            }
        }

        // WHYCODE_MAX_TURNS
        if let Ok(val) = std::env::var("WHYCODE_MAX_TURNS")
            && let Ok(n) = val.parse::<usize>()
        {
            self.session.max_context_tokens = n;
        }

        // WHYCODE_LOG_LEVEL
        if let Ok(val) = std::env::var("WHYCODE_LOG_LEVEL") {
            self.general.log_level = Some(val);
        }

        // WHYCODE_PROJECT_DIR
        if let Ok(val) = std::env::var("WHYCODE_PROJECT_DIR") {
            self.general.project_path = Some(PathBuf::from(val));
        }

        if let Ok(val) = std::env::var("WHYCODE_SANDBOX") {
            self.security.sandbox = val;
        }
        if let Ok(val) = std::env::var("WHYCODE_SANDBOX_NETWORK") {
            self.security.sandbox_network = matches!(
                val.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
        if let Ok(val) = std::env::var("WHYCODE_SANDBOX_FALLBACK") {
            self.security.sandbox_fallback = val;
        }
        if let Ok(val) = std::env::var("WHYCODE_NETWORK_ALLOWLIST") {
            self.security.network_allowlist = network::parse_domain_list(&val);
        }
        if let Ok(val) = std::env::var("WHYCODE_NETWORK_DENYLIST") {
            self.security.network_denylist = network::parse_domain_list(&val);
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

    /// Load custom commands from markdown files (OpenCode paths adapted):
    /// - global: `~/.config/.../commands/*.md`
    /// - project: `<project>/.whycode/commands/*.md`
    pub fn load_command_files(&mut self, project_dir: &Path) {
        if let Ok(global_dir) = Self::default_path()
            && let Some(parent) = global_dir.parent()
        {
            load_commands_from_dir(&mut self.commands, &parent.join("commands"));
        }
        load_commands_from_dir(
            &mut self.commands,
            &project_dir.join(".whycode").join("commands"),
        );
        // also accept OpenCode-style .opencode/commands
        load_commands_from_dir(
            &mut self.commands,
            &project_dir.join(".opencode").join("commands"),
        );
    }
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
fn parse_command_markdown(content: &str) -> Option<CustomCommandConfig> {
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

impl CustomCommandConfig {
    /// Expand `$ARGUMENTS`, `$1`… placeholders.
    pub fn render(&self, args: &str) -> String {
        let parts: Vec<&str> = args.split_whitespace().collect();
        let mut out = self.template.clone();
        out = out.replace("$ARGUMENTS", args);
        for (i, p) in parts.iter().enumerate() {
            out = out.replace(&format!("${}", i + 1), p);
        }
        // shell injection: !`cmd` — run and replace (best-effort, sync)
        while let Some(start) = out.find("!`") {
            if let Some(rel_end) = out[start + 2..].find('`') {
                let cmd = &out[start + 2..start + 2 + rel_end];
                let output = run_inline_shell(cmd);
                out.replace_range(start..start + 2 + rel_end + 1, &output);
            } else {
                break;
            }
        }
        out
    }
}

fn run_inline_shell(cmd: &str) -> String {
    #[cfg(windows)]
    let output = std::process::Command::new("cmd").args(["/C", cmd]).output();
    #[cfg(not(windows))]
    let output = std::process::Command::new("sh").args(["-c", cmd]).output();
    match output {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.is_empty() {
                s.push_str(&err);
            }
            s
        }
        Err(e) => format!("(command failed: {})", e),
    }
}

impl Config {
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
    pub fn validate(&self) -> Result<()> {
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
                    .or_else(|_| std::env::var(format!("WHYCODE_{}_API_KEY", name.to_uppercase())))
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
            if let Some(ref url) = provider.base_url
                && (url.contains("localhost") || url.contains("127.0.0.1"))
            {
                issues.push(format!(
                    "Provider '{}' base_url points to localhost ({}). \
                     This is fine for local development but will not work in production.",
                    name, url
                ));
            }
        }

        // Check session config
        if self.session.max_context_tokens == 0 {
            issues.push(
                "session.max_context_tokens is set to 0. This will disable context.".to_string(),
            );
        }

        // Check agents
        if self.agents.is_empty() {
            issues.push(
                "No agents configured. At least one agent is recommended for proper operation."
                    .to_string(),
            );
        }

        // Check that default_agent resolves to an existing agent
        if !self.default_agent.is_empty()
            && !self.agents.iter().any(|a| a.name == self.default_agent)
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
                .filter(|i| !i.contains("localhost") && !i.contains("127.0.0.1"))
                .collect();

            if errors.is_empty() {
                // Only localhost warnings — still ok
                Ok(())
            } else if errors.len() == 1 {
                Err(Error::Config(errors[0].clone()))
            } else {
                Err(Error::Config(
                    errors
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("; "),
                ))
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

    /// Explicit `context_window` from config for this provider/model, if any.
    ///
    /// Does not consult the built-in catalog — call
    /// `whycode_llm::resolve_context_window` for the full chain.
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
    use super::*;
    use std::io::Write;

    fn make_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            api_key: Some(format!("key-{}", name)),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        }
    }

    fn make_model(provider: &str, model: &str) -> ModelConfig {
        ModelConfig {
            model_id: model.to_string(),
            provider_id: provider.to_string(),
            max_tokens: Some(4096),
            context_window: None,
            temperature: None,
            top_p: None,
            thinking: None,
            supports_tools: None,
            supports_images: None,
        }
    }

    // ── test_default_config ─────────────────────────────────────────────

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.agents.len(), 5, "default config should have 5 agents");
        assert_eq!(cfg.default_agent, "build");
        assert!(cfg.default_model.is_none());
        assert!(cfg.providers.is_empty());
        assert!(cfg.models.is_empty());

        // Verify agent names (OpenCode: build/plan primary, general/explore/scout subagents)
        let names: Vec<&str> = cfg.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"build"));
        assert!(names.contains(&"plan"));
        assert!(names.contains(&"explore"));
        assert!(names.contains(&"general"));
        assert!(names.contains(&"scout"));
    }

    // ── test_config_load_save ───────────────────────────────────────────

    #[test]
    fn test_config_serialize_deserialize_roundtrip() {
        let mut cfg = Config::default();
        cfg.providers
            .insert("openai".to_string(), make_provider("openai"));
        cfg.models
            .insert("gpt-4".to_string(), make_model("openai", "gpt-4"));

        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(loaded.agents.len(), cfg.agents.len());
        assert_eq!(loaded.default_agent, cfg.default_agent);
        assert_eq!(loaded.providers.len(), cfg.providers.len());
        assert!(loaded.providers.contains_key("openai"));
        assert_eq!(
            loaded.providers["openai"].api_key.as_deref(),
            Some("key-openai")
        );
        assert_eq!(loaded.models.len(), cfg.models.len());
        assert_eq!(loaded.models["gpt-4"].model_id, "gpt-4");
    }

    #[test]
    fn test_config_load_save_tempfile() {
        let mut cfg = Config::default();
        cfg.providers
            .insert("openai".to_string(), make_provider("openai"));

        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");

        let dir = std::env::temp_dir();
        let path = dir.join(format!("whycode-test-config-{}.toml", std::process::id()));
        {
            let mut f = std::fs::File::create(&path).expect("create temp file");
            f.write_all(toml_str.as_bytes()).expect("write");
        }

        let content = std::fs::read_to_string(&path).expect("read back");
        let loaded: Config = toml::from_str(&content).expect("deserialize");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.agents.len(), 5);
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers["openai"].name, "openai");
    }

    // ── test_get_provider ───────────────────────────────────────────────

    #[test]
    fn test_get_provider() {
        let mut cfg = Config::default();
        cfg.providers
            .insert("anthropic".to_string(), make_provider("anthropic"));

        assert!(cfg.get_provider("openai").is_none());
        let p = cfg
            .get_provider("anthropic")
            .expect("should find anthropic");
        assert_eq!(p.name, "anthropic");
        assert_eq!(p.api_key.as_deref(), Some("key-anthropic"));
    }

    // ── test_get_model ──────────────────────────────────────────────────

    #[test]
    fn test_get_model() {
        let mut cfg = Config::default();
        cfg.models
            .insert("gpt-4".to_string(), make_model("openai", "gpt-4"));
        cfg.models
            .insert("claude-3".to_string(), make_model("anthropic", "claude-3"));

        let m = cfg.get_model("openai", "gpt-4").expect("should find gpt-4");
        assert_eq!(m.model_id, "gpt-4");
        assert_eq!(m.provider_id, "openai");

        let m = cfg
            .get_model("anthropic", "claude-3")
            .expect("should find claude-3");
        assert_eq!(m.model_id, "claude-3");

        assert!(cfg.get_model("openai", "nonexistent").is_none());
        assert!(cfg.get_model("nonexistent", "gpt-4").is_none());
    }

    // ── test_get_agent ──────────────────────────────────────────────────

    #[test]
    fn test_get_agent() {
        let cfg = Config::default();

        let a = cfg.get_agent("build").expect("should find build agent");
        assert_eq!(a.name, "build");
        assert!(a.permission.allow_file_writes);
        assert!(a.permission.allow_shell);

        let a = cfg.get_agent("plan").expect("should find plan agent");
        assert_eq!(a.name, "plan");
        assert!(!a.permission.allow_file_writes);
        assert!(!a.permission.allow_shell);

        assert!(cfg.get_agent("nonexistent").is_none());
    }

    #[test]
    fn test_default_agent() {
        let cfg = Config::default();
        let a = cfg.default_agent().expect("default agent should exist");
        assert_eq!(a.name, "build");
    }

    // ── test_substitute_vars ────────────────────────────────────────────

    #[test]
    fn test_substitute_vars_braced() {
        unsafe {
            std::env::set_var("WHYCODE_TEST_FOO", "hello-world");
        }
        let result = Config::substitute_vars("prefix ${WHYCODE_TEST_FOO} suffix");
        assert_eq!(result, "prefix hello-world suffix");
        unsafe {
            std::env::remove_var("WHYCODE_TEST_FOO");
        }
    }

    #[test]
    fn test_substitute_vars_unbraced() {
        unsafe {
            std::env::set_var("WHYCODE_TEST_BAR", "bar-value");
        }
        let result = Config::substitute_vars("start $WHYCODE_TEST_BAR end");
        assert_eq!(result, "start bar-value end");
        unsafe {
            std::env::remove_var("WHYCODE_TEST_BAR");
        }
    }

    #[test]
    fn test_substitute_vars_unknown_kept() {
        let result = Config::substitute_vars("${NONEXISTENT_VAR_12345}");
        assert_eq!(result, "${NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn test_substitute_vars_lone_dollar() {
        let result = Config::substitute_vars("just a $ sign");
        assert_eq!(result, "just a $ sign");
    }

    #[test]
    fn test_substitute_vars_no_vars() {
        let result = Config::substitute_vars("plain text with no variables");
        assert_eq!(result, "plain text with no variables");
    }

    // ── test_merge_with ─────────────────────────────────────────────────

    #[test]
    fn test_merge_with_priority() {
        let mut base = Config::default();
        base.providers
            .insert("openai".to_string(), make_provider("openai"));
        base.default_agent = "plan".to_string();

        let mut overlay = Config::default();
        overlay
            .providers
            .insert("anthropic".to_string(), make_provider("anthropic"));
        overlay.default_agent = "explore".to_string();
        let mc = make_model("openai", "gpt-4");
        overlay.models.insert("gpt-4".to_string(), mc.clone());
        overlay.default_model = Some(mc);

        let merged = base.merge_with(&overlay);

        // overlay providers are added
        assert_eq!(merged.providers.len(), 2);
        assert!(merged.providers.contains_key("openai"));
        assert!(merged.providers.contains_key("anthropic"));

        // overlay default_agent wins
        assert_eq!(merged.default_agent, "explore");

        // overlay default_model wins
        assert!(merged.default_model.is_some());
        assert_eq!(merged.default_model.unwrap().model_id, "gpt-4");
    }

    #[test]
    fn test_merge_with_provider_override() {
        let mut base = Config::default();
        base.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                name: "openai".to_string(),
                api_key: Some("old-key".to_string()),
                api_base: Some("https://old.example.com".to_string()),
                base_url: None,
                headers: None,
                models: vec![],
                tool_arguments: None,
                extra: HashMap::new(),
            },
        );

        let mut overlay = Config::default();
        overlay.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                name: "openai".to_string(),
                api_key: Some("new-key".to_string()),
                api_base: None,
                base_url: None,
                headers: None,
                models: vec![],
                tool_arguments: None,
                extra: HashMap::new(),
            },
        );

        let merged = base.merge_with(&overlay);
        assert_eq!(merged.providers.len(), 1);
        let p = &merged.providers["openai"];
        // api_key from overlay wins
        assert_eq!(p.api_key.as_deref(), Some("new-key"));
        // api_base from base is kept (overlay didn't set it)
        assert_eq!(p.api_base.as_deref(), Some("https://old.example.com"));
    }
    #[test]
    fn mcp_stdio_config_deserializes() {
        let toml = r#"
            [mcp_servers.fs]
            command = "npx"
            args = ["-y", "server"]
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let s = &cfg.mcp_servers["fs"];
        assert_eq!(s.command.as_deref(), Some("npx"));
        assert_eq!(s.resolved_transport().unwrap(), McpTransportKind::Stdio);
        assert!(!s.is_remote());
    }

    #[test]
    fn hooks_table_deserializes() {
        let toml = r#"
            [[hooks]]
            event = "pre_tool"
            match = "bash"
            command = "echo pre"
            block_on_failure = true
            timeout_secs = 10

            [[hooks]]
            event = "post_tool"
            match = "*"
            command = "true"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.hooks.len(), 2);
        assert_eq!(cfg.hooks[0].event, HookEvent::PreTool);
        assert_eq!(cfg.hooks[0].tool_match, "bash");
        assert!(cfg.hooks[0].block_on_failure);
        assert_eq!(cfg.hooks[0].timeout_secs, 10);
        assert_eq!(cfg.hooks[1].event, HookEvent::PostTool);
        assert_eq!(cfg.hooks[1].tool_match, "*");
        assert!(!cfg.hooks[1].block_on_failure);
        assert_eq!(cfg.hooks[1].timeout_secs, 30); // default
    }

    #[test]
    fn merge_hooks_nonempty_replaces() {
        let mut base = Config::default();
        base.hooks.push(HookConfig {
            event: HookEvent::PreTool,
            tool_match: "bash".into(),
            command: "base".into(),
            block_on_failure: false,
            timeout_secs: 5,
        });
        let mut overlay = Config::default();
        overlay.hooks.push(HookConfig {
            event: HookEvent::PostTool,
            tool_match: "*".into(),
            command: "overlay".into(),
            block_on_failure: false,
            timeout_secs: 5,
        });
        let merged = base.merge_with(&overlay);
        assert_eq!(merged.hooks.len(), 1);
        assert_eq!(merged.hooks[0].command, "overlay");
    }

    #[test]
    fn mcp_http_url_infers_auto() {
        let toml = r#"
            [mcp_servers.remote]
            url = "https://mcp.example.com/mcp"
            headers = { Authorization = "Bearer t" }
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let s = &cfg.mcp_servers["remote"];
        assert_eq!(s.resolved_transport().unwrap(), McpTransportKind::Auto);
        assert!(s.is_remote());
    }

    #[test]
    fn mcp_explicit_sse_type() {
        let toml = r#"
            [mcp_servers.legacy]
            type = "sse"
            url = "http://127.0.0.1:9/sse"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.mcp_servers["legacy"].resolved_transport().unwrap(),
            McpTransportKind::Sse
        );
    }

    #[test]
    fn mcp_http_type_requires_url() {
        let s = McpServerConfig {
            transport: Some(McpTransportKind::Http),
            command: None,
            args: vec![],
            env: None,
            cwd: None,
            url: None,
            headers: None,
        };
        assert!(s.resolved_transport().is_err());
    }
}
