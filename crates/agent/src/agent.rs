use futures::StreamExt;
use std::sync::Arc;
use whycode_core::SandboxSettings;
use whycode_core::network::NetworkPolicy;
use whycode_core::tool::ToolContext;
use whycode_core::types::{
    AgentInfo, ContentBlock, PermissionAction, StreamEvent, ToolCall, ToolResult,
};
use whycode_llm::provider::ProviderRegistry;
use whycode_tools::executor::ToolExecutor;
use whycode_tools::profile::ToolProfile;

use whycode_session::session::Session;
use std::collections::VecDeque;
use std::time::Instant;

use super::events::{CancelFlag, EventSink, TurnEvent, emit, is_cancelled};
use super::permission::{PermissionPrompter, default_prompter};
use super::question::{QuestionPrompter, default_question_prompter, run_question_tool};
use super::subagent::{SubagentRunner, SubagentTask};
use super::tool_stream::ToolCallAssembler;
use whycode_command_risk::{Decision, RiskThreshold, assess, decide};
use whycode_config::HookConfig;
use whycode_plugin::hooks::{HookContext, PreHookDecision, run_post_hooks, run_pre_hooks};

/// Tool names that run an arbitrary shell command string.
const SHELL_TOOLS: &[&str] = &["bash", "shell"];

/// Soft cap for permission prompt detail (TUI wraps; avoid megabyte dumps).
const PERMISSION_DETAIL_MAX: usize = 4_000;

/// Human-readable tool arguments for the permission dialog (not compact JSON).
fn format_permission_detail(args: &serde_json::Value) -> String {
    let text = match args {
        serde_json::Value::Object(map) if map.is_empty() => "(no arguments)".to_string(),
        serde_json::Value::Object(map) => {
            // Single `command` field → show the shell string alone (most common).
            if map.len() == 1
                && let Some(cmd) = map.get("command").and_then(|v| v.as_str())
            {
                return truncate_permission_detail(cmd);
            }
            let mut lines = Vec::with_capacity(map.len());
            for (key, value) in map {
                match value {
                    serde_json::Value::String(s) => {
                        if s.contains('\n') || s.chars().count() > 72 {
                            lines.push(format!("{key}:"));
                            for line in s.lines() {
                                lines.push(format!("  {line}"));
                            }
                        } else {
                            lines.push(format!("{key}: {s}"));
                        }
                    }
                    serde_json::Value::Null => lines.push(format!("{key}: null")),
                    serde_json::Value::Bool(b) => lines.push(format!("{key}: {b}")),
                    serde_json::Value::Number(n) => lines.push(format!("{key}: {n}")),
                    other => {
                        let pretty =
                            serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string());
                        if pretty.contains('\n') {
                            lines.push(format!("{key}:"));
                            for line in pretty.lines() {
                                lines.push(format!("  {line}"));
                            }
                        } else {
                            lines.push(format!("{key}: {pretty}"));
                        }
                    }
                }
            }
            lines.join("\n")
        }
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    truncate_permission_detail(&text)
}

fn format_shell_risk_detail(command: &str, reason: &str) -> String {
    let body = format!("Command:\n{command}\n\nRisk: {reason}");
    truncate_permission_detail(&body)
}

fn truncate_permission_detail(s: &str) -> String {
    if s.chars().count() <= PERMISSION_DETAIL_MAX {
        return s.to_string();
    }
    let kept: String = s.chars().take(PERMISSION_DETAIL_MAX).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod permission_detail_tests {
    use super::{format_permission_detail, format_shell_risk_detail};
    use serde_json::json;

    #[test]
    fn single_command_is_plain_string() {
        let d = format_permission_detail(&json!({"command": "ls -la"}));
        assert_eq!(d, "ls -la");
    }

    #[test]
    fn object_keys_are_labeled_not_compact_json() {
        let d = format_permission_detail(&json!({"path": "src/main.rs", "offset": 10}));
        assert!(d.contains("path: src/main.rs"), "{d}");
        assert!(d.contains("offset: 10"), "{d}");
        assert!(!d.starts_with('{'), "must not be compact JSON: {d}");
    }

    #[test]
    fn shell_risk_has_command_and_risk_sections() {
        let d = format_shell_risk_detail("rm -rf /tmp/x", "destructive delete");
        assert!(d.contains("Command:"), "{d}");
        assert!(d.contains("rm -rf /tmp/x"), "{d}");
        assert!(d.contains("Risk: destructive delete"), "{d}");
    }
}

/// Tools that must never fan out in parallel (side effects, races, or UI ask).
const SERIAL_TOOLS: &[&str] = &[
    "bash",
    "shell",
    "write",
    "edit",
    "apply_patch",
    "git_commit",
    "todo_write",
    "todo",
    "task",
    "swarm",
    "plan",
    "question",
    "code_mode",
    "skill",
    "external_directory",
    "memory",
];

/// Whether this tool can safely run beside other tools in the same step.
///
/// Industry pattern (OpenCode issue #24764, Codex parallel function calls):
/// fan out independent reads; keep mutators and permission-gated tools serial.
fn is_parallel_safe_tool(name: &str, _permission: &whycode_core::types::PermissionSet) -> bool {
    // Mutators/shell stay serial. Permission Ask is fine in parallel now that
    // the TUI queues permission dialogs (VecDeque), not a single slot.
    !SERIAL_TOOLS.contains(&name)
}

/// Default system prompt (loaded from prompts/build.txt at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/build.txt");

/// Main agent orchestrating the conversation loop
pub struct Agent {
    pub info: AgentInfo,
    provider_registry: Arc<ProviderRegistry>,
    tool_executor: Arc<ToolExecutor>,
    permission_prompter: Arc<dyn PermissionPrompter>,
    question_prompter: Arc<dyn QuestionPrompter>,
    risk_threshold: RiskThreshold,
    sandbox: SandboxSettings,
    network: NetworkPolicy,
    /// Config-driven pre/post tool hooks (empty by default).
    hooks: Vec<HookConfig>,
    /// When session estimate exceeds this, compact before the next LLM call
    /// (Claude Code / OpenCode style). `0` disables auto-compact.
    compaction_threshold: usize,
    /// Tools schema sent to the model (`core` = smaller TTFT).
    tool_profile: ToolProfile,
    /// When false, Anthropic bodies skip cache_control markers.
    use_prompt_cache: bool,
    /// Optional fast model for trivial chat (`provider/model` or bare id).
    model_fast: Option<String>,
    /// Cross-session memory settings (from config).
    memory: whycode_memory::MemorySettings,
    /// Heuristic intent posture for build turns (`auto` / `off` / `always`).
    intent_guidance: crate::intent::IntentGuidanceMode,
    /// Parallel multi-agent swarm (config-driven).
    swarm_enabled: bool,
    swarm_max_agents: usize,
}

/// Identical tool name+args this many times in a row → refuse (OpenCode doom_loop).
const DOOM_LOOP_THRESHOLD: usize = 3;

fn tool_call_signature(tc: &ToolCall) -> String {
    let args = serde_json::to_string(&tc.arguments).unwrap_or_else(|_| "{}".into());
    format!("{}|{args}", tc.name)
}

/// True when executing `calls` would make the last N signatures all equal.
pub(crate) fn would_doom_loop(recent: &VecDeque<String>, calls: &[ToolCall]) -> bool {
    if calls.is_empty() {
        return false;
    }
    // Only treat a pure repeat batch (or single call) as doom-loop.
    let first = tool_call_signature(&calls[0]);
    if !calls.iter().all(|c| tool_call_signature(c) == first) {
        return false;
    }
    let mut n = calls.len();
    for sig in recent.iter().rev() {
        if sig == &first {
            n += 1;
            if n >= DOOM_LOOP_THRESHOLD {
                return true;
            }
        } else {
            break;
        }
    }
    n >= DOOM_LOOP_THRESHOLD
}

impl Agent {
    pub fn new(info: AgentInfo) -> Self {
        Self {
            info,
            provider_registry: Arc::new(ProviderRegistry::default()),
            tool_executor: Arc::new(ToolExecutor::new()),
            permission_prompter: default_prompter(),
            question_prompter: default_question_prompter(),
            risk_threshold: RiskThreshold::default(),
            sandbox: SandboxSettings::default(),
            network: NetworkPolicy::unrestricted(),
            hooks: Vec::new(),
            // Match config default when `with_config` is not used.
            compaction_threshold: 150_000,
            tool_profile: ToolProfile::Core,
            use_prompt_cache: true,
            model_fast: None,
            memory: whycode_memory::MemorySettings::default(),
            intent_guidance: crate::intent::IntentGuidanceMode::default(),
            swarm_enabled: true,
            swarm_max_agents: 4,
        }
    }

    pub fn with_provider_registry(mut self, registry: ProviderRegistry) -> Self {
        self.provider_registry = Arc::new(registry);
        self
    }

    pub fn with_tool_executor(mut self, executor: ToolExecutor) -> Self {
        self.tool_executor = Arc::new(executor);
        self
    }

    pub fn with_permission_prompter(mut self, prompter: Arc<dyn PermissionPrompter>) -> Self {
        self.permission_prompter = prompter;
        self
    }

    pub fn with_question_prompter(mut self, prompter: Arc<dyn QuestionPrompter>) -> Self {
        self.question_prompter = prompter;
        self
    }

    /// Load custom providers from config and merge global permission rules.
    pub fn with_config(mut self, config: &whycode_config::Config) -> Self {
        let mut registry = ProviderRegistry::default();
        registry.register_from_config(config);
        self.provider_registry = Arc::new(registry);
        self.info.permission = config.effective_permission(&self.info.permission);
        self.risk_threshold = config
            .security
            .bash_risk_threshold
            .parse()
            .unwrap_or_else(|e| {
                tracing::warn!("{e}; falling back to the default");
                RiskThreshold::default()
            });
        self.sandbox = config.security.sandbox_settings();
        self.network = config.security.network_policy();
        self.hooks = config.hooks.clone();
        self.compaction_threshold = config.session.compaction_threshold;
        self.tool_profile = ToolProfile::parse(&config.session.tool_profile);
        self.use_prompt_cache =
            !matches!(config.session.prompt_cache.trim().to_ascii_lowercase().as_str(), "none" | "off" | "false" | "0");
        self.model_fast = config.session.model_fast.clone();
        self.memory = memory_settings_from_config(config);
        self.intent_guidance =
            crate::intent::IntentGuidanceMode::parse(&config.session.intent_guidance);
        self.swarm_enabled = config.swarm.enabled;
        self.swarm_max_agents = config.swarm.max_agents.clamp(1, crate::swarm::SWARM_HARD_MAX_AGENTS);
        tracing::debug!(
            sandbox = %whycode_sandbox::describe_backend(&self.sandbox),
            network_allow = self.network.allowlist.len(),
            network_deny = self.network.denylist.len(),
            hooks = self.hooks.len(),
            compaction_threshold = self.compaction_threshold,
            tool_profile = self.tool_profile.as_str(),
            use_prompt_cache = self.use_prompt_cache,
            memory_enabled = self.memory.enabled,
            intent_guidance = ?self.intent_guidance,
            swarm_enabled = self.swarm_enabled,
            swarm_max_agents = self.swarm_max_agents,
            "shell sandbox, network policy, and hooks"
        );
        self
    }

    /// Memory settings loaded from config (for CLI/TUI helpers).
    pub fn memory_settings(&self) -> &whycode_memory::MemorySettings {
        &self.memory
    }

    pub fn with_tool_profile(mut self, profile: ToolProfile) -> Self {
        self.tool_profile = profile;
        self
    }

    pub fn model_fast(&self) -> Option<&str> {
        self.model_fast.as_deref()
    }

    /// Build tool context for a session, applying permission network flags.
    fn tool_context(&self, session: &Session) -> ToolContext {
        let mut sandbox = self.sandbox.clone();
        if !self.info.permission.allow_network {
            sandbox.network = false;
        }
        ToolContext {
            working_dir: session.project_path.to_string_lossy().to_string(),
            session_id: Some(session.id.clone()),
            sandbox,
            network: self.network.clone(),
            file_claims: None,
            agent_id: None,
            agent_label: None,
        }
    }

    /// Connect MCP servers from config and register their tools on a fresh executor.
    pub async fn with_mcp(mut self, config: &whycode_config::Config) -> Self {
        if config.mcp_servers.is_empty() {
            return self;
        }
        let mut full = ToolExecutor::new();
        let n = super::mcp_load::register_mcp_tools(&mut full, config).await;
        if n > 0 {
            self.tool_executor = Arc::new(full);
            tracing::info!(count = n, "MCP tools registered");
        }
        self
    }

    /// Get the system prompt for this agent (includes runtime context such as today's date).
    pub fn system_prompt(&self) -> String {
        let base = self
            .info
            .system_prompt
            .clone()
            .unwrap_or_else(|| Self::system_prompt_for(&self.info.name));
        Self::with_runtime_context(&base)
    }

    /// Get the system prompt for a named agent.
    ///
    /// If the agent has an explicit `system_prompt` set in its info, that wins.
    /// Otherwise falls back to loading the matching prompt file from
    /// `crates/agent/prompts/<name>.txt` at compile time.
    ///
    /// Does **not** attach AGENTS.md or runtime context — callers that build a
    /// live session should pass the result through [`Self::with_agents_md`].
    pub fn system_prompt_for(agent_name: &str) -> String {
        match agent_name {
            "build" => include_str!("../prompts/build.txt").to_string(),
            "plan" => include_str!("../prompts/plan.txt").to_string(),
            "ask" => include_str!("../prompts/ask.txt").to_string(),
            "explore" => include_str!("../prompts/explore.txt").to_string(),
            "general" => include_str!("../prompts/general.txt").to_string(),
            "scout" => include_str!("../prompts/explore.txt").to_string(),
            _ => DEFAULT_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Append runtime environment facts the model needs for time-sensitive work.
    ///
    /// Idempotent: if the prompt already contains `Today's date:`, it is returned unchanged.
    pub fn with_runtime_context(system_prompt: &str) -> String {
        if system_prompt.contains("Today's date:") {
            return system_prompt.to_string();
        }
        let today = chrono::Local::now().format("%Y-%m-%d");
        format!(
            "{system_prompt}\n\n# Environment\n\n\
             Today's date: {today}.\n\
             When searching for the current or latest version of software, do not pin the query to a past year; \
             prefer canonical sources (npm registry, GitHub Releases, official docs)."
        )
    }

    /// Append project AGENTS.md (OpenCode rules file) and runtime context to a system prompt.
    pub fn with_agents_md(system_prompt: &str, project_path: &std::path::Path) -> String {
        let candidates = [
            project_path.join("AGENTS.md"),
            project_path.join("agents.md"),
            project_path.join(".whycode").join("AGENTS.md"),
        ];
        let with_agents = {
            let mut out = None;
            for path in &candidates {
                if let Ok(content) = std::fs::read_to_string(path) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        out = Some(format!(
                            "{}\n\n# Project Instructions (AGENTS.md)\n\n{}",
                            system_prompt, trimmed
                        ));
                        break;
                    }
                }
            }
            out.unwrap_or_else(|| system_prompt.to_string())
        };
        Self::with_runtime_context(&with_agents)
    }

    /// Resolve provider + model + key for a title refine call, or `None` if
    /// the session should not refine / credentials are missing.
    fn title_refine_target(
        &self,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        title_model_override: Option<&str>,
    ) -> Option<(String, String, String, String, Option<String>)> {
        if !crate::title::should_refine_title(session) {
            return None;
        }
        let user = session.first_user_text()?;

        let (title_provider, title_model) =
            crate::title::resolve_title_model(provider_name, model, title_model_override);

        let (use_provider_name, use_model) =
            if self.provider_registry.get(&title_provider).is_some() {
                (title_provider, title_model)
            } else if self.provider_registry.get(provider_name).is_some() {
                (provider_name.to_string(), model.to_string())
            } else {
                tracing::debug!(%title_provider, "no provider for title refine");
                return None;
            };

        let key = if use_provider_name == provider_name {
            api_key.to_string()
        } else {
            std::env::var(format!("{}_API_KEY", use_provider_name.to_uppercase()))
                .unwrap_or_default()
        };
        if key.is_empty() {
            return None;
        }
        if self.provider_registry.get(&use_provider_name).is_none() {
            return None;
        }

        let assistant = session.first_assistant_snippet(400);
        Some((use_provider_name, use_model, key, user, assistant))
    }

    /// Refine the session title with a small/fast model when still auto-titleable.
    ///
    /// Uses `api_key` for the session provider; for a cross-provider
    /// `title_model` override, env `{PROVIDER}_API_KEY` is tried as a best effort.
    /// Prefer [`Self::spawn_title_refine`] in interactive UIs so the turn can
    /// finish without waiting on this secondary call.
    pub async fn maybe_refine_title(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        title_model_override: Option<&str>,
    ) {
        let Some((use_provider_name, use_model, key, user, assistant)) = self.title_refine_target(
            session,
            provider_name,
            model,
            api_key,
            title_model_override,
        ) else {
            return;
        };

        let Some(provider) = self.provider_registry.get(&use_provider_name) else {
            return;
        };

        match crate::title::generate_title(
            provider,
            &key,
            &use_model,
            &user,
            assistant.as_deref(),
        )
        .await
        {
            Ok(title) => crate::title::apply_refine_result(session, &title, &use_model),
            Err(e) => {
                tracing::debug!(error = %e, "session title refine failed");
            }
        }
    }

    /// Fire-and-forget title refine. Sends `(session_id, title)` on `title_tx`
    /// when ready. Returns `true` if a background task was spawned.
    ///
    /// Does not hold the session lock — callers apply the title when the
    /// channel delivers (TUI main loop). Skips trivial greetings and sessions
    /// that already have a manual/generated title.
    ///
    /// The session id is included so a late title is never applied to a
    /// different session (or the TUI placeholder held while the turn runs).
    pub fn spawn_title_refine(
        &self,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        title_model_override: Option<&str>,
        title_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    ) -> bool {
        let Some((use_provider_name, use_model, key, user, assistant)) = self.title_refine_target(
            session,
            provider_name,
            model,
            api_key,
            title_model_override,
        ) else {
            return false;
        };

        let session_id = session.id.clone();
        // ProviderRegistry is Arc-backed on the agent; clone the whole registry
        // handle so the task outlives this method without needing the Agent.
        let registry = Arc::clone(&self.provider_registry);
        tokio::spawn(async move {
            let Some(provider) = registry.get(&use_provider_name) else {
                return;
            };
            match crate::title::generate_title(
                provider,
                &key,
                &use_model,
                &user,
                assistant.as_deref(),
            )
            .await
            {
                Ok(title) if !title.is_empty() => {
                    tracing::debug!(%title, model = %use_model, "session title refined (async)");
                    let _ = title_tx.send((session_id, title));
                }
                Ok(_) => {
                    tracing::debug!("title model returned empty; keeping heuristic/default");
                }
                Err(e) => {
                    tracing::debug!(error = %e, "session title refine failed (async)");
                }
            }
        });
        true
    }

    /// Run a single conversation turn (no streaming UI events).
    pub async fn run_turn(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
    ) -> whycode_core::Result<String> {
        self.run_turn_with_events(
            session,
            provider_name,
            model,
            api_key,
            max_turns,
            None,
            None,
        )
        .await
    }

    /// Run a turn, optionally streaming `TurnEvent`s and honouring a cancel flag (Esc).
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn_with_events(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
        events: Option<EventSink>,
        cancel: Option<CancelFlag>,
    ) -> whycode_core::Result<String> {
        // Trivial chit-chat: omit tools entirely (huge prefill savings).
        // Only on short single-user sessions — once tools were used, keep them.
        let last_user = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == whycode_core::types::Role::User)
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .unwrap_or_default();
        let tools_free_chat = crate::title::is_trivial_title_seed(&last_user)
            && session.user_message_count() <= 1
            && !session.messages.iter().any(|m| {
                matches!(m.role, whycode_core::types::Role::Tool)
                    || matches!(
                        &m.content,
                        whycode_core::types::MessageContent::Blocks(b)
                            if b.iter().any(|x| matches!(x, ContentBlock::ToolUse { .. }))
                    )
            });

        let tools = if tools_free_chat {
            tracing::debug!("trivial chat — omitting tools from request for TTFT");
            Vec::new()
        } else {
            let mut defs = self
                .tool_executor
                .get_definitions_profile(&self.info.permission, self.tool_profile);
            // Hide swarm when disabled so the model does not invent fan-out.
            if !self.swarm_enabled {
                defs.retain(|d| d.name != "swarm");
            }
            defs
        };

        // Classify once per user turn (zero LLM cost): badge, posture, tool auth.
        let turn_intent = crate::intent::classify_user_intent(&last_user);
        {
            let badge = crate::intent::badge_label(&turn_intent)
                .unwrap_or("")
                .to_string();
            let (notice_kind, notice) =
                match crate::intent::intent_notice(&turn_intent, &self.info.name) {
                    Some(n) => {
                        let k = match n.kind {
                            crate::intent::IntentNoticeKind::Info => "info",
                            crate::intent::IntentNoticeKind::Warning => "warning",
                        };
                        (k.to_string(), n.message)
                    }
                    None => (String::new(), String::new()),
                };
            emit(
                &events,
                TurnEvent::Intent {
                    kind: turn_intent.intent.as_str().to_string(),
                    confidence: turn_intent.confidence,
                    badge,
                    notice_kind,
                    notice,
                },
            );
        }

        let tool_ctx = self.tool_context(session);

        let provider = self
            .provider_registry
            .get(provider_name)
            .ok_or_else(|| {
                whycode_core::Error::Llm(format!(
                    "Unknown provider: {}. Available: anthropic, openai, google, and configured custom providers",
                    provider_name
                ))
            })?;

        let mut turn_count = 0;
        let mut final_text = String::new();
        // Latency: wall clock for the whole user turn (all LLM steps + tools).
        let user_turn_t0 = Instant::now();
        let mut ttft_ms: Option<u128> = None;
        // Recent tool signatures for OpenCode-style doom-loop detection.
        let mut recent_tool_sigs: VecDeque<String> = VecDeque::with_capacity(8);

        loop {
            if is_cancelled(&cancel) {
                emit(&events, TurnEvent::Cancelled);
                return Err(whycode_core::Error::Agent("Cancelled".into()));
            }

            turn_count += 1;
            if turn_count > max_turns {
                return Err(whycode_core::Error::Agent(format!(
                    "Exceeded maximum turns ({})",
                    max_turns
                )));
            }

            // Always shrink oversized / old tool dumps before prefill (cheap).
            // Full compact only when over the configured token threshold.
            let _ = session.truncate_large_tool_results();
            let _ = session.prune_old_tool_results();
            if self.compaction_threshold > 0 {
                let before = session.token_count();
                if before > self.compaction_threshold {
                    let n_before = session.messages.len();
                    session.compact(self.compaction_threshold);
                    let n_after = session.messages.len();
                    if n_after < n_before {
                        emit(
                            &events,
                            TurnEvent::Status(format!(
                                "Compacted context ({n_before} → {n_after} msgs)…"
                            )),
                        );
                        tracing::info!(
                            before_tokens = before,
                            messages_before = n_before,
                            messages_after = n_after,
                            "auto-compact before LLM step"
                        );
                    }
                }
            }

            emit(
                &events,
                TurnEvent::Status(format!("LLM request (step {turn_count})…")),
            );

            let mut request =
                session.build_request(&tools, None, self.info.temperature, Some(true));
            request.use_prompt_cache = self.use_prompt_cache;

            // First LLM step: ephemeral intent posture (not stored in session;
            // keeps system prompt cache-stable). Notice is already on Intent event.
            if turn_count == 1 {
                if crate::intent::should_inject(self.intent_guidance, &turn_intent) {
                    if let Some(suffix) =
                        crate::intent::posture_suffix(&turn_intent, &self.info.name)
                    {
                        // Append to last user message in the request only.
                        use whycode_core::types::{MessageContent, Role};
                        for msg in request.messages.iter_mut().rev() {
                            if msg.role != Role::User {
                                continue;
                            }
                            match &mut msg.content {
                                MessageContent::Text(t) => t.push_str(&suffix),
                                MessageContent::Blocks(blocks) => {
                                    blocks.push(ContentBlock::Text {
                                        text: suffix.clone(),
                                    });
                                }
                            }
                            tracing::debug!(
                                intent = turn_intent.intent.as_str(),
                                confidence = turn_intent.confidence,
                                agent = %self.info.name,
                                "intent posture injected into request"
                            );
                            break;
                        }
                    }
                }
            }

            let mut accumulated_text = String::new();
            let mut turn_usage = whycode_core::types::Usage::default();
            let mut assembler = ToolCallAssembler::new();
            let step_t0 = Instant::now();

            // Professional transport: classify + full-jitter backoff + Retry-After.
            // Only the HTTP open is retried — mid-stream drops stay single-shot.
            let mut event_stream = whycode_llm::default_transport()
                .stream(provider, &request, api_key, model)
                .await?;

            while let Some(event) = event_stream.next().await {
                if is_cancelled(&cancel) {
                    // Persist partial assistant text before aborting
                    if !accumulated_text.is_empty() {
                        session.add_assistant_message(vec![ContentBlock::Text {
                            text: accumulated_text.clone(),
                        }]);
                        final_text.push_str(&accumulated_text);
                    }
                    emit(&events, TurnEvent::Cancelled);
                    return Err(whycode_core::Error::Agent("Cancelled".into()));
                }

                match event? {
                    StreamEvent::TextDelta { text } => {
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        emit(&events, TurnEvent::TextDelta(text.clone()));
                        accumulated_text.push_str(&text);
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        // Defer ToolStart until after argument fragments are
                        // merged — OpenAI streams send null/empty args first.
                        assembler.on_tool_use(id, name, input);
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        assembler.on_tool_use_delta(&id, &input_json_delta);
                    }
                    StreamEvent::Thinking { text } => {
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::debug!("Thinking: {}", text);
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::debug!("Thinking: {}", text);
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        // Providers report these in pieces — Anthropic sends
                        // input at message_start and output at message_delta —
                        // so accumulate rather than replace.
                        turn_usage.input_tokens += input_tokens;
                        turn_usage.output_tokens += output_tokens;
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        *turn_usage.cache_creation_input_tokens.get_or_insert(0) +=
                            creation_input_tokens;
                        *turn_usage.cache_read_input_tokens.get_or_insert(0) += read_input_tokens;
                    }
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        return Err(whycode_core::Error::Llm(message));
                    }
                }
            }

            // Merge streamed argument fragments into parsed JSON objects.
            let tool_calls = assembler.finish();
            let step_ms = step_t0.elapsed().as_millis();

            // Emit ToolStart with final parsed arguments (not the empty first chunk).
            for tc in &tool_calls {
                if ttft_ms.is_none() {
                    ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                }
                emit(
                    &events,
                    TurnEvent::ToolStart {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    },
                );
            }

            // Once per turn, after the stream closes and before any tool runs.
            // A provider that reports nothing produces no event, so a silent
            // provider is distinguishable from a zero-cost turn.
            if !turn_usage.is_empty() {
                session.add_usage(&turn_usage);
                emit(&events, TurnEvent::Usage(turn_usage.clone()));
            }

            let mut blocks: Vec<ContentBlock> = Vec::new();

            if !accumulated_text.is_empty() {
                blocks.push(ContentBlock::Text {
                    text: accumulated_text.clone(),
                });
                final_text.push_str(&accumulated_text);
            }

            for tc in &tool_calls {
                blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                });
            }

            // Never persist an empty assistant turn — strict OpenAI-compatible
            // APIs reject assistant messages with no text/tool_calls.
            if !blocks.is_empty() {
                session.add_assistant_message(blocks);
            }

            if tool_calls.is_empty() {
                whycode_core::logging::emit_sid(
                    "agent",
                    "info",
                    "turn.step",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({
                        "step": turn_count,
                        "step_ms": step_ms,
                        "ttft_ms": ttft_ms,
                        "tool_batch_ms": null,
                        "tool_count": 0,
                        "tools_profile": self.tool_profile.as_str(),
                        "input_tokens": turn_usage.input_tokens,
                        "output_tokens": turn_usage.output_tokens,
                        "cache_read_tokens": turn_usage.cache_read_input_tokens,
                        "cache_creation_tokens": turn_usage.cache_creation_input_tokens,
                        "done": true,
                    })),
                );
                break;
            }

            // Doom-loop: refuse identical tool+args repeated DOOM_LOOP_THRESHOLD times
            // (OpenCode processor.ts doom_loop permission pattern).
            let results = if would_doom_loop(&recent_tool_sigs, &tool_calls) {
                emit(
                    &events,
                    TurnEvent::Status(
                        "Doom loop: identical tool call repeated — refusing".into(),
                    ),
                );
                tracing::warn!(
                    tools = ?tool_calls.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
                    "doom loop refused"
                );
                let mut refused = Vec::with_capacity(tool_calls.len());
                for tc in &tool_calls {
                    emit(
                        &events,
                        TurnEvent::ToolEnd {
                            id: tc.id.clone(),
                            content: format!(
                                "Doom loop: tool `{}` with the same arguments was repeated \
                                 {DOOM_LOOP_THRESHOLD}+ times. Stop retrying; change approach \
                                 or ask the user.",
                                tc.name
                            ),
                            is_error: true,
                        },
                    );
                    refused.push(ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Doom loop: tool `{}` with the same arguments was repeated \
                             {DOOM_LOOP_THRESHOLD}+ times. Stop retrying; change approach \
                             or ask the user.",
                            tc.name
                        ),
                        is_error: true,
                    });
                    let sig = tool_call_signature(tc);
                    recent_tool_sigs.push_back(sig);
                    while recent_tool_sigs.len() > 16 {
                        recent_tool_sigs.pop_front();
                    }
                }
                refused
            } else {
                // Parallel when safe (OpenCode / Codex / Claude Code pattern).
                // Sequential for shell / mutating / permission-ask tools so risk
                // gates and the TUI single-slot permission UI stay correct.
                let tool_t0 = Instant::now();
                let results = self
                    .execute_tool_calls(
                        &tool_calls,
                        session,
                        &tool_ctx,
                        provider_name,
                        model,
                        api_key,
                        &events,
                        &cancel,
                        Some(&turn_intent),
                    )
                    .await?;
                let tool_batch_ms = tool_t0.elapsed().as_millis();
                for tc in &tool_calls {
                    let sig = tool_call_signature(tc);
                    recent_tool_sigs.push_back(sig);
                    while recent_tool_sigs.len() > 16 {
                        recent_tool_sigs.pop_front();
                    }
                }
                whycode_core::logging::emit_sid(
                    "agent",
                    "info",
                    "turn.step",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({
                        "step": turn_count,
                        "step_ms": step_ms,
                        "ttft_ms": ttft_ms,
                        "tool_batch_ms": tool_batch_ms,
                        "tool_count": tool_calls.len(),
                        "tools_profile": self.tool_profile.as_str(),
                        "input_tokens": turn_usage.input_tokens,
                        "output_tokens": turn_usage.output_tokens,
                        "cache_read_tokens": turn_usage.cache_read_input_tokens,
                        "cache_creation_tokens": turn_usage.cache_creation_input_tokens,
                        "done": false,
                    })),
                );
                results
            };

            // Capture failures before move — avoid cloning large tool bodies.
            let failed_tools: Vec<String> = results
                .iter()
                .filter(|r| r.is_error)
                .map(|r| {
                    format!(
                        "The tool failed with error: {content}. Please correct your approach.",
                        content = r.content
                    )
                })
                .collect();

            session.add_tool_results(results);

            if !failed_tools.is_empty() {
                let recovery_msg = failed_tools.join("\n");
                session.add_user_message(&recovery_msg);
            }
        }

        whycode_core::logging::emit_sid(
            "agent",
            "info",
            "turn.done",
            Some(session.id.as_str()),
            Some(serde_json::json!({
                "steps": turn_count,
                "ttft_ms": ttft_ms,
                "worked_ms": user_turn_t0.elapsed().as_millis(),
                "tools_profile": self.tool_profile.as_str(),
            })),
        );

        // Hindsight-style auto-retain (heuristic + optional LLM). Best-effort
        // and **async** — never await here. LLM extract can take 5–12s and used
        // to keep the TUI on `generating` after the answer was already on screen
        // (same pitfall as title refine; see docs/KNOWHOW.md).
        super::memory_retain::spawn_post_turn_retain(
            session,
            &final_text,
            &self.memory,
            Arc::clone(&self.provider_registry),
            provider_name,
            model,
            api_key,
            events,
        );

        Ok(final_text)
    }

    /// Run a batch of tool calls, parallelizing independent read-only tools.
    ///
    /// Results are returned in the **same order** as `tool_calls` (required by
    /// the messages API). Shell, mutators, and tools that need an interactive
    /// permission ask stay sequential so risk/UI semantics stay single-threaded.
    #[allow(clippy::too_many_arguments)]
    async fn execute_tool_calls(
        &self,
        tool_calls: &[ToolCall],
        session: &Session,
        tool_ctx: &ToolContext,
        provider_name: &str,
        model: &str,
        api_key: &str,
        events: &Option<EventSink>,
        cancel: &Option<CancelFlag>,
        turn_intent: Option<&crate::intent::IntentAssessment>,
    ) -> whycode_core::Result<Vec<ToolResult>> {
        if tool_calls.is_empty() {
            return Ok(Vec::new());
        }

        // Single call — no fan-out overhead.
        if tool_calls.len() == 1 {
            let tc = &tool_calls[0];
            if is_cancelled(cancel) {
                emit(events, TurnEvent::Cancelled);
                return Err(whycode_core::Error::Agent("Cancelled".into()));
            }
            emit(
                events,
                TurnEvent::Status(format!("Running tool `{}`…", tc.name)),
            );
            let result = self
                .execute_with_permission(
                    tc,
                    session,
                    tool_ctx,
                    provider_name,
                    model,
                    api_key,
                    turn_intent,
                    events.as_ref(),
                )
                .await;
            emit(
                events,
                TurnEvent::ToolEnd {
                    id: tc.id.clone(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            );
            return Ok(vec![result]);
        }

        let all_parallel = tool_calls
            .iter()
            .all(|tc| is_parallel_safe_tool(&tc.name, &self.info.permission));

        if all_parallel {
            let names: Vec<&str> = tool_calls.iter().map(|t| t.name.as_str()).collect();
            emit(
                events,
                TurnEvent::Status(format!(
                    "Running {} tools in parallel: {}…",
                    tool_calls.len(),
                    names.join(", ")
                )),
            );
            // ToolStart already emitted by the caller for every call.
            let futs: Vec<_> = tool_calls
                .iter()
                .map(|tc| {
                    self.execute_with_permission(
                        tc,
                        session,
                        tool_ctx,
                        provider_name,
                        model,
                        api_key,
                        turn_intent,
                        events.as_ref(),
                    )
                })
                .collect();
            let results = futures::future::join_all(futs).await;
            for (tc, result) in tool_calls.iter().zip(results.iter()) {
                emit(
                    events,
                    TurnEvent::ToolEnd {
                        id: tc.id.clone(),
                        content: result.content.clone(),
                        is_error: result.is_error,
                    },
                );
            }
            return Ok(results);
        }

        // Mixed or unsafe batch — sequential (correct + simple).
        let mut results = Vec::with_capacity(tool_calls.len());
        for tc in tool_calls {
            if is_cancelled(cancel) {
                emit(events, TurnEvent::Cancelled);
                return Err(whycode_core::Error::Agent("Cancelled".into()));
            }
            emit(
                events,
                TurnEvent::Status(format!("Running tool `{}`…", tc.name)),
            );
            let result = self
                .execute_with_permission(
                    tc,
                    session,
                    tool_ctx,
                    provider_name,
                    model,
                    api_key,
                    turn_intent,
                    events.as_ref(),
                )
                .await;
            emit(
                events,
                TurnEvent::ToolEnd {
                    id: tc.id.clone(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            );
            results.push(result);
        }
        Ok(results)
    }

    /// Apply the shell risk gate, then allow/ask/deny, then execute (or spawn
    /// a task subagent).
    ///
    /// `pub(crate)` so the risk gate can be tested at this level: the unit
    /// tests in `command-risk` cover classification, but only this method
    /// proves that a catastrophic command is refused even when the permission
    /// map says `allow`.
    pub(crate) async fn execute_with_permission(
        &self,
        tc: &ToolCall,
        session: &Session,
        tool_ctx: &ToolContext,
        provider_name: &str,
        model: &str,
        api_key: &str,
        turn_intent: Option<&crate::intent::IntentAssessment>,
        events: Option<&EventSink>,
    ) -> ToolResult {
        // Questionnaire: UI-backed channel (TUI) or stdin/auto — never race in
        // parallel with other tools (SERIAL_TOOLS). Skip permission map; asking
        // the user *is* the interaction.
        if tc.name == "question" {
            return run_question_tool(
                self.question_prompter.as_ref(),
                &tc.arguments,
                &tc.id,
            )
            .await;
        }

        // Shell commands are gated on what the command would destroy. The
        // permission map below only sees the tool name, so on its own `allow`
        // would run anything the model emits.
        let mut risk_confirmed = false;
        if SHELL_TOOLS.contains(&tc.name.as_str()) {
            let command = tc
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let assessment = assess(command, std::path::Path::new(&tool_ctx.working_dir));

            match decide(&assessment, self.risk_threshold) {
                Decision::Allow => {}
                Decision::Refuse { reason } => {
                    tracing::warn!(command, reason, "refused catastrophic shell command");
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Refused: {reason}.\n\
                             This command is classified catastrophic and cannot be approved. \
                             Run it yourself if you are certain."
                        ),
                        is_error: true,
                    };
                }
                Decision::Confirm { reason } => {
                    // Structured for the TUI permission dialog (see format_permission_detail).
                    let detail = format_shell_risk_detail(command, &reason);
                    if !self.permission_prompter.ask(&tc.name, &detail).await {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!("User denied permission for tool '{}'.", tc.name),
                            is_error: true,
                        };
                    }
                    risk_confirmed = true;
                }
            }
        }

        // Intent authorization (Claude-style): question/plan turns must not
        // silently mutate. Runs after blast-radius risk, before permission map.
        if let Some(intent) = turn_intent {
            let command = tc
                .arguments
                .get("command")
                .and_then(|v| v.as_str());
            match crate::intent::authorize_tool(
                intent,
                &self.info.name,
                &tc.name,
                command,
                self.intent_guidance,
            ) {
                crate::intent::ToolAuthDecision::Allow => {}
                crate::intent::ToolAuthDecision::Refuse { reason } => {
                    tracing::info!(
                        tool = %tc.name,
                        intent = intent.intent.as_str(),
                        "intent auth refused tool"
                    );
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!("Refused (intent): {reason}"),
                        is_error: true,
                    };
                }
                crate::intent::ToolAuthDecision::Confirm { reason } => {
                    if !risk_confirmed {
                        let detail = format_permission_detail(&tc.arguments);
                        let body = format!("{detail}\n\nIntent check:\n{reason}");
                        if !self.permission_prompter.ask(&tc.name, &body).await {
                            return ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: format!(
                                    "User denied permission for tool '{}' (intent gate).",
                                    tc.name
                                ),
                                is_error: true,
                            };
                        }
                        risk_confirmed = true;
                    }
                }
            }
        }

        match self.info.permission.action_for(&tc.name) {
            PermissionAction::Deny => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!(
                        "Permission denied for tool '{}'. Adjust agent permissions or config.permission.",
                        tc.name
                    ),
                    is_error: true,
                };
            }
            // Already confirmed with the command in hand; do not ask twice.
            PermissionAction::Ask if risk_confirmed => {}
            PermissionAction::Ask => {
                let detail = format_permission_detail(&tc.arguments);
                let allowed = self.permission_prompter.ask(&tc.name, &detail).await;
                if !allowed {
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!("User denied permission for tool '{}'.", tc.name),
                        is_error: true,
                    };
                }
            }
            PermissionAction::Allow => {}
        }

        // Pre-tool hooks (after risk + permission, before execution).
        let tool_input = tc.arguments.to_string();
        let pre_ctx = HookContext::pre(
            tc.name.clone(),
            tc.id.clone(),
            tool_input.clone(),
            Some(session.id.clone()),
            tool_ctx.working_dir.clone(),
        );
        match run_pre_hooks(&self.hooks, &pre_ctx).await {
            PreHookDecision::Allow => {}
            PreHookDecision::Block { reason } => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: reason,
                    is_error: true,
                };
            }
        }

        let result = if tc.name == "task" {
            self.execute_task_tool(tc, session, provider_name, model, api_key)
                .await
        } else if tc.name == "swarm" {
            self.execute_swarm_tool(tc, session, provider_name, model, api_key, events)
                .await
        } else {
            self.tool_executor
                .execute(tc, tool_ctx, &self.info.permission)
                .await
        };

        // Post-tool hooks never block; failures are logged inside the runner.
        let post_ctx = HookContext::post(
            tc.name.clone(),
            tc.id.clone(),
            tool_input,
            Some(session.id.clone()),
            tool_ctx.working_dir.clone(),
            result.is_error,
            &result.content,
        );
        run_post_hooks(&self.hooks, &post_ctx).await;

        result
    }

    /// Execute the `swarm` tool: parallel subagents + file-claim conflict notify.
    async fn execute_swarm_tool(
        &self,
        call: &ToolCall,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        events: Option<&EventSink>,
    ) -> whycode_core::types::ToolResult {
        use std::time::Instant;
        use whycode_core::types::{AgentInfo, AgentMode, PermissionSet, ToolResult};
        use whycode_core::{ClaimResult, FileClaimRegistry};
        use tokio::sync::Semaphore;

        if !self.swarm_enabled {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "swarm is disabled (`[swarm] enabled = false` in config).".into(),
                is_error: true,
            };
        }

        let specs = match crate::swarm::parse_swarm_tasks(&call.arguments) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    tool_call_id: call.id.clone(),
                    content: e,
                    is_error: true,
                };
            }
        };

        let max_from_args = call
            .arguments
            .get("max_concurrent")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let max_concurrent = max_from_args
            .unwrap_or(self.swarm_max_agents)
            .clamp(1, crate::swarm::SWARM_HARD_MAX_AGENTS)
            .min(specs.len())
            .max(1);

        let claims = FileClaimRegistry::new();
        if let Some(tx) = events {
            let tx = tx.clone();
            claims.set_listener(Some(std::sync::Arc::new(move |ev| {
                let _ = tx.send(TurnEvent::FileConflict {
                    path: ev.path,
                    claimant: ev.claimant_label,
                    owner: ev.owner_label,
                });
            })));
        }

        let wall_t0 = Instant::now();
        let total = specs.len();
        emit(
            &events.cloned(),
            TurnEvent::SwarmStatus {
                active: 0,
                total,
                message: format!("Starting swarm: {total} workers (max {max_concurrent} concurrent)…"),
            },
        );

        // Pre-claim optional paths so conflicts surface before work starts.
        for (i, spec) in specs.iter().enumerate() {
            let worker_id = format!("worker-{i}");
            let label = format!("{worker_id}/{}", spec.subagent_type);
            for rel in &spec.paths {
                let full = if std::path::Path::new(rel).is_absolute() {
                    std::path::PathBuf::from(rel)
                } else {
                    session.project_path.join(rel)
                };
                match claims.try_claim(&worker_id, &label, &full) {
                    ClaimResult::Acquired | ClaimResult::Held => {}
                    ClaimResult::Conflict {
                        owner_label,
                        owner_id: _,
                    } => {
                        if let Some(tx) = events {
                            let _ = tx.send(TurnEvent::FileConflict {
                                path: full.display().to_string(),
                                claimant: label.clone(),
                                owner: owner_label.clone(),
                            });
                        }
                        return ToolResult {
                            tool_call_id: call.id.clone(),
                            content: format!(
                                "Pre-claim conflict: `{rel}` for {label} is already claimed by `{owner_label}`. \
                                 Give each worker disjoint `paths`."
                            ),
                            is_error: true,
                        };
                    }
                }
            }
        }

        let sem = std::sync::Arc::new(Semaphore::new(max_concurrent));
        let provider_name: std::sync::Arc<str> = provider_name.into();
        let model: std::sync::Arc<str> = model.into();
        let api_key: std::sync::Arc<str> = api_key.into();
        let project_path = session.project_path.clone();
        let registry = Arc::clone(&self.provider_registry);
        let executor = Arc::clone(&self.tool_executor);
        let sandbox = self.sandbox.clone();
        let network = self.network.clone();
        let memory = self.memory.clone();
        let parent_permission = self.info.permission.clone();
        let agents_md_path = session.project_path.clone();

        let mut handles = Vec::with_capacity(specs.len());

        for (i, spec) in specs.into_iter().enumerate() {
            let worker_id = format!("worker-{i}");
            let label = format!("{worker_id}/{}", spec.subagent_type);
            let permit = Arc::clone(&sem);
            let pn = Arc::clone(&provider_name);
            let m = Arc::clone(&model);
            let ak = Arc::clone(&api_key);
            let claims = claims.clone();
            let registry = Arc::clone(&registry);
            let executor = Arc::clone(&executor);
            let sandbox = sandbox.clone();
            let network = network.clone();
            let memory = memory.clone();
            let parent_permission = parent_permission.clone();
            let project_path = project_path.clone();
            let agents_md_path = agents_md_path.clone();
            let events_tx = events.cloned();

            handles.push(tokio::spawn(async move {
                let _guard = match permit.acquire().await {
                    Ok(g) => g,
                    Err(_) => {
                        return (
                            worker_id,
                            spec.subagent_type,
                            spec.goal,
                            false,
                            0.0,
                            "Semaphore closed".to_string(),
                        );
                    }
                };
                if let Some(ref tx) = events_tx {
                    let _ = tx.send(TurnEvent::SwarmStatus {
                        active: 0,
                        total,
                        message: format!("Swarm {label}: running…"),
                    });
                }

                let (permission, system_prompt) = match spec.subagent_type.as_str() {
                    "explore" | "scout" => (
                        PermissionSet {
                            allowed_tools: Some(vec![
                                "read".into(),
                                "grep".into(),
                                "glob".into(),
                                "list".into(),
                                "webfetch".into(),
                                "websearch".into(),
                                "lsp".into(),
                            ]),
                            denied_tools: Some(vec![
                                "write".into(),
                                "edit".into(),
                                "shell".into(),
                                "bash".into(),
                                "apply_patch".into(),
                                "todowrite".into(),
                                "todo".into(),
                                "task".into(),
                                "swarm".into(),
                            ]),
                            allow_file_writes: false,
                            allow_network: true,
                            allow_shell: false,
                            allowed_paths: None,
                            rules: Default::default(),
                        },
                        Agent::system_prompt_for(&spec.subagent_type),
                    ),
                    _ => {
                        let mut perm = parent_permission;
                        let mut denied = perm.denied_tools.unwrap_or_default();
                        for t in [
                            "todowrite",
                            "todo",
                            "todoread",
                            "task",
                            "swarm",
                        ] {
                            if !denied.iter().any(|x| x == t) {
                                denied.push(t.to_string());
                            }
                        }
                        perm.denied_tools = Some(denied);
                        (perm, Agent::system_prompt_for("general"))
                    }
                };

                let mut info = AgentInfo {
                    name: spec.subagent_type.clone(),
                    description: format!("Swarm worker {worker_id}"),
                    mode: AgentMode::Subagent,
                    permission,
                    model: None,
                    system_prompt: Some(Agent::with_agents_md(&system_prompt, &agents_md_path)),
                    temperature: None,
                    top_p: None,
                };
                // Keep name as type for memory banks; label is separate for claims.
                let _ = &mut info;

                let claim_note = if spec.paths.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nYou own these paths exclusively for this swarm run: {}.\
                         \nDo not edit other workers' files.",
                        spec.paths.join(", ")
                    )
                };
                let context = match spec.context {
                    Some(c) => Some(format!("{c}{claim_note}")),
                    None if claim_note.is_empty() => None,
                    None => Some(claim_note.trim().to_string()),
                };

                let task = SubagentTask {
                    goal: spec.goal.clone(),
                    context,
                    tools: None,
                    max_turns: spec.max_turns,
                };

                let runner = SubagentRunner::new(
                    registry,
                    executor,
                    info,
                    project_path,
                    sandbox,
                    network,
                )
                .with_memory(memory)
                .with_file_claims(claims.clone(), worker_id.clone(), label.clone());

                let t0 = Instant::now();
                let result = runner.run(task, &pn, &m, &ak).await;
                let secs = t0.elapsed().as_secs_f64();
                claims.release_agent(&worker_id);

                match result {
                    Ok(r) => (
                        worker_id,
                        spec.subagent_type,
                        spec.goal,
                        r.success,
                        secs,
                        r.output,
                    ),
                    Err(e) => (
                        worker_id,
                        spec.subagent_type,
                        spec.goal,
                        false,
                        secs,
                        format!("Swarm worker error: {e}"),
                    ),
                }
            }));
        }

        let mut sections = Vec::with_capacity(handles.len());
        let mut ok = 0usize;
        for handle in handles {
            match handle.await {
                Ok((id, kind, goal, success, secs, body)) => {
                    if success {
                        ok += 1;
                    }
                    sections.push(crate::swarm::format_worker_report(
                        &id, &kind, success, secs, &goal, &body,
                    ));
                }
                Err(e) => {
                    sections.push(format!("### worker join error\n\n{e}\n"));
                }
            }
        }

        claims.clear();
        let wall = wall_t0.elapsed().as_secs_f64();
        emit(
            &events.cloned(),
            TurnEvent::SwarmStatus {
                active: 0,
                total,
                message: format!("Swarm done: {ok}/{total} ok in {wall:.1}s"),
            },
        );

        let mut report = crate::swarm::format_swarm_header(total, ok, wall);
        report.push('\n');
        report.push_str(&sections.join("\n"));

        let claim_snap = claims.snapshot();
        if !claim_snap.is_empty() {
            // cleared above; left for completeness if release missed anything
        }
        let _ = claim_snap;

        ToolResult {
            tool_call_id: call.id.clone(),
            content: report,
            is_error: ok == 0,
        }
    }

    /// Execute the `task` tool by spawning a real subagent (OpenCode Task tool parity).
    async fn execute_task_tool(
        &self,
        call: &ToolCall,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> whycode_core::types::ToolResult {
        use whycode_core::types::{AgentMode, PermissionSet, ToolResult};

        let goal = call
            .arguments
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if goal.is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "task requires a non-empty `goal`".to_string(),
                is_error: true,
            };
        }

        let context = call
            .arguments
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let subagent_type = call
            .arguments
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        let max_turns = call
            .arguments
            .get("max_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(15) as usize;

        // Permission profile per OpenCode subagent type
        let (permission, system_prompt) = match subagent_type {
            "explore" | "scout" => (
                PermissionSet {
                    allowed_tools: Some(vec![
                        "read".into(),
                        "grep".into(),
                        "glob".into(),
                        "list".into(),
                        "webfetch".into(),
                        "websearch".into(),
                        "lsp".into(),
                    ]),
                    denied_tools: Some(vec![
                        "write".into(),
                        "edit".into(),
                        "shell".into(),
                        "bash".into(),
                        "apply_patch".into(),
                        "todowrite".into(),
                        "todo".into(),
                        "swarm".into(),
                    ]),
                    allow_file_writes: false,
                    allow_network: true,
                    allow_shell: false,
                    allowed_paths: None,
                    rules: Default::default(),
                },
                Self::system_prompt_for(subagent_type),
            ),
            _ => {
                // general: full tools except todo / nested swarm (OpenCode default + safety)
                let mut perm = self.info.permission.clone();
                let mut denied = perm.denied_tools.unwrap_or_default();
                for t in ["todowrite", "todo", "todoread", "swarm"] {
                    if !denied.iter().any(|x| x == t) {
                        denied.push(t.to_string());
                    }
                }
                perm.denied_tools = Some(denied);
                (perm, Self::system_prompt_for("general"))
            }
        };

        let mut info = self.info.clone();
        info.name = subagent_type.to_string();
        info.mode = AgentMode::Subagent;
        info.permission = permission;
        info.system_prompt = Some(Self::with_agents_md(&system_prompt, &session.project_path));

        let task = SubagentTask {
            goal: goal.clone(),
            context,
            tools: None,
            max_turns,
        };

        let runner = SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            info,
            session.project_path.clone(),
            self.sandbox.clone(),
            self.network.clone(),
        )
        .with_memory(self.memory.clone());

        match runner.run(task, provider_name, model, api_key).await {
            Ok(result) => ToolResult {
                tool_call_id: call.id.clone(),
                content: if result.success {
                    format!(
                        "Subagent ({}) completed in {:.1}s:\n\n{}",
                        subagent_type,
                        result.duration.as_secs_f64(),
                        result.output
                    )
                } else {
                    format!(
                        "Subagent ({}) finished with errors:\n\n{}",
                        subagent_type, result.output
                    )
                },
                is_error: !result.success,
            },
            Err(e) => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!("Failed to run subagent: {}", e),
                is_error: true,
            },
        }
    }

    /// Spawn a single subagent to accomplish a goal.
    ///
    /// The subagent runs in a fresh session with its own conversation loop.
    /// Returns the subagent's textual output.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_subagent(
        &self,
        goal: String,
        context: Option<String>,
        tools: Option<Vec<String>>,
        max_turns: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
        project_path: std::path::PathBuf,
    ) -> whycode_core::Result<String> {
        let task = SubagentTask {
            goal: goal.clone(),
            context,
            tools,
            max_turns,
        };

        let runner = SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            self.info.clone(),
            project_path,
            self.sandbox.clone(),
            self.network.clone(),
        )
        .with_memory(self.memory.clone());

        let result = runner.run(task, provider_name, model, api_key).await?;

        Ok(result.output)
    }

    /// Spawn multiple subagents in parallel, respecting a concurrency limit.
    ///
    /// Each `SubagentTask` spawns an independent subagent. Up to `max_concurrent`
    /// subagents run at once; the rest are queued. Returns a Vec of outputs in the
    /// same order as the input tasks.
    pub async fn spawn_parallel(
        &self,
        goals: Vec<SubagentTask>,
        max_concurrent: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
        project_path: std::path::PathBuf,
    ) -> whycode_core::Result<Vec<String>> {
        use tokio::sync::Semaphore;

        let sem = Arc::new(Semaphore::new(max_concurrent.max(1)));
        let provider_name = Arc::from(provider_name.to_string());
        let model = Arc::from(model.to_string());
        let api_key = Arc::from(api_key.to_string());

        let runner = Arc::new(
            SubagentRunner::new(
                Arc::clone(&self.provider_registry),
                Arc::clone(&self.tool_executor),
                self.info.clone(),
                project_path,
                self.sandbox.clone(),
                self.network.clone(),
            )
            .with_memory(self.memory.clone()),
        );

        let mut handles = Vec::with_capacity(goals.len());

        for task in goals {
            let permit = Arc::clone(&sem);
            let r = Arc::clone(&runner);
            let pn = Arc::clone(&provider_name);
            let m = Arc::clone(&model);
            let ak = Arc::clone(&api_key);

            handles.push(tokio::spawn(async move {
                let _guard = permit.acquire().await;
                r.run(task, &pn, &m, &ak).await
            }));
        }

        let mut outputs = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => outputs.push(result.output),
                Ok(Err(e)) => outputs.push(format!("Subagent error: {}", e)),
                Err(e) => outputs.push(format!("Join error: {}", e)),
            }
        }

        Ok(outputs)
    }
}

/// Map config memory table → whycode-memory settings bag.
pub fn memory_settings_from_config(config: &whycode_config::Config) -> whycode_memory::MemorySettings {
    let m = &config.memory;
    whycode_memory::MemorySettings {
        enabled: m.enabled,
        auto_inject: m.auto_inject,
        auto_retain: m.auto_retain,
        retain_llm: m.retain_llm,
        retain_llm_always: m.retain_llm_always,
        retain_every_n: m.retain_every_n,
        retain_max_facts: m.retain_max_facts,
        max_index_lines: m.max_index_lines,
        max_index_bytes: m.max_index_bytes,
        recall_top_k: m.recall_top_k,
        recall_min_score: m.recall_min_score,
        recall_token_budget: m.recall_token_budget,
        embed_dim: m.embed_dim,
        scope: whycode_memory::MemoryScope::parse(&m.scope),
        embed_backend: whycode_memory::EmbedBackend::parse(&m.embed_backend),
        agent_bank: None,
        code_inject: m.code_inject,
        code_top_k: m.code_top_k,
        code_min_score: m.code_min_score,
        auto_index: m.auto_index,
        auto_index_max_files: m.auto_index_max_files,
        auto_index_max_chunks: m.auto_index_max_chunks,
        subagent_banks: m.subagent_banks,
    }
}
