use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Role of a message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Wall-clock time this message was authored. Omitted on older sessions.
    /// Not sent to providers (`convert_messages` builds a fresh JSON object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Message {
    /// Stamp `created_at` if it is still empty (new user / assistant / tool rows).
    pub fn stamp(mut self) -> Self {
        if self.created_at.is_none() {
            self.created_at = Some(chrono::Utc::now());
        }
        self
    }
}

/// Content can be text or a list of content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Blocks(blocks) => {
                for b in blocks {
                    if let ContentBlock::Text { text } = b {
                        return Some(text);
                    }
                }
                None
            }
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }
}

/// Content blocks for structured messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: Option<bool>,
    },
    /// Anthropic / Grok extended thinking. `signature` must be echoed on the
    /// next turn or the API rejects the history (prompt-cache + interleaved).
    #[serde(rename = "thinking")]
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Opaque redacted thought (`redacted_thinking`). Echo `data` verbatim.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        data: String,
    },
}

impl ContentBlock {
    pub fn is_thinking(&self) -> bool {
        matches!(self, Self::Thinking { .. } | Self::RedactedThinking { .. })
    }
}

/// Drop trailing thinking / redacted blocks.
///
/// Anthropic: an assistant message must not *end* with thinking.
pub fn strip_trailing_thinking(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    let mut out = blocks.to_vec();
    while out.last().is_some_and(ContentBlock::is_thinking) {
        out.pop();
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    #[serde(rename = "url")]
    Url { url: String },
}

/// Tool call request from the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool call result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Streaming event from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolUseDelta {
        id: String,
        input_json_delta: String,
    },
    Thinking {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    /// Anthropic `signature_delta` — attach to the open thinking block.
    ThinkingSignature {
        signature: String,
    },
    /// Anthropic `redacted_thinking` content block.
    RedactedThinking {
        data: String,
    },
    MessageStart {
        message: Box<Message>,
    },
    MessageDelta {
        delta: serde_json::Value,
    },
    MessageStop,
    Error {
        message: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Prompt-cache accounting, reported separately because only Anthropic
    /// returns it. Folding it into `Usage` would mean every other provider
    /// carrying two fields it can never fill.
    CacheUsage {
        creation_input_tokens: u64,
        read_input_tokens: u64,
    },
}

impl Usage {
    /// Add another response's usage to this one.
    ///
    /// Cache figures stay `None` until a provider reports them, so a session
    /// against a provider without prompt caching shows nothing rather than a
    /// misleading zero.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        for (slot, value) in [
            (
                &mut self.cache_creation_input_tokens,
                other.cache_creation_input_tokens,
            ),
            (
                &mut self.cache_read_input_tokens,
                other.cache_read_input_tokens,
            ),
        ] {
            if let Some(v) = value {
                *slot = Some(slot.unwrap_or(0) + v);
            }
        }
    }

    /// Fold one streaming usage snapshot into this step.
    ///
    /// Providers send either a single final total or a running snapshot per
    /// chunk. `max` keeps Anthropic's split input/output events and does not
    /// double-count a gateway that repeats the full usage object.
    /// Distinct LLM steps still use [`Usage::add`] (session / subagent fold).
    pub fn absorb_stream(&mut self, input_tokens: u64, output_tokens: u64) {
        self.input_tokens = self.input_tokens.max(input_tokens);
        self.output_tokens = self.output_tokens.max(output_tokens);
    }

    /// Fold Anthropic-style cache figures from a stream snapshot.
    pub fn absorb_stream_cache(&mut self, created: u64, read: u64) {
        if created > 0 {
            let slot = self.cache_creation_input_tokens.get_or_insert(0);
            *slot = (*slot).max(created);
        }
        if read > 0 {
            let slot = self.cache_read_input_tokens.get_or_insert(0);
            *slot = (*slot).max(read);
        }
    }

    /// Everything the model was billed for.
    ///
    /// Cache reads and writes are **Anthropic-style additive** input tokens
    /// (not already counted in `input_tokens`). OpenAI-compatible
    /// `prompt_tokens_details.cached_tokens` is a subset of `prompt_tokens` and
    /// must **not** be stored in `cache_read_input_tokens`.
    pub fn total(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// Request to an LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub system: String,
    /// Shared transcript slice — clone of `LlmRequest` is O(1) on messages.
    /// Intent inject and other rare mutations COW via [`LlmRequest::messages_mut`].
    pub messages: std::sync::Arc<[Message]>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
    pub thinking: Option<serde_json::Value>,
    /// When true (default), providers that support inline prompt caching
    /// (Anthropic) attach OpenCode-style cache breakpoints.
    #[serde(default = "default_use_prompt_cache")]
    pub use_prompt_cache: bool,
}

impl LlmRequest {
    /// COW access for rare in-place edits (intent posture, tests).
    ///
    /// Clones the transcript only when this `Arc` is shared; unique Arcs
    /// (fresh `build_request`) mutate in place with no extra copy.
    pub fn messages_mut(&mut self) -> &mut [Message] {
        // COW: clone the transcript only when this Arc is shared. Panic-free
        // (no expect/unwrap) so the core panic budget stays at zero.
        std::sync::Arc::make_mut(&mut self.messages)
    }
}

pub(crate) fn default_use_prompt_cache() -> bool {
    true
}

/// LLM response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
    pub model: String,
}

/// Token accounting for one response or an accumulated session total.
///
/// Cache fields follow **Anthropic** semantics: they are billed and counted
/// separately from `input_tokens` (additive). OpenAI-compatible gateways report
/// cached tokens as a *subset* of `prompt_tokens` — providers must not write
/// that subset into `cache_read_input_tokens` or totals/context meters double-count.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

/// Tool/function definition for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

/// Wire encoding for OpenAI-compatible `tool_calls[].function.arguments`.
///
/// This is a **provider** property (the API's expected request shape), not
/// something inferred from model id strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgumentsFormat {
    /// OpenAI chat-completions: JSON *string* of an object (`"{\"q\":1}"`).
    #[default]
    JsonString,
    /// Bare JSON object (`{"q":1}`). Some gateways/templates require this.
    Object,
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    /// Custom base URL for the provider API (overrides api_base if set)
    #[serde(default)]
    pub base_url: Option<String>,
    /// Custom HTTP headers to include with provider requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Known model ids for this provider (optional in config.toml).
    #[serde(default)]
    pub models: Vec<String>,
    /// How this API expects `function.arguments` on assistant tool calls.
    ///
    /// `None` → OpenAI-style JSON string. Set to `object` only when the
    /// gateway requires a bare object. This is provider config, not a
    /// model-name heuristic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_arguments: Option<ToolArgumentsFormat>,
    /// Provider-specific extra fields (optional in config.toml).
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ProviderConfig {
    /// Resolved tool-arguments wire format (OpenAI JSON string when unset).
    pub fn tool_arguments_format(&self) -> ToolArgumentsFormat {
        self.tool_arguments.unwrap_or_default()
    }

    /// Resolve the full API URL for a given model.
    ///
    /// Priority: `base_url` field > `api_base` field > provider-specific default.
    pub fn resolve_url(&self, _model: &str) -> String {
        let base = self
            .base_url
            .as_deref()
            .or(self.api_base.as_deref())
            .unwrap_or_else(|| self.default_base_url());

        // Strip trailing slash, then append /chat/completions
        let base = base.trim_end_matches('/');
        format!("{}/chat/completions", base)
    }

    /// Return the well-known default base URL for this provider.
    fn default_base_url(&self) -> &str {
        match self.name.to_lowercase().as_str() {
            "openai" => "https://api.openai.com/v1",
            "anthropic" => "https://api.anthropic.com/v1",
            "groq" => "https://api.groq.com/openai/v1",
            "deepseek" => "https://api.deepseek.com/v1",
            "google" | "gemini" => "https://generativelanguage.googleapis.com/v1beta",
            "google-antigravity" => "https://daily-cloudcode-pa.googleapis.com/v1internal",
            "azure" | "azure_openai" => "https://{resource}.openai.azure.com/openai",
            "openrouter" => "https://openrouter.ai/api/v1",
            "together" | "together_ai" => "https://api.together.xyz/v1",
            "fireworks" | "fireworks_ai" => "https://api.fireworks.ai/inference/v1",
            "mistral" => "https://api.mistral.ai/v1",
            "cohere" => "https://api.cohere.com/v1",
            "perplexity" => "https://api.perplexity.com",
            "xai" => "https://api.x.ai/v1",
            "ollama" => "http://localhost:11434",
            _ => "https://api.openai.com/v1", // fallback to OpenAI-compatible
        }
    }
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_id: String,
    pub provider_id: String,
    /// Max *completion* tokens per response (sent as `max_tokens` to the API).
    pub max_tokens: Option<u32>,
    /// Context window size in tokens (used / max meter, compaction).
    ///
    /// Distinct from [`Self::max_tokens`]: that caps the reply; this is the
    /// full prompt+completion budget for the model. When `None`, the runtime
    /// falls back to a built-in catalog then `session.max_context_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub thinking: Option<bool>,
    pub supports_tools: Option<bool>,
    pub supports_images: Option<bool>,
}

/// Agent info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub mode: AgentMode,
    pub permission: PermissionSet,
    pub model: Option<ModelConfig>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

/// Session-level overlay for when to interrupt the user (issue #45).
///
/// Distinct from [`AgentMode`] (primary vs subagent) and from the build/plan/ask
/// *agent* (which tools exist). This is the authorization layer: when to prompt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    /// Auto-answer `question` and auto-allow permission `ask`.
    /// Never overrides `[permission]` deny, bash catastrophic, or sandbox/network.
    #[default]
    Auto,
    /// Prompt on `question`. Auto-allow low-risk `ask`; still prompt high-risk.
    Important,
    /// Prompt on every `question` and permission `ask` (today's TUI).
    Manual,
}

impl ApprovalMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Important, Self::Manual];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "dontask" | "don't-ask" | "dont-ask" => Some(Self::Auto),
            "important" | "ask" | "default" => Some(Self::Important),
            "manual" | "always" => Some(Self::Manual),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Important => "important",
            Self::Manual => "manual",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Important => "important",
            Self::Manual => "manual",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Auto => "Auto-answer questions and auto-allow permission asks",
            Self::Important => "Prompt on questions and high-risk tools only",
            Self::Manual => "Prompt on every question and permission ask",
        }
    }
}

impl std::fmt::Display for ApprovalMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// OpenCode-style permission action for a tool or pattern.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    /// Run without prompting
    #[default]
    Allow,
    /// Prompt the user for approval
    Ask,
    /// Block the tool
    Deny,
}

impl PermissionAction {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "allow" | "true" | "yes" => Some(Self::Allow),
            "ask" | "prompt" => Some(Self::Ask),
            "deny" | "false" | "no" | "block" => Some(Self::Deny),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionSet {
    /// Explicit allow list (if set, only these tools are candidates)
    pub allowed_tools: Option<Vec<String>>,
    /// Explicit deny list
    pub denied_tools: Option<Vec<String>>,
    pub allow_file_writes: bool,
    pub allow_network: bool,
    pub allow_shell: bool,
    pub allowed_paths: Option<Vec<PathBuf>>,
    /// OpenCode-style rules: tool name or glob pattern → allow|ask|deny.
    /// Patterns support trailing `*` (e.g. `mymcp_*`, `git *` is for bash args later).
    /// Last matching rule wins when multiple match (map iteration order is unstable;
    /// prefer more-specific keys). Exact name match always wins over patterns.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub rules: HashMap<String, PermissionAction>,
}

impl PermissionSet {
    /// Resolve the effective permission action for a tool name (OpenCode parity).
    pub fn action_for(&self, tool_name: &str) -> PermissionAction {
        // 1. Exact rule match
        if let Some(a) = self.rules.get(tool_name) {
            return *a;
        }

        // 2. Glob-style rules: `prefix*` and `*`
        let mut matched: Option<PermissionAction> = None;
        for (pattern, action) in &self.rules {
            if pattern == "*" {
                matched = Some(*action);
                continue;
            }
            if let Some(prefix) = pattern.strip_suffix('*')
                && tool_name.starts_with(prefix)
            {
                matched = Some(*action);
            }
        }
        if let Some(a) = matched {
            return a;
        }

        // 3. Legacy deny list
        if let Some(denied) = &self.denied_tools
            && denied.iter().any(|d| d == tool_name)
        {
            return PermissionAction::Deny;
        }

        // 4. Legacy allow list (if set, tools not listed are denied)
        if let Some(allowed) = &self.allowed_tools
            && !allowed.iter().any(|a| a == tool_name)
        {
            return PermissionAction::Deny;
        }

        // 5. Category flags
        if matches!(
            tool_name,
            "write" | "edit" | "apply_patch" | "todo_write" | "todowrite" | "todo"
        ) && !self.allow_file_writes
        {
            return PermissionAction::Deny;
        }
        if matches!(tool_name, "shell" | "bash") && !self.allow_shell {
            return PermissionAction::Deny;
        }
        if matches!(tool_name, "webfetch" | "websearch" | "mcp_websearch") && !self.allow_network {
            return PermissionAction::Deny;
        }
        // Real browser is outside the OS sandbox — never silent-allow.
        if tool_name == "browser" {
            return PermissionAction::Ask;
        }

        PermissionAction::Allow
    }

    /// Whether the tool may run at all (Allow or Ask). Deny → false.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.action_for(tool_name) != PermissionAction::Deny
    }

    /// Path-scoped rules for file tools: `edit(src/**)`, `write(**/*.md)`, `read(*)`.
    ///
    /// `path` should be project-relative with `/` separators when possible.
    /// Longest matching glob wins. Absolute paths outside the project are never
    /// auto-Allowed by a broad `**` (coerced to Ask).
    pub fn action_for_path(&self, tool_name: &str, path: &str) -> Option<PermissionAction> {
        let tool = tool_name.trim().to_ascii_lowercase();
        let path_norm = path.replace('\\', "/");
        let mut best: Option<(usize, PermissionAction)> = None;
        for (pattern, action) in &self.rules {
            let Some((rule_tool, glob)) = parse_path_rule(pattern) else {
                continue;
            };
            let tool_ok = rule_tool == tool
                || (rule_tool == "edit" && tool == "apply_patch")
                || (rule_tool == "write" && matches!(tool.as_str(), "edit" | "apply_patch"));
            if !tool_ok {
                continue;
            }
            if !path_glob_matches(&glob, &path_norm) {
                continue;
            }
            let mut act = *action;
            // Never silent-allow absolute system paths via write(**).
            if act == PermissionAction::Allow
                && (path_norm.starts_with('/') || path_norm.contains(".."))
                && (glob == "**" || glob == "*" || glob.ends_with("/**"))
            {
                act = PermissionAction::Ask;
            }
            let score = glob.len();
            if best.map(|(s, _)| score >= s).unwrap_or(true) {
                best = Some((score, act));
            }
        }
        best.map(|(_, a)| a)
    }

    /// Shell-scoped rules: `bash(git *)`, `shell(npm test)`, `Bash(cargo:*)`.
    ///
    /// Returns the most specific matching rule (longest pattern wins). Broad
    /// Allow patterns for interpreters (`python *`, `node:*`, …) are coerced to
    /// Ask so they cannot silently unlock arbitrary code execution.
    pub fn action_for_shell(&self, command: &str) -> Option<PermissionAction> {
        let cmd = command.trim();
        if cmd.is_empty() {
            return None;
        }
        let mut best: Option<(usize, PermissionAction)> = None;
        for (pattern, action) in &self.rules {
            let Some((tool, arg_pat)) = parse_shell_rule(pattern) else {
                continue;
            };
            debug_assert!(tool == "bash" || tool == "shell");
            let _ = tool;
            if !shell_arg_matches(&arg_pat, cmd) {
                continue;
            }
            let mut act = *action;
            if act == PermissionAction::Allow && is_dangerous_shell_allow_pattern(&arg_pat) {
                act = PermissionAction::Ask;
            }
            let score = arg_pat.len();
            if best.map(|(s, _)| score >= s).unwrap_or(true) {
                best = Some((score, act));
            }
        }
        best.map(|(_, a)| a)
    }
}

/// Parse `bash(git *)` / `shell(npm test)` → (`bash`, `git *`).
pub(crate) fn parse_shell_rule(pattern: &str) -> Option<(String, String)> {
    let pattern = pattern.trim();
    let open = pattern.find('(')?;
    let close = pattern.rfind(')')?;
    if close <= open {
        return None;
    }
    let tool = pattern[..open].trim().to_ascii_lowercase();
    if tool != "bash" && tool != "shell" {
        return None;
    }
    let inner = pattern[open + 1..close].trim().to_string();
    if inner.is_empty() {
        return None;
    }
    Some((tool, inner))
}

/// Parse `edit(src/**)` / `read(*)` → (tool, glob).
pub(crate) fn parse_path_rule(pattern: &str) -> Option<(String, String)> {
    let pattern = pattern.trim();
    let open = pattern.find('(')?;
    let close = pattern.rfind(')')?;
    if close <= open {
        return None;
    }
    let tool = pattern[..open].trim().to_ascii_lowercase();
    const PATH_TOOLS: &[&str] = &[
        "read",
        "write",
        "edit",
        "apply_patch",
        "glob",
        "list",
        "grep",
    ];
    if !PATH_TOOLS.contains(&tool.as_str()) {
        return None;
    }
    let inner = pattern[open + 1..close].trim().replace('\\', "/");
    if inner.is_empty() {
        return None;
    }
    Some((tool, inner))
}

/// Glob match for path rules.
///
/// Supports: exact, `prefix/**`, `**/name`, `*.rs`, `src/*`, `**`.
pub(crate) fn path_glob_matches(glob: &str, path: &str) -> bool {
    let glob = glob.trim_start_matches("./");
    let path = path.trim_start_matches("./");
    if glob == "*" || glob == "**" {
        return true;
    }
    if !glob.contains('*') {
        return path == glob || path.starts_with(&format!("{glob}/"));
    }
    // `src/**` → under src/
    if let Some(prefix) = glob.strip_suffix("/**") {
        let prefix = prefix.trim_end_matches('/');
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    // `**/*.rs` or `**/*`
    if let Some(suffix) = glob.strip_prefix("**/") {
        return path == suffix
            || path.ends_with(&format!("/{suffix}"))
            || match_simple_star(suffix, path.rsplit('/').next().unwrap_or(path));
    }
    // single-segment * only (e.g. `src/*.rs`, `*.md`)
    match_simple_star(glob, path)
}

/// `*` matches any run of non-`/` characters; path may include `/` only where
/// the pattern has literal `/`.
pub(crate) fn match_simple_star(pat: &str, s: &str) -> bool {
    let mut pi = 0usize;
    let mut si = 0usize;
    let pb = pat.as_bytes();
    let sb = s.as_bytes();
    let mut star_p: Option<usize> = None;
    let mut star_s = 0usize;
    while si < sb.len() {
        if pi < pb.len() && pb[pi] == b'*' {
            star_p = Some(pi);
            star_s = si;
            pi += 1;
            continue;
        }
        if pi < pb.len() && pb[pi] == sb[si] {
            pi += 1;
            si += 1;
            continue;
        }
        if let Some(sp) = star_p {
            // * cannot consume `/`
            if sb[star_s] == b'/' {
                return false;
            }
            star_s += 1;
            si = star_s;
            pi = sp + 1;
            continue;
        }
        return false;
    }
    while pi < pb.len() && pb[pi] == b'*' {
        pi += 1;
    }
    pi == pb.len()
}

/// Glob-ish match for shell rule payloads. `:` is treated as a separator like
/// Claude Code's `Bash(git:*)` form.
pub(crate) fn shell_arg_matches(pat: &str, command: &str) -> bool {
    let pat = pat.trim().replace(':', " ");
    let cmd = command.trim();
    if let Some(prefix) = pat.strip_suffix('*') {
        let prefix = prefix.trim_end();
        if prefix.is_empty() {
            return true;
        }
        return cmd == prefix || cmd.starts_with(&format!("{prefix} ")) || cmd.starts_with(prefix);
    }
    cmd == pat || cmd.starts_with(&format!("{pat} "))
}

/// Interpreters / shells that turn a broad allow into arbitrary code.
const DANGEROUS_SHELL_ALLOW_BASES: &[&str] = &[
    "python", "python3", "python2", "node", "deno", "tsx", "ruby", "perl", "php", "lua", "bash",
    "sh", "zsh", "fish", "eval", "exec", "env", "xargs", "sudo", "npx", "bunx", "ssh",
];

pub(crate) fn is_dangerous_shell_allow_pattern(arg_pat: &str) -> bool {
    let p = arg_pat.trim().to_ascii_lowercase().replace(':', " ");
    let p = p.trim();
    // Bare `*` allow on bash is full shell — never silent-allow.
    if p == "*" {
        return true;
    }
    for base in DANGEROUS_SHELL_ALLOW_BASES {
        if p == *base {
            return true;
        }
        if p == format!("{base} *") || p == format!("{base}*") {
            return true;
        }
        // `python -c *` etc.
        if p.starts_with(base)
            && p.contains('*')
            && (p.as_bytes().get(base.len()) == Some(&b' ')
                || p.as_bytes().get(base.len()) == Some(&b'*'))
        {
            return true;
        }
    }
    // Package runners that execute arbitrary scripts.
    for run in ["npm run", "yarn run", "pnpm run", "bun run"] {
        if p == run || p == format!("{run} *") || p.starts_with(&format!("{run} *")) {
            return true;
        }
    }
    false
}

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
    pub project_path: PathBuf,
}
