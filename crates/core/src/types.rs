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

    /// Everything the model was billed for.
    ///
    /// Cache reads and writes are input tokens the provider reports separately;
    /// they are not already counted in `input_tokens`.
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
    pub messages: Vec<Message>,
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

fn default_use_prompt_cache() -> bool {
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
            "azure" | "azure_openai" => "https://{resource}.openai.azure.com/openai",
            "openrouter" => "https://openrouter.ai/api/v1",
            "together" | "together_ai" => "https://api.together.xyz/v1",
            "fireworks" | "fireworks_ai" => "https://api.fireworks.ai/inference/v1",
            "mistral" => "https://api.mistral.ai/v1",
            "cohere" => "https://api.cohere.com/v1",
            "perplexity" => "https://api.perplexity.com",
            "xai" => "https://api.x.ai/v1",
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

        PermissionAction::Allow
    }

    /// Whether the tool may run at all (Allow or Ask). Deny → false.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.action_for(tool_name) != PermissionAction::Deny
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
            if tool != "bash" && tool != "shell" {
                continue;
            }
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
fn parse_shell_rule(pattern: &str) -> Option<(String, String)> {
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

/// Glob-ish match for shell rule payloads. `:` is treated as a separator like
/// Claude Code's `Bash(git:*)` form.
fn shell_arg_matches(pat: &str, command: &str) -> bool {
    let pat = pat.trim().replace(':', " ");
    let cmd = command.trim();
    if pat == "*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix('*') {
        let prefix = prefix.trim_end();
        if prefix.is_empty() {
            return true;
        }
        return cmd == prefix
            || cmd.starts_with(&format!("{prefix} "))
            || cmd.starts_with(prefix);
    }
    cmd == pat || cmd.starts_with(&format!("{pat} "))
}

/// Interpreters / shells that turn a broad allow into arbitrary code.
const DANGEROUS_SHELL_ALLOW_BASES: &[&str] = &[
    "python", "python3", "python2", "node", "deno", "tsx", "ruby", "perl", "php", "lua", "bash",
    "sh", "zsh", "fish", "eval", "exec", "env", "xargs", "sudo", "npx", "bunx", "ssh",
];

fn is_dangerous_shell_allow_pattern(arg_pat: &str) -> bool {
    let p = arg_pat
        .trim()
        .to_ascii_lowercase()
        .replace(':', " ");
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── test_message_content_as_text ────────────────────────────────────

    #[test]
    fn test_message_content_as_text_string_variant() {
        let mc = MessageContent::Text("hello world".to_string());
        assert_eq!(mc.as_text(), Some("hello world"));
    }

    #[test]
    fn test_message_content_as_text_blocks_with_text() {
        let mc = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "first block".to_string(),
            },
            ContentBlock::Text {
                text: "second block".to_string(),
            },
        ]);
        assert_eq!(mc.as_text(), Some("first block"));
    }

    #[test]
    fn test_message_content_as_text_blocks_without_text() {
        let mc = MessageContent::Blocks(vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            input: serde_json::json!({"q": "test"}),
        }]);
        assert_eq!(mc.as_text(), None);
    }

    #[test]
    fn test_message_content_as_text_empty_blocks() {
        let mc = MessageContent::Blocks(vec![]);
        assert_eq!(mc.as_text(), None);
    }

    #[test]
    fn test_message_content_text_constructor() {
        let mc = MessageContent::text("hello");
        assert_eq!(mc.as_text(), Some("hello"));
    }

    // ── test_serialize_deserialize_message ──────────────────────────────

    #[test]
    fn test_serialize_deserialize_message_text() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("hello".to_string()),
            tool_call_id: None,
            name: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::User);
        assert_eq!(deser.content.as_text(), Some("hello"));
        assert!(deser.tool_call_id.is_none());
    }

    #[test]
    fn test_serialize_deserialize_message_blocks() {
        let msg = Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "assistant reply".to_string(),
            }]),
            tool_call_id: None,
            name: Some("assistant".to_string()),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::Assistant);
        assert_eq!(deser.content.as_text(), Some("assistant reply"));
        assert_eq!(deser.name.as_deref(), Some("assistant"));
    }

    #[test]
    fn test_serialize_deserialize_message_tool() {
        let msg = Message {
            role: Role::Tool,
            content: MessageContent::Text("tool result".to_string()),
            tool_call_id: Some("call-1".to_string()),
            name: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.role, Role::Tool);
        assert_eq!(deser.tool_call_id.as_deref(), Some("call-1"));
    }

    // ── test_tool_definition_serialize ──────────────────────────────────

    #[test]
    fn test_tool_definition_serialize() {
        let td = ToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
        };

        let json = serde_json::to_string(&td).expect("serialize");
        let deser: ToolDefinition = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deser.name, "search");
        assert_eq!(deser.description, "Search the web");
        assert_eq!(deser.parameters["type"], "object");
        assert!(deser.parameters["required"][0] == "query");
    }

    // ── test_permission_set_default ─────────────────────────────────────

    #[test]
    fn test_permission_set_default() {
        let ps = PermissionSet::default();
        assert!(ps.allowed_tools.is_none());
        assert!(ps.denied_tools.is_none());
        // Default booleans are false
        assert!(!ps.allow_file_writes);
        assert!(!ps.allow_network);
        assert!(!ps.allow_shell);
        assert!(ps.allowed_paths.is_none());
        assert!(ps.rules.is_empty());
    }

    #[test]
    fn test_permission_action_for_rules() {
        let mut ps = PermissionSet {
            allow_file_writes: true,
            allow_shell: true,
            allow_network: true,
            ..Default::default()
        };
        ps.rules.insert("bash".into(), PermissionAction::Ask);
        ps.rules.insert("edit".into(), PermissionAction::Deny);
        ps.rules.insert("mymcp_*".into(), PermissionAction::Deny);

        assert_eq!(ps.action_for("bash"), PermissionAction::Ask);
        assert_eq!(ps.action_for("edit"), PermissionAction::Deny);
        assert_eq!(ps.action_for("mymcp_search"), PermissionAction::Deny);
        assert_eq!(ps.action_for("read"), PermissionAction::Allow);
        assert!(!ps.is_tool_allowed("edit"));
        assert!(ps.is_tool_allowed("bash")); // ask still appears in schema
    }

    #[test]
    fn test_role_serialization() {
        let role = Role::User;
        let json = serde_json::to_string(&role).expect("serialize");
        assert_eq!(json, "\"user\"");

        let deser: Role = serde_json::from_str("\"assistant\"").expect("deserialize");
        assert_eq!(deser, Role::Assistant);
    }

    #[test]
    fn test_provider_config_resolve_url() {
        let pc = ProviderConfig {
            name: "openai".to_string(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        };
        assert_eq!(
            pc.resolve_url("gpt-4"),
            "https://api.openai.com/v1/chat/completions"
        );

        let pc_custom = ProviderConfig {
            name: "openai".to_string(),
            api_key: None,
            api_base: None,
            base_url: Some("https://custom.example.com/api".to_string()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        };
        assert_eq!(
            pc_custom.resolve_url("gpt-4"),
            "https://custom.example.com/api/chat/completions"
        );
    }

    // ── shell-scoped permission rules ─────────────────────────────────

    fn perms_with(rules: &[(&str, PermissionAction)]) -> PermissionSet {
        let mut p = PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        };
        for (k, a) in rules {
            p.rules.insert((*k).to_string(), *a);
        }
        p
    }

    #[test]
    fn shell_rule_git_star_allows_git_commands() {
        let p = perms_with(&[("bash(git *)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("git status"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(
            p.action_for_shell("git commit -m x"),
            Some(PermissionAction::Allow)
        );
        assert_eq!(p.action_for_shell("rm -rf /"), None);
    }

    #[test]
    fn shell_rule_colon_form_matches() {
        let p = perms_with(&[("Bash(cargo:*)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("cargo test"),
            Some(PermissionAction::Allow)
        );
    }

    #[test]
    fn dangerous_interpreter_allow_coerces_to_ask() {
        let p = perms_with(&[("bash(python *)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("python script.py"),
            Some(PermissionAction::Ask)
        );
        let p = perms_with(&[("bash(node:*)", PermissionAction::Allow)]);
        assert_eq!(
            p.action_for_shell("node app.js"),
            Some(PermissionAction::Ask)
        );
    }

    #[test]
    fn shell_rule_deny_blocks_matching() {
        let p = perms_with(&[("bash(curl *)", PermissionAction::Deny)]);
        assert_eq!(
            p.action_for_shell("curl https://evil"),
            Some(PermissionAction::Deny)
        );
    }
}

#[cfg(test)]
mod usage_tests {
    use super::Usage;

    fn usage(input: u64, output: u64, created: Option<u64>, read: Option<u64>) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: created,
            cache_read_input_tokens: read,
        }
    }

    #[test]
    fn adding_accumulates_every_field() {
        let mut total = usage(10, 20, Some(1), Some(2));
        total.add(&usage(5, 7, Some(3), Some(4)));
        assert_eq!(total.input_tokens, 15);
        assert_eq!(total.output_tokens, 27);
        assert_eq!(total.cache_creation_input_tokens, Some(4));
        assert_eq!(total.cache_read_input_tokens, Some(6));
    }

    #[test]
    fn cache_stays_none_until_a_provider_reports_it() {
        // A provider without prompt caching must not make the session look
        // like it cached zero tokens — it reported nothing at all.
        let mut total = Usage::default();
        total.add(&usage(10, 20, None, None));
        assert_eq!(total.cache_creation_input_tokens, None);
        assert_eq!(total.cache_read_input_tokens, None);
    }

    #[test]
    fn the_first_report_promotes_none_to_some() {
        let mut total = usage(10, 20, None, None);
        total.add(&usage(0, 0, Some(5), Some(6)));
        assert_eq!(total.cache_creation_input_tokens, Some(5));
        assert_eq!(total.cache_read_input_tokens, Some(6));
    }

    #[test]
    fn total_counts_cache_tokens_as_well() {
        // Cache reads and writes are input tokens reported separately, not a
        // subset of input_tokens, so they add rather than overlap.
        assert_eq!(usage(10, 20, Some(3), Some(4)).total(), 37);
        assert_eq!(usage(10, 20, None, None).total(), 30);
    }

    #[test]
    fn an_untouched_usage_is_empty() {
        assert!(Usage::default().is_empty());
        assert!(usage(0, 0, Some(0), Some(0)).is_empty());
        assert!(!usage(0, 1, None, None).is_empty());
        assert!(!usage(0, 0, Some(1), None).is_empty());
    }

    #[test]
    fn accumulating_nothing_changes_nothing() {
        let mut total = usage(10, 20, Some(1), Some(2));
        let before = total.clone();
        total.add(&Usage::default());
        assert_eq!(total.input_tokens, before.input_tokens);
        assert_eq!(total.output_tokens, before.output_tokens);
        assert_eq!(
            total.cache_creation_input_tokens,
            before.cache_creation_input_tokens
        );
    }
}
