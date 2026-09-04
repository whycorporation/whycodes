//! Config schema types and serde defaults.
//!
//! Leaf types (`Message`, `Tool`, sandbox policy) live in `whycodes-core`.
//! This module owns the user-facing `Config` tree.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use whycodes_core::network::{self, NetworkPolicy};
use whycodes_core::sandbox::SandboxSettings;
use whycodes_core::types::{
    AgentInfo, ApprovalMode, ModelConfig, PermissionAction, ProviderConfig,
};

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

    /// On-disk schema version. Missing / `0` is migrated to
    /// [`CONFIG_SCHEMA_VERSION`] on load.
    #[serde(default)]
    pub schema_version: u32,

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

    /// Cross-session semantic / auto memory.
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Parallel multi-agent swarm + file conflict notify.
    #[serde(default)]
    pub swarm: SwarmConfig,

    /// Background jobs and lightweight scheduling (FEATURES §11).
    #[serde(default)]
    pub automation: AutomationConfig,

    /// Discord / Telegram session notifications (off by default).
    #[serde(default)]
    pub notify: NotifyConfig,
}

/// Process-local background shell jobs and schedule/loop knobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    /// Max concurrent background shell jobs. Default 8.
    #[serde(default = "default_max_background_jobs")]
    pub max_background_jobs: usize,
}

pub(crate) fn default_max_background_jobs() -> usize {
    8
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            max_background_jobs: default_max_background_jobs(),
        }
    }
}

/// Concurrent multi-agent work (`swarm` tool).
///
/// When `worktrees` is on (default) and the project is a git repo, each worker
/// runs in a detached git worktree under `.whycodes/swarm/`. Changes merge back
/// into the main checkout with three-way conflict detection. File claims still
/// gate same-checkout mode and pre-declared path ownership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmConfig {
    /// Advertise and run the `swarm` tool. Default on.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Max concurrent workers (hard-capped at 8 in the agent). Default 4.
    #[serde(default = "default_swarm_max_agents")]
    pub max_agents: usize,
    /// Isolate writers in git worktrees (default on). Falls back to same-checkout
    /// + file claims when the project is not a git repo.
    #[serde(default = "default_true")]
    pub worktrees: bool,
    /// `worktree` or `checkout`. When set, overrides `worktrees`.
    #[serde(default)]
    pub isolation: Option<String>,
}

impl SwarmConfig {
    /// Whether to try git worktrees for this run.
    pub fn use_worktrees(&self) -> bool {
        match self.isolation.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("checkout") => false,
            Some(s) if s.eq_ignore_ascii_case("worktree") => true,
            _ => self.worktrees,
        }
    }
}

pub(crate) fn default_swarm_max_agents() -> usize {
    4
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_agents: default_swarm_max_agents(),
            worktrees: true,
            isolation: None,
        }
    }
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
/// Environment for the command: `WHYCODES_HOOK_EVENT`, `WHYCODES_TOOL_NAME`,
/// `WHYCODES_TOOL_INPUT` (JSON), `WHYCODES_TOOL_ID`, `WHYCODES_SESSION_ID`,
/// `WHYCODES_WORKING_DIR`. Post-tool also sets `WHYCODES_TOOL_IS_ERROR` and
/// `WHYCODES_TOOL_OUTPUT` (truncated).
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

pub(crate) fn default_hook_match() -> String {
    "*".to_string()
}

pub(crate) fn default_hook_timeout() -> u64 {
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

pub(crate) fn default_risk_threshold() -> String {
    "destructive".to_string()
}

pub(crate) fn default_sandbox_mode() -> String {
    "workspace".to_string()
}

pub(crate) fn default_sandbox_fallback() -> String {
    "allow".to_string()
}

pub(crate) fn default_true_bool() -> bool {
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
        use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};

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

        // Plan: primary (Ctrl+T), file edits denied — OpenCode Plan / Cursor Plan
        let plan_agent = AgentInfo {
            name: "plan".to_string(),
            description: "Read-only planning agent — analyzes code and proposes changes but does not modify files.".to_string(),
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
                    "git_commit".to_string(),
                    "task".to_string(),
                    "swarm".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: None, // prompts/plan.txt
            temperature: None,
            top_p: None,
        };

        // Ask: primary read-only Q&A — Cursor Ask mode (no implementation)
        let ask_agent = AgentInfo {
            name: "ask".to_string(),
            description:
                "Read-only Q&A agent — explains code and answers questions without modifying files."
                    .to_string(),
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
                    "question".to_string(),
                ]),
                denied_tools: Some(vec![
                    "write".to_string(),
                    "edit".to_string(),
                    "bash".to_string(),
                    "shell".to_string(),
                    "apply_patch".to_string(),
                    "todowrite".to_string(),
                    "todo".to_string(),
                    "git_commit".to_string(),
                    "task".to_string(),
                    "swarm".to_string(),
                    "plan".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: None, // prompts/ask.txt
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
                    "question".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: false,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a read-only exploration subagent. Find files, search code, answer questions. No file modifications. Do not call `question` — report findings to the parent.".to_string()),
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
                    "question".to_string(),
                ]),
                allow_file_writes: false,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a scout subagent. Research external docs and dependencies. Prefer webfetch/websearch. Do not modify the workspace project files. Do not call `question` — report findings to the parent.".to_string()),
            temperature: None,
            top_p: None,
        };

        Config {
            providers: HashMap::new(),
            models: HashMap::new(),
            agents: vec![
                build_agent,
                plan_agent,
                ask_agent,
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
            schema_version: CONFIG_SCHEMA_VERSION,
            mcp_servers: HashMap::new(),
            permission: HashMap::new(),
            commands: HashMap::new(),
            security: SecurityConfig::default(),
            hooks: Vec::new(),
            memory: MemoryConfig::default(),
            swarm: SwarmConfig::default(),
            automation: AutomationConfig::default(),
            notify: NotifyConfig::default(),
        }
    }
}

/// Cross-session semantic / auto memory (Claude-style index + local recall).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Master switch. Default on (cheap hashing embedder; no ONNX).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Auto-inject top-k semantic hits for the current user message.
    #[serde(default = "default_true")]
    pub auto_inject: bool,
    /// Post-turn auto-retain (heuristic and/or LLM). Default on.
    #[serde(default = "default_true")]
    pub auto_retain: bool,
    /// Also call a small LLM to extract durable facts when heuristic finds none
    /// (or always when `retain_llm_always`). Default on.
    #[serde(default = "default_true")]
    pub retain_llm: bool,
    /// Run LLM retain even if heuristic already found facts.
    #[serde(default)]
    pub retain_llm_always: bool,
    /// Run retain every N user turns (1 = every turn).
    #[serde(default = "default_retain_every_n")]
    pub retain_every_n: usize,
    /// Max facts retained per turn.
    #[serde(default = "default_retain_max_facts")]
    pub retain_max_facts: usize,
    /// Max lines of MEMORY.md loaded into every session (Claude parity: 200).
    #[serde(default = "default_memory_index_lines")]
    pub max_index_lines: usize,
    /// Max bytes of MEMORY.md loaded into every session (Claude parity: 25 KiB).
    #[serde(default = "default_memory_index_bytes")]
    pub max_index_bytes: usize,
    /// Max recalled facts injected per turn.
    #[serde(default = "default_memory_top_k")]
    pub recall_top_k: usize,
    /// Minimum cosine similarity for a fact to be recalled.
    #[serde(default = "default_memory_min_score")]
    pub recall_min_score: f32,
    /// Token budget for recalled facts (chars ≈ tokens × 4).
    #[serde(default = "default_memory_token_budget")]
    pub recall_token_budget: usize,
    /// Hashing embedder dimension (stored BLOB width).
    #[serde(default = "default_memory_embed_dim")]
    pub embed_dim: usize,
    /// `user` (data_dir, default) or `project` (`.whycodes/memory`, git-shareable).
    #[serde(default = "default_memory_scope")]
    pub scope: String,
    /// `hash` (default) or `onnx` (MiniLM; needs `--features onnx`).
    #[serde(default = "default_memory_backend")]
    pub embed_backend: String,
    /// Inject code-index hits when available.
    #[serde(default = "default_true")]
    pub code_inject: bool,
    #[serde(default = "default_code_top_k")]
    pub code_top_k: usize,
    #[serde(default = "default_code_min_score")]
    pub code_min_score: f32,
    /// Give subagents their own memory bank (`project::agent_name`).
    #[serde(default = "default_true")]
    pub subagent_banks: bool,
    /// On session start, ensure a code index exists (skip if already indexed).
    #[serde(default = "default_true")]
    pub auto_index: bool,
    /// Max source files to walk when auto-indexing.
    #[serde(default = "default_auto_index_files")]
    pub auto_index_max_files: usize,
    /// Max chunks when auto-indexing.
    #[serde(default = "default_auto_index_chunks")]
    pub auto_index_max_chunks: usize,
    /// Inject related past-session turns.
    #[serde(default = "default_true")]
    pub session_inject: bool,
    #[serde(default = "default_session_top_k")]
    pub session_top_k: usize,
    #[serde(default = "default_session_min_score")]
    pub session_min_score: f32,
    /// Cap the fact bank after retain.
    #[serde(default = "default_true")]
    pub consolidate: bool,
    #[serde(default = "default_consolidate_max")]
    pub consolidate_max: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_inject: true,
            auto_retain: true,
            retain_llm: true,
            retain_llm_always: false,
            retain_every_n: default_retain_every_n(),
            retain_max_facts: default_retain_max_facts(),
            max_index_lines: default_memory_index_lines(),
            max_index_bytes: default_memory_index_bytes(),
            recall_top_k: default_memory_top_k(),
            recall_min_score: default_memory_min_score(),
            recall_token_budget: default_memory_token_budget(),
            embed_dim: default_memory_embed_dim(),
            scope: default_memory_scope(),
            embed_backend: default_memory_backend(),
            code_inject: true,
            code_top_k: default_code_top_k(),
            code_min_score: default_code_min_score(),
            subagent_banks: true,
            auto_index: true,
            auto_index_max_files: default_auto_index_files(),
            auto_index_max_chunks: default_auto_index_chunks(),
            session_inject: true,
            session_top_k: default_session_top_k(),
            session_min_score: default_session_min_score(),
            consolidate: true,
            consolidate_max: default_consolidate_max(),
        }
    }
}

pub(crate) fn default_memory_index_lines() -> usize {
    200
}
pub(crate) fn default_memory_index_bytes() -> usize {
    25_600
}
pub(crate) fn default_memory_top_k() -> usize {
    5
}
pub(crate) fn default_memory_min_score() -> f32 {
    0.28
}
pub(crate) fn default_memory_token_budget() -> usize {
    800
}
pub(crate) fn default_memory_embed_dim() -> usize {
    256
}
pub(crate) fn default_retain_every_n() -> usize {
    1
}
pub(crate) fn default_retain_max_facts() -> usize {
    3
}
pub(crate) fn default_memory_scope() -> String {
    "user".into()
}
pub(crate) fn default_memory_backend() -> String {
    "hash".into()
}
pub(crate) fn default_code_top_k() -> usize {
    4
}
pub(crate) fn default_code_min_score() -> f32 {
    0.22
}
pub(crate) fn default_auto_index_files() -> usize {
    1500
}
pub(crate) fn default_auto_index_chunks() -> usize {
    4000
}
pub(crate) fn default_session_top_k() -> usize {
    3
}
pub(crate) fn default_session_min_score() -> f32 {
    0.22
}
pub(crate) fn default_consolidate_max() -> usize {
    80
}

pub(crate) fn default_agent() -> String {
    "build".to_string()
}

/// Settings for the interactive `question` tool (Grok-style questionnaire).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionToolConfig {
    /// When true, unanswered questionnaires fail after [`Self::timeout_secs`].
    #[serde(default = "default_question_timeout_enabled")]
    pub timeout_enabled: bool,
    /// Seconds to wait for answers when timeout is enabled (default 1800 = 30 min).
    #[serde(default = "default_question_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for QuestionToolConfig {
    fn default() -> Self {
        Self {
            timeout_enabled: default_question_timeout_enabled(),
            timeout_secs: default_question_timeout_secs(),
        }
    }
}

pub(crate) fn default_question_timeout_enabled() -> bool {
    true
}

pub(crate) fn default_question_timeout_secs() -> u64 {
    1800
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
    /// Interactive questionnaire (`question` tool).
    #[serde(default)]
    pub question: QuestionToolConfig,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    #[serde(default)]
    pub custom_tools: HashMap<String, CustomToolConfig>,
}

pub(crate) fn default_true() -> bool {
    true
}

/// Current `config.toml` schema. Bump when a load-time rewrite is required.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_max_tokens")]
    pub max_context_tokens: usize,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: usize,
    #[serde(default)]
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
    /// Fast model for trivial chat (`provider/model` or bare model id).
    /// Empty = auto-pick small sibling of the session model (haiku/mini/flash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_fast: Option<String>,
    /// Cheap model for `task` / `swarm` fan-out (`provider/model` or bare id).
    /// Empty = auto-pick the small sibling of the session model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_smol: Option<String>,
    /// Model used while the `plan` agent is active (`provider/model` or bare id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_plan: Option<String>,
    /// First-token race partner: `off` (default), `auto` (small sibling), or
    /// `provider/model`. When set, a slow primary TTFT starts the partner.
    #[serde(default = "default_model_race")]
    pub model_race: String,
    /// How long to wait for the primary first token before opening `model_race`.
    #[serde(default = "default_race_after_ms")]
    pub race_after_ms: u64,
    /// Process-local text-only response cache: `auto` (default) or `off`.
    #[serde(default = "default_response_cache")]
    pub response_cache: String,
    /// Heuristic intent posture for build turns: `auto` (default), `off`, or `always`.
    ///
    /// When enabled, high-confidence question/plan signals inject an ephemeral
    /// `<whycodes_intent>` block into the LLM request (not session history) so
    /// the model answers or plans instead of over-eager edits. Hard modes
    /// (`ask` / `plan`) still enforce tool denylists regardless of this flag.
    #[serde(default = "default_intent_guidance")]
    pub intent_guidance: String,
    /// Full-replace compact (manual `/compact` and auto over threshold):
    /// `"auto"` (default) asks the session model for a structured summary;
    /// `"off"` is the local stub only.
    #[serde(default = "default_compaction_llm")]
    pub compaction_llm: String,
    /// OpenAI-compat / xAI `reasoning_effort`: `low` | `medium` | `high` | `xhigh`.
    /// Empty = family default (`medium` when the model supports effort).
    /// `xhigh` (Max) is grok-4.6+ only; older models clamp to `high`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Standalone prose keywords (`ultrathink`, `orchestrate`) that inject a
    /// hidden per-turn instruction. Default on.
    #[serde(default)]
    pub magic_keywords: MagicKeywordsConfig,
    /// Regexes matched against streamed assistant text. A hit aborts the
    /// current LLM request and injects `hint` before the next step.
    #[serde(default)]
    pub stream_rules: Vec<StreamRuleConfig>,
}

/// One time-traveling stream rule (abort + inject).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StreamRuleConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub hint: String,
}

/// Per-turn hidden notices when the user writes a magic keyword in prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicKeywordsConfig {
    /// Master switch. Default on.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `ultrathink` — careful multi-step reasoning + highest thinking effort.
    #[serde(default = "default_true")]
    pub ultrathink: bool,
    /// `orchestrate` — parallel subagent contract until the request is done.
    #[serde(default = "default_true")]
    pub orchestrate: bool,
}

impl Default for MagicKeywordsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ultrathink: true,
            orchestrate: true,
        }
    }
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
            model_fast: None,
            model_smol: None,
            model_plan: None,
            model_race: default_model_race(),
            race_after_ms: default_race_after_ms(),
            response_cache: default_response_cache(),
            intent_guidance: default_intent_guidance(),
            compaction_llm: default_compaction_llm(),
            reasoning_effort: None,
            magic_keywords: MagicKeywordsConfig::default(),
            stream_rules: Vec::new(),
        }
    }
}

pub(crate) fn default_compaction_llm() -> String {
    "auto".into()
}

pub(crate) fn default_tool_profile() -> String {
    "core".into()
}

pub(crate) fn default_prompt_cache() -> String {
    "auto".into()
}

pub(crate) fn default_model_race() -> String {
    "off".into()
}

pub(crate) fn default_race_after_ms() -> u64 {
    800
}

pub(crate) fn default_response_cache() -> String {
    "auto".into()
}

pub(crate) fn default_intent_guidance() -> String {
    "auto".into()
}

pub(crate) fn default_max_tokens() -> usize {
    200_000
}

pub(crate) fn default_compaction_threshold() -> usize {
    150_000
}

/// Session notifications over Discord Incoming Webhooks and/or Telegram bots.
///
/// Off until `on` is non-empty **and** at least one channel is configured.
/// Secrets should live in env (`WHYCODES_NOTIFY_*`) or `${VAR}` substitution,
/// not in a committed project config.
///
/// ```toml
/// [notify]
/// on = ["turn_done", "need_input"]
/// discord_webhook = "${DISCORD_WEBHOOK_URL}"
/// telegram_bot_token = "${TELEGRAM_BOT_TOKEN}"
/// telegram_chat_id = "${TELEGRAM_CHAT_ID}"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotifyConfig {
    /// Events that fire a message. Empty = disabled.
    /// Known values: `turn_done`, `need_input`.
    #[serde(default)]
    pub on: Vec<String>,
    /// Discord Incoming Webhook URL. Empty / omitted = skip Discord.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_webhook: Option<String>,
    /// Telegram bot token from BotFather. Empty / omitted = skip Telegram.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_token: Option<String>,
    /// Telegram chat / group / channel id (`sendMessage` `chat_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_chat_id: Option<String>,
    /// HTTP timeout per channel. Default 8s, clamped 1–60.
    #[serde(default = "default_notify_timeout_secs")]
    pub timeout_secs: u64,
}

pub(crate) fn default_notify_timeout_secs() -> u64 {
    8
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            on: Vec::new(),
            discord_webhook: None,
            telegram_bot_token: None,
            telegram_chat_id: None,
            timeout_secs: default_notify_timeout_secs(),
        }
    }
}

impl NotifyConfig {
    /// True when this event is listed in `on` (case-insensitive).
    pub fn wants(&self, event: NotifyEvent) -> bool {
        self.on.iter().any(|e| NotifyEvent::parse(e) == Some(event))
    }

    /// Discord webhook after trim; `None` if unset/blank.
    pub fn discord_webhook_url(&self) -> Option<&str> {
        nonempty_str(self.discord_webhook.as_deref())
    }

    /// Telegram bot token after trim; `None` if unset/blank.
    pub fn telegram_token(&self) -> Option<&str> {
        nonempty_str(self.telegram_bot_token.as_deref())
    }

    /// Telegram chat id after trim; `None` if unset/blank.
    pub fn telegram_chat(&self) -> Option<&str> {
        nonempty_str(self.telegram_chat_id.as_deref())
    }

    /// True when Discord and/or a complete Telegram pair is configured.
    pub fn has_channel(&self) -> bool {
        self.discord_webhook_url().is_some()
            || (self.telegram_token().is_some() && self.telegram_chat().is_some())
    }

    /// True when this event should actually be sent.
    pub fn enabled_for(&self, event: NotifyEvent) -> bool {
        self.wants(event) && self.has_channel()
    }

    pub(crate) fn merge_with(&self, other: &NotifyConfig) -> NotifyConfig {
        let mut merged = self.clone();
        if !other.on.is_empty() {
            merged.on = other.on.clone();
        }
        if other.discord_webhook.is_some() {
            merged.discord_webhook = other.discord_webhook.clone();
        }
        if other.telegram_bot_token.is_some() {
            merged.telegram_bot_token = other.telegram_bot_token.clone();
        }
        if other.telegram_chat_id.is_some() {
            merged.telegram_chat_id = other.telegram_chat_id.clone();
        }
        if other.timeout_secs != default_notify_timeout_secs() {
            merged.timeout_secs = other.timeout_secs;
        }
        merged
    }
}

/// Lifecycle events that can fire a session notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyEvent {
    /// Parent agent turn finished (success or cancel). Subagents do not fire this.
    TurnDone,
    /// Interactive wait: permission dialog, `question` tool, or SDK/daemon ask.
    NeedInput,
}

impl NotifyEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnDone => "turn_done",
            Self::NeedInput => "need_input",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "turn_done" | "turn-done" | "done" => Some(Self::TurnDone),
            "need_input" | "need-input" | "input" => Some(Self::NeedInput),
            _ => None,
        }
    }
}

pub(crate) fn parse_notify_on_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn nonempty_str(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn nonempty_opt(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Discord Incoming Webhook host allowlist (no arbitrary POST target).
pub fn is_discord_webhook_url(url: &str) -> bool {
    let url = url.trim();
    if !url.starts_with("https://") {
        return false;
    }
    let Ok(host) = network::host_from_url(url) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if host != "discord.com" && host != "discordapp.com" {
        return false;
    }
    // Prefix already required above; `HTTPS://` is rejected by `starts_with`.
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let path_start = rest.find('/').unwrap_or(rest.len());
    let path = rest[path_start..].split(['?', '#']).next().unwrap_or("");
    path.starts_with("/api/webhooks/") && path.len() > "/api/webhooks/".len()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub key_bindings: Option<HashMap<String, String>>,
    /// Idle follow-up suggestions: `"off"` (default) or `"idle"`.
    #[serde(default = "default_prompt_suggestions")]
    pub prompt_suggestions: String,
    /// Show the sidebar when the TUI opens.
    #[serde(default)]
    pub show_sidebar: bool,
    /// Per-agent (and `model`) colors for prompt chrome.
    ///
    /// Values are `#rgb` / `#rrggbb`, a theme role (`accent`, `success`,
    /// `info`, `warning`, `error`, `primary`, `secondary`, `thinking`, `dim`),
    /// or an ANSI name (`red`, `green`, …).
    ///
    /// ```toml
    /// [tui.agent_colors]
    /// build = "#7aa2f7"
    /// plan = "accent"
    /// ask = "info"
    /// model = "secondary"
    /// ```
    #[serde(default)]
    pub agent_colors: HashMap<String, String>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: None,
            key_bindings: None,
            prompt_suggestions: default_prompt_suggestions(),
            show_sidebar: false,
            agent_colors: HashMap::new(),
        }
    }
}

pub(crate) fn default_prompt_suggestions() -> String {
    "off".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub project_path: Option<PathBuf>,
    pub log_level: Option<String>,
    pub default_gcp_project: Option<String>,
    /// Check GitHub for a newer release when an interactive TUI session starts
    /// (home-screen confirm; never a silent replace).
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Session-level overlay for when to interrupt (`auto` | `important` | `manual`).
    /// Omitted = `auto`: auto-answer questions and auto-allow permission asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            project_path: None,
            log_level: None,
            default_gcp_project: None,
            auto_update: true,
            approval_mode: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomToolConfig {
    pub command: String,
    pub description: String,
    pub parameters: Option<serde_json::Value>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp(
        command: Option<&str>,
        url: Option<&str>,
        transport: Option<McpTransportKind>,
    ) -> McpServerConfig {
        McpServerConfig {
            transport,
            command: command.map(str::to_string),
            args: vec![],
            env: None,
            cwd: None,
            url: url.map(str::to_string),
            headers: None,
        }
    }

    #[test]
    fn mcp_notify_hooks_and_discord() {
        assert_eq!(
            mcp(Some("npx"), None, None).resolved_transport().unwrap(),
            McpTransportKind::Stdio
        );
        assert_eq!(
            mcp(None, Some("https://mcp.example"), None)
                .resolved_transport()
                .unwrap(),
            McpTransportKind::Auto
        );
        assert!(mcp(None, None, None).resolved_transport().is_err());
        assert!(
            mcp(None, None, Some(McpTransportKind::Stdio))
                .resolved_transport()
                .is_err()
        );
        assert!(
            mcp(None, None, Some(McpTransportKind::Http))
                .resolved_transport()
                .is_err()
        );
        assert!(
            mcp(None, None, Some(McpTransportKind::Sse))
                .resolved_transport()
                .is_err()
        );
        assert!(
            mcp(None, None, Some(McpTransportKind::Auto))
                .resolved_transport()
                .is_err()
        );
        let remote = mcp(None, Some("https://x"), Some(McpTransportKind::Http));
        assert!(remote.is_remote());
        assert!(!mcp(Some("npx"), None, Some(McpTransportKind::Stdio)).is_remote());

        let mut notify = NotifyConfig::default();
        assert!(!notify.wants(NotifyEvent::TurnDone));
        assert!(!notify.has_channel());
        notify.on = vec!["turn_done".into(), "need-input".into()];
        notify.discord_webhook = Some(" https://discord.com/api/webhooks/1/t ".into());
        assert!(notify.wants(NotifyEvent::TurnDone));
        assert!(notify.wants(NotifyEvent::NeedInput));
        assert!(notify.has_channel());
        assert!(notify.enabled_for(NotifyEvent::TurnDone));
        assert_eq!(
            notify.discord_webhook_url(),
            Some("https://discord.com/api/webhooks/1/t")
        );
        notify.telegram_bot_token = Some(" tok ".into());
        notify.telegram_chat_id = Some(" chat ".into());
        assert_eq!(notify.telegram_token(), Some("tok"));
        assert_eq!(notify.telegram_chat(), Some("chat"));
        let overlay = NotifyConfig {
            on: vec!["turn_done".into()],
            discord_webhook: Some("https://other".into()),
            telegram_bot_token: Some("b".into()),
            telegram_chat_id: Some("c".into()),
            timeout_secs: 12,
        };
        let merged = notify.merge_with(&overlay);
        assert_eq!(merged.timeout_secs, 12);

        assert_eq!(NotifyEvent::TurnDone.as_str(), "turn_done");
        assert_eq!(NotifyEvent::NeedInput.as_str(), "need_input");
        assert_eq!(NotifyEvent::parse("done"), Some(NotifyEvent::TurnDone));
        assert_eq!(NotifyEvent::parse("input"), Some(NotifyEvent::NeedInput));
        assert_eq!(NotifyEvent::parse("nope"), None);
        assert_eq!(
            parse_notify_on_csv("turn_done, need_input"),
            vec!["turn_done", "need_input"]
        );

        assert_eq!(HookEvent::default(), HookEvent::PreTool);
        let hook_json = serde_json::to_string(&HookEvent::PostTool).unwrap();
        assert!(hook_json.contains("post_tool"));

        assert!(is_discord_webhook_url(
            "https://discord.com/api/webhooks/123456789012345678/abcdefghijklmnopqrstuvwxyz"
        ));
        assert!(is_discord_webhook_url(
            "https://discordapp.com/api/webhooks/1/token"
        ));
        assert!(!is_discord_webhook_url(
            "https://example.com/api/webhooks/1/t"
        ));
        assert!(!is_discord_webhook_url("not-a-url"));
        assert!(!is_discord_webhook_url("https://discord.com/api/webhooks/"));
        assert_eq!(CONFIG_SCHEMA_VERSION, 1);
    }
}
