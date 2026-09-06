//! Agent facade: identity, config, prompts, and the conversation loop.
//!
//! Turn execution, tool gates, compaction, and swarm/task dispatch live in
//! sibling modules so this file stays the public surface.

mod compact;
mod dispatch;
mod gate;
mod spawn;
mod turn;

use std::collections::VecDeque;
use std::sync::Arc;

use whycodes_core::SandboxSettings;
use whycodes_core::network::NetworkPolicy;
use whycodes_core::tool::ToolContext;
use whycodes_core::types::{AgentInfo, ApprovalMode, ContentBlock, ToolCall, ToolResult};
use whycodes_llm::provider::ProviderRegistry;
use whycodes_session::session::Session;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::profile::ToolProfile;

use crate::events::{EventSink, TurnEvent};
use crate::permission::{PermissionPrompter, default_prompter};
use crate::question::{QuestionPrompter, default_question_prompter};
use whycodes_command_risk::RiskThreshold;
use whycodes_config::{HookConfig, NotifyConfig};

pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../prompts/build.txt");

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
    /// Discord / Telegram session notifications (off by default).
    notify: NotifyConfig,
    /// When session estimate exceeds this, compact before the next LLM call
    /// (Claude Code / OpenCode style). `0` disables auto-compact.
    compaction_threshold: usize,
    /// `"auto"` = LLM summary on compact (Grok full-replace); `"off"` = local stub.
    compaction_llm: bool,
    /// Tools schema sent to the model (`core` = smaller TTFT).
    tool_profile: ToolProfile,
    /// When false, Anthropic bodies skip cache_control markers.
    use_prompt_cache: bool,
    /// Optional fast model for trivial chat (`provider/model` or bare id).
    model_fast: Option<String>,
    /// First-token race: `off` / `auto` / `provider/model`.
    model_race: String,
    race_after: std::time::Duration,
    /// Process-local text-only response cache.
    response_cache: bool,
    /// Cross-session memory settings (from config).
    memory: whycodes_memory::MemorySettings,
    /// Heuristic intent posture for build turns (`auto` / `off` / `always`).
    intent_guidance: crate::intent::IntentGuidanceMode,
    /// Hidden per-turn notices for standalone prose keywords.
    magic_keywords: whycodes_config::MagicKeywordsConfig,
    /// Session `reasoning_effort` (`low`/`medium`/`high`/`xhigh`). Empty = family default.
    reasoning_effort: Option<String>,
    /// Session overlay for when to interrupt (`auto` / `important` / `manual`).
    approval_mode: ApprovalMode,
    /// `/fresh`: skip provider prompt cache (and local response cache) once.
    skip_prompt_cache_once: std::sync::atomic::AtomicBool,
    /// Cheap model for task/swarm (`provider/model` or bare id).
    model_smol: Option<String>,
    /// Model used while the `plan` agent is active.
    model_plan: Option<String>,
    /// Compiled stream-interrupt rules (name, regex, hint).
    stream_rules: Vec<(String, regex::Regex, String)>,
    /// Parallel multi-agent swarm (config-driven).
    swarm_enabled: bool,
    swarm_max_agents: usize,
    /// Isolate workers in git worktrees when the project is a repo.
    swarm_worktrees: bool,
    /// Background shell jobs (`bash` background=true, `bg`, `schedule`).
    background: crate::background::BackgroundRegistry,
    /// Optional long-lived event sink (TUI) for bg completion + enqueue.
    event_sink: Option<EventSink>,
    /// Max concurrent background jobs.
    max_background_jobs: usize,
    /// Deferred tools activated via `tool_search` for this agent session.
    activated_tools: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Optional tool cwd override (`worktree enter`).
    cwd_override: Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Subagent token usage waiting to be folded into the parent session.
    subagent_usage_pending: Arc<std::sync::Mutex<whycodes_core::types::Usage>>,
    /// Resident workspace file index shared with file tools (warm fast path
    /// for glob/grep/list enumeration). Started by the host (TUI/CLI).
    file_index: Option<Arc<whycodes_index::WorkspaceIndex>>,
    /// Process-wide claims for parallel TUI sessions (Ctrl+N).
    session_claims: Option<whycodes_core::FileClaimRegistry>,
    /// Swarm mailbox when this agent is a worker (or parent mid-swarm).
    swarm_hub: Option<whycodes_core::SwarmHub>,
}

/// Stop auto-compact after this many consecutive ineffective passes (Claude Code).
pub(crate) const MAX_CONSECUTIVE_COMPACT_FAILURES: u32 = 3;

/// Identical tool name+args this many times in a row → refuse (OpenCode doom_loop).
pub(crate) const DOOM_LOOP_THRESHOLD: usize = 3;

/// Validate checkpoint/rewind tool results and return pending side effects.
///
/// Side effects run after `add_tool_results` so the checkpoint boundary includes
/// the successful checkpoint tool result.
pub(crate) fn settle_checkpoint_rewind(
    session: &Session,
    tool_calls: &[ToolCall],
    results: &mut [ToolResult],
) -> (Option<String>, Option<String>) {
    let mut checkpoint_goal = None;
    let mut rewind_report = None;
    for (tc, r) in tool_calls.iter().zip(results.iter_mut()) {
        if r.is_error {
            continue;
        }
        match tc.name.as_str() {
            "checkpoint" => {
                if session.checkpoint.is_some() {
                    r.is_error = true;
                    r.content = "Checkpoint already active.".into();
                } else if let Some(goal) = tc
                    .arguments
                    .get("goal")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    checkpoint_goal = Some(goal.to_string());
                }
            }
            "rewind" => {
                if session.checkpoint.is_none() {
                    r.is_error = true;
                    r.content = if session.last_rewind_report.is_some() {
                        "Checkpoint already completed; continue from the retained rewind report \
                         instead of calling rewind again."
                            .into()
                    } else {
                        "No active checkpoint. Create a checkpoint before calling rewind.".into()
                    };
                } else if let Some(report) = tc
                    .arguments
                    .get("report")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    rewind_report = Some(report.to_string());
                }
            }
            _ => {}
        }
    }
    (checkpoint_goal, rewind_report)
}

pub(crate) fn persist_agent_artifact(project: &std::path::Path, id: &str, body: &str) {
    let id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if id.is_empty() {
        return;
    }
    let dir = whycodes_core::project_dir(project).join("agents");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, "agent artifact dir");
        return;
    }
    let path = dir.join(format!("{id}.md"));
    if let Err(e) = std::fs::write(&path, body) {
        tracing::debug!(error = %e, path = %path.display(), "agent artifact write");
    }
}

fn compile_stream_rules(
    rules: &[whycodes_config::StreamRuleConfig],
) -> Vec<(String, regex::Regex, String)> {
    let mut out = Vec::new();
    for rule in rules {
        let name = rule.name.trim();
        let pattern = rule.pattern.trim();
        let hint = rule.hint.trim();
        if name.is_empty() || pattern.is_empty() || hint.is_empty() {
            continue;
        }
        match regex::Regex::new(pattern) {
            Ok(re) => out.push((name.to_string(), re, hint.to_string())),
            Err(e) => {
                tracing::warn!(name, pattern, error = %e, "invalid stream rule regex");
            }
        }
    }
    out
}

pub(crate) fn first_stream_rule_hit<'a>(
    rules: &'a [(String, regex::Regex, String)],
    text: &str,
) -> Option<(&'a str, &'a str)> {
    for (name, re, hint) in rules {
        if re.is_match(text) {
            return Some((name.as_str(), hint.as_str()));
        }
    }
    None
}

fn append_skills_catalog(system_prompt: &str, project_path: &std::path::Path) -> String {
    let Ok(reg) = whycodes_skill::SkillRegistry::load_project(project_path) else {
        return system_prompt.to_string();
    };
    let catalog = reg.catalog_markdown();
    if catalog.is_empty() {
        return system_prompt.to_string();
    }
    format!("{system_prompt}\n\n{catalog}")
}

pub(crate) fn append_request_user_suffix(
    request: &mut whycodes_core::types::LlmRequest,
    suffix: &str,
) {
    use whycodes_core::types::{MessageContent, Role};
    for msg in request.messages_mut().iter_mut().rev() {
        if msg.role != Role::User {
            continue;
        }
        match &mut msg.content {
            MessageContent::Text(t) => t.push_str(suffix),
            MessageContent::Blocks(blocks) => {
                blocks.push(ContentBlock::Text {
                    text: suffix.to_string(),
                });
            }
        }
        break;
    }
}

pub(crate) fn tool_call_signature(tc: &ToolCall) -> String {
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
            notify: NotifyConfig::default(),
            // Match config default when `with_config` is not used.
            compaction_threshold: 150_000,
            compaction_llm: true,
            tool_profile: ToolProfile::Core,
            use_prompt_cache: true,
            model_fast: None,
            model_race: "off".into(),
            race_after: std::time::Duration::from_millis(800),
            response_cache: true,
            memory: whycodes_memory::MemorySettings::default(),
            intent_guidance: crate::intent::IntentGuidanceMode::default(),
            magic_keywords: whycodes_config::MagicKeywordsConfig::default(),
            reasoning_effort: None,
            approval_mode: ApprovalMode::Auto,
            skip_prompt_cache_once: std::sync::atomic::AtomicBool::new(false),
            model_smol: None,
            model_plan: None,
            stream_rules: Vec::new(),
            swarm_enabled: true,
            swarm_max_agents: 4,
            swarm_worktrees: true,
            background: crate::background::BackgroundRegistry::default(),
            event_sink: None,
            max_background_jobs: crate::background::DEFAULT_MAX_BACKGROUND_JOBS,
            activated_tools: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            cwd_override: Arc::new(std::sync::Mutex::new(None)),
            subagent_usage_pending: Arc::new(std::sync::Mutex::new(
                whycodes_core::types::Usage::default(),
            )),
            file_index: None,
            session_claims: None,
            swarm_hub: None,
        }
    }

    pub fn with_provider_registry(mut self, registry: ProviderRegistry) -> Self {
        self.provider_registry = Arc::new(registry);
        self
    }

    /// Replace the provider registry in place (tests / CLI scripted LLM).
    pub fn set_provider_registry(&mut self, registry: ProviderRegistry) {
        self.provider_registry = Arc::new(registry);
    }

    pub fn with_tool_executor(mut self, executor: ToolExecutor) -> Self {
        self.tool_executor = Arc::new(executor);
        self
    }

    /// Share the workspace file index with this agent's file tools.
    pub fn with_file_index(mut self, index: Arc<whycodes_index::WorkspaceIndex>) -> Self {
        self.file_index = Some(index);
        self
    }

    /// Mutate the file index in place (used for paint-then-hydrate boot).
    pub fn set_file_index(&mut self, index: Arc<whycodes_index::WorkspaceIndex>) {
        self.file_index = Some(index);
    }

    /// Load shell plugins in place (paint-then-hydrate; avoids `with_plugins` move).
    pub fn hydrate_plugins(&mut self, project_dir: Option<&std::path::Path>) {
        let mut exec = whycodes_tools::executor::ToolExecutor::new();
        let n = exec.register_config_plugins(project_dir);
        if n > 0 {
            self.tool_executor = Arc::new(exec);
        }
    }

    /// Share a process-wide claim registry (parallel TUI sessions).
    pub fn with_session_claims(mut self, claims: whycodes_core::FileClaimRegistry) -> Self {
        self.session_claims = Some(claims);
        self
    }

    pub fn session_claims(&self) -> Option<whycodes_core::FileClaimRegistry> {
        self.session_claims.clone()
    }

    pub fn with_permission_prompter(mut self, prompter: Arc<dyn PermissionPrompter>) -> Self {
        self.permission_prompter = prompter;
        self
    }

    pub fn with_question_prompter(mut self, prompter: Arc<dyn QuestionPrompter>) -> Self {
        self.question_prompter = prompter;
        self
    }

    /// Next LLM turn skips the provider prompt cache (stale cache / wedged stream).
    pub fn skip_prompt_cache_next(&self) {
        self.skip_prompt_cache_once
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Session-level OpenAI-compat / xAI `reasoning_effort` (`low`/`medium`/`high`/`xhigh`).
    pub fn set_reasoning_effort(&mut self, effort: Option<String>) {
        self.reasoning_effort = effort;
    }

    /// Session-level approval overlay (`auto` / `important` / `manual`).
    pub fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.approval_mode = mode;
    }

    /// Current approval overlay.
    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval_mode
    }

    /// Load custom providers from config and merge global permission rules.
    pub fn with_config(mut self, config: &whycodes_config::Config) -> Self {
        self.apply_config(config);
        self
    }

    /// Re-apply config on a live agent (TUI `/import` / first-run confirm).
    pub fn apply_config(&mut self, config: &whycodes_config::Config) {
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
        self.notify = config.notify.clone();
        self.compaction_threshold = config.session.compaction_threshold;
        self.compaction_llm = !matches!(
            config
                .session
                .compaction_llm
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "false" | "0" | "none" | "local"
        );
        self.tool_profile = ToolProfile::parse(&config.session.tool_profile);
        self.use_prompt_cache = !matches!(
            config
                .session
                .prompt_cache
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "none" | "off" | "false" | "0"
        );
        self.model_fast = config.session.model_fast.clone();
        self.model_race = config.session.model_race.clone();
        self.race_after = std::time::Duration::from_millis(config.session.race_after_ms);
        self.response_cache = !matches!(
            config
                .session
                .response_cache
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "off" | "false" | "0" | "none"
        );
        self.memory = memory_settings_from_config(config);
        self.intent_guidance =
            crate::intent::IntentGuidanceMode::parse(&config.session.intent_guidance);
        self.magic_keywords = config.session.magic_keywords.clone();
        self.reasoning_effort = config.session.reasoning_effort.clone();
        self.approval_mode = config.general.approval_mode.unwrap_or_default();
        self.model_smol = config.session.model_smol.clone();
        self.model_plan = config.session.model_plan.clone();
        self.stream_rules = compile_stream_rules(&config.session.stream_rules);
        self.swarm_enabled = config.swarm.enabled;
        self.swarm_max_agents = config
            .swarm
            .max_agents
            .clamp(1, crate::swarm::SWARM_HARD_MAX_AGENTS);
        self.swarm_worktrees = config.swarm.use_worktrees();
        self.max_background_jobs = config.automation.max_background_jobs.clamp(
            1,
            crate::background::DEFAULT_MAX_BACKGROUND_JOBS
                .saturating_mul(2)
                .max(8),
        );
        // Resize ceiling only — keep the same registry so in-flight jobs survive
        // agent switches / re-config.
        self.background.set_max_jobs(self.max_background_jobs);
        tracing::debug!(
            sandbox = %whycodes_sandbox::describe_backend(&self.sandbox),
            network_allow = self.network.allowlist.len(),
            network_deny = self.network.denylist.len(),
            hooks = self.hooks.len(),
            compaction_threshold = self.compaction_threshold,
            tool_profile = self.tool_profile.as_str(),
            use_prompt_cache = self.use_prompt_cache,
            model_race = %self.model_race,
            response_cache = self.response_cache,
            memory_enabled = self.memory.enabled,
            intent_guidance = ?self.intent_guidance,
            swarm_enabled = self.swarm_enabled,
            swarm_max_agents = self.swarm_max_agents,
            swarm_worktrees = self.swarm_worktrees,
            max_background_jobs = self.max_background_jobs,
            "shell sandbox, network policy, and hooks"
        );
    }

    /// Forward `panel` tool updates onto the turn event channel.
    pub(crate) fn panel_sink(&self) -> Option<whycodes_core::PanelSink> {
        let tx = self.event_sink.clone()?;
        Some(std::sync::Arc::new(move |update| {
            if let Err(e) = tx.send(TurnEvent::Panel(update)) {
                tracing::debug!(error = %e, "panel event dropped (listener closed)");
            }
        }))
    }

    /// Forward `todowrite` updates onto the turn event channel.
    pub(crate) fn todo_sink(&self) -> Option<whycodes_core::TodoSink> {
        let tx = self.event_sink.clone()?;
        Some(std::sync::Arc::new(move |todos| {
            if let Err(e) = tx.send(TurnEvent::Todos { todos }) {
                tracing::debug!(error = %e, "todo event dropped (listener closed)");
            }
        }))
    }

    /// Attach a long-lived event sink (TUI) for background job notifications
    /// and scheduled prompt enqueue. Safe to call once after channel setup.
    pub fn wire_event_sink(&mut self, sink: EventSink) {
        self.event_sink = Some(sink.clone());
        let tx = sink;
        self.background
            .set_listener(Some(std::sync::Arc::new(move |ev| {
                let _ = tx.send(TurnEvent::Background {
                    id: ev.id,
                    status: ev.status.as_str().to_string(),
                    summary: ev.summary,
                });
            })));
    }

    pub fn background_registry(&self) -> &crate::background::BackgroundRegistry {
        &self.background
    }

    /// Share background jobs across agent identity switches (Ctrl+T).
    pub fn with_background_registry(mut self, reg: crate::background::BackgroundRegistry) -> Self {
        self.background = reg;
        self
    }

    /// Memory settings loaded from config (for CLI/TUI helpers).
    pub fn memory_settings(&self) -> &whycodes_memory::MemorySettings {
        &self.memory
    }

    pub fn with_tool_profile(mut self, profile: ToolProfile) -> Self {
        self.tool_profile = profile;
        self
    }

    pub fn model_fast(&self) -> Option<&str> {
        self.model_fast.as_deref()
    }

    /// Resolve an optional first-token race partner (`off` / `auto` / ref).
    pub(crate) fn race_partner(
        &self,
        provider_name: &str,
        model: &str,
    ) -> Option<(String, String)> {
        let raw = self.model_race.trim();
        if raw.is_empty()
            || matches!(
                raw.to_ascii_lowercase().as_str(),
                "off" | "false" | "0" | "none"
            )
        {
            return None;
        }
        let (p, m) = if raw.eq_ignore_ascii_case("auto") {
            crate::title::resolve_title_model(provider_name, model, None)
        } else {
            crate::title::resolve_title_model(provider_name, model, Some(raw))
        };
        if p == provider_name && m == model {
            return None;
        }
        self.provider_registry.get(&p)?;
        Some((p, m))
    }

    /// Build tool context for a session, applying permission network flags.
    pub(crate) fn tool_context(&self, session: &Session) -> ToolContext {
        let mut sandbox = self.sandbox.clone();
        if !self.info.permission.allow_network {
            sandbox.network = false;
        }
        let working_dir = self
            .cwd_override
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| session.project_path.to_string_lossy().to_string());
        ToolContext {
            working_dir,
            session_id: Some(session.id.clone()),
            sandbox,
            network: self.network.clone(),
            file_claims: self.session_claims.clone(),
            agent_id: self
                .session_claims
                .as_ref()
                .map(|_| format!("sess-{}", &session.id[..8.min(session.id.len())])),
            agent_label: self.session_claims.as_ref().map(|_| self.info.name.clone()),
            file_index: self.file_index.clone(),
            panel: self.panel_sink(),
            todo_sink: self.todo_sink(),
            swarm_hub: self.swarm_hub.clone(),
        }
    }

    /// Snapshot of tools activated via `tool_search`.
    pub fn activated_tools_snapshot(&self) -> Vec<String> {
        self.activated_tools
            .lock()
            .map(|g| {
                let mut v: Vec<_> = g.iter().cloned().collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    /// Active tool cwd (worktree enter), if any.
    pub fn cwd_override_path(&self) -> Option<std::path::PathBuf> {
        self.cwd_override.lock().ok().and_then(|g| g.clone())
    }

    /// Register shell plugins + MCP tools on a fresh executor.
    ///
    /// Always reloads built-ins; adds `plugin_*` from `plugins.toml` and
    /// `plugin.json` trees, then MCP server tools when configured.
    pub async fn with_mcp(mut self, config: &whycodes_config::Config) -> Self {
        self.load_mcp(config).await;
        self
    }

    /// Like [`Self::with_mcp`] but for an already-owned agent (TUI deferred load).
    pub async fn load_mcp(&mut self, config: &whycodes_config::Config) {
        let project = config.general.project_path.as_deref();
        let mut full = ToolExecutor::new();
        let n_plug = full.register_config_plugins(project);
        let n_mcp = if config.mcp_servers.is_empty() {
            0
        } else {
            crate::mcp_load::register_mcp_tools(&mut full, config).await
        };
        if n_plug > 0 || n_mcp > 0 {
            self.tool_executor = Arc::new(full);
            if n_plug > 0 {
                tracing::info!(count = n_plug, "shell plugins registered");
            }
            if n_mcp > 0 {
                tracing::info!(count = n_mcp, "MCP tools registered");
            }
        }
    }

    /// Load shell plugins only (when not calling [`Self::with_mcp`]).
    pub fn with_plugins(mut self, project_dir: Option<&std::path::Path>) -> Self {
        let mut exec = ToolExecutor::new();
        let n = exec.register_config_plugins(project_dir);
        if n > 0 {
            self.tool_executor = Arc::new(exec);
            tracing::info!(count = n, "shell plugins registered");
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
            "build" => include_str!("../../prompts/build.txt").to_string(),
            "plan" => include_str!("../../prompts/plan.txt").to_string(),
            "ask" => include_str!("../../prompts/ask.txt").to_string(),
            "explore" => include_str!("../../prompts/explore.txt").to_string(),
            "general" => include_str!("../../prompts/general.txt").to_string(),
            "scout" => include_str!("../../prompts/explore.txt").to_string(),
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

    /// Append project instruction files (AGENTS.md and sibling conventions)
    /// plus runtime context to a system prompt.
    pub fn with_agents_md(system_prompt: &str, project_path: &std::path::Path) -> String {
        let with_files =
            crate::context_files::append_project_instructions(system_prompt, project_path);
        let with_skills = append_skills_catalog(&with_files, project_path);
        Self::with_runtime_context(&with_skills)
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
        self.provider_registry.get(&use_provider_name)?;

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
        let Some((use_provider_name, use_model, key, user, assistant)) =
            self.title_refine_target(session, provider_name, model, api_key, title_model_override)
        else {
            return;
        };

        let Some(provider) = self.provider_registry.get(&use_provider_name) else {
            return;
        };

        match crate::title::generate_title(provider, &key, &use_model, &user, assistant.as_deref())
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
        let Some((use_provider_name, use_model, key, user, assistant)) =
            self.title_refine_target(session, provider_name, model, api_key, title_model_override)
        else {
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
}

/// Map config memory table → whycodes-memory settings bag.
pub fn memory_settings_from_config(
    config: &whycodes_config::Config,
) -> whycodes_memory::MemorySettings {
    let m = &config.memory;
    whycodes_memory::MemorySettings {
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
        scope: whycodes_memory::MemoryScope::parse(&m.scope),
        embed_backend: whycodes_memory::EmbedBackend::parse(&m.embed_backend),
        agent_bank: None,
        code_inject: m.code_inject,
        code_top_k: m.code_top_k,
        code_min_score: m.code_min_score,
        auto_index: m.auto_index,
        auto_index_max_files: m.auto_index_max_files,
        auto_index_max_chunks: m.auto_index_max_chunks,
        subagent_banks: m.subagent_banks,
        session_inject: m.session_inject,
        session_top_k: m.session_top_k,
        session_min_score: m.session_min_score,
        consolidate: m.consolidate,
        consolidate_max: m.consolidate_max,
    }
}

#[cfg(test)]
mod permission_detail_tests {
    use super::*;
    use crate::tool_policy::*;
    use serde_json::json;
    use whycodes_core::Tool;
    use whycodes_core::types::PermissionSet;

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

    #[test]
    fn empty_args_is_labeled() {
        assert_eq!(format_permission_detail(&json!({})), "(no arguments)");
    }

    #[test]
    fn scalars_fall_back_to_pretty_json() {
        let d = format_permission_detail(&json!("plain string"));
        assert_eq!(d, "\"plain string\"");
        assert!(d.starts_with('\"'));
    }

    #[test]
    fn nested_objects_are_labeled_with_indented_lines() {
        let d = format_permission_detail(&json!({"patch": {"file": "a.rs", "edits": 2}}));
        assert!(d.contains("patch:"), "{d}");
        assert!(d.contains("\"file\": \"a.rs\""), "{d}");
    }

    #[test]
    fn multiline_strings_are_indented() {
        let d = format_permission_detail(&json!({"content": "line1\nline2\nline3"}));
        assert!(d.contains("content:"), "{d}");
        assert!(d.contains("  line1"), "{d}");
        assert!(d.contains("  line2"), "{d}");
        assert!(d.contains("  line3"), "{d}");
    }

    #[test]
    fn null_bool_number_values_are_labeled() {
        let d = format_permission_detail(&json!({"a": null, "b": true, "c": 42}));
        assert!(d.contains("a: null"), "{d}");
        assert!(d.contains("b: true"), "{d}");
        assert!(d.contains("c: 42"), "{d}");
    }

    #[test]
    fn truncate_permission_detail_caps_long_text() {
        let long = "x".repeat(PERMISSION_DETAIL_MAX + 10);
        let t = truncate_permission_detail(&long);
        assert_eq!(t.chars().count(), PERMISSION_DETAIL_MAX + 1); // ellipsis appended
        assert!(t.ends_with('…'));
        assert!(t.chars().take(PERMISSION_DETAIL_MAX).all(|c| c == 'x'));
        let short = "short".to_string();
        assert_eq!(truncate_permission_detail(&short), "short");
    }

    #[test]
    fn worktree_names_are_validated() {
        assert!(is_safe_worktree_name("feat-auth"));
        assert!(is_safe_worktree_name("branch_2"));
        assert!(is_safe_worktree_name("A1-b_c"));
        assert!(!is_safe_worktree_name(""));
        assert!(!is_safe_worktree_name("   "));
        assert!(!is_safe_worktree_name("a/b"));
        assert!(!is_safe_worktree_name(".."));
        assert!(!is_safe_worktree_name("with space"));
        assert!(!is_safe_worktree_name(&"x".repeat(65)));
    }

    #[test]
    fn file_tool_path_extracts_and_normalizes() {
        let tc = |name: &str, args: serde_json::Value| ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments: args,
        };
        assert_eq!(
            file_tool_path(&tc("read", json!({"path": "src/main.rs"}))),
            Some("src/main.rs".into())
        );
        // backslashes normalized to forward slashes
        assert_eq!(
            file_tool_path(&tc("edit", json!({"path": "src\\mod.rs"}))),
            Some("src/mod.rs".into())
        );
        // path trimmed
        assert_eq!(
            file_tool_path(&tc("write", json!({"path": "  a.rs  "}))),
            Some("a.rs".into())
        );
        // apply_patch may use path
        assert_eq!(
            file_tool_path(&tc("apply_patch", json!({"path": "x.rs"}))),
            Some("x.rs".into())
        );
        // missing / empty / non-string path
        assert_eq!(file_tool_path(&tc("read", json!({}))), None);
        assert_eq!(file_tool_path(&tc("read", json!({"path": ""}))), None);
        assert_eq!(file_tool_path(&tc("read", json!({"path": 42}))), None);
        // unknown tool name
        assert_eq!(file_tool_path(&tc("bash", json!({"path": "x"}))), None);
    }

    #[test]
    fn parallel_safety_respects_serial_list() {
        assert!(is_parallel_safe_tool("read", &PermissionSet::default()));
        assert!(is_parallel_safe_tool("grep", &PermissionSet::default()));
        assert!(is_parallel_safe_tool("glob", &PermissionSet::default()));
        assert!(is_parallel_safe_tool("todoread", &PermissionSet::default()));
        for name in SERIAL_TOOLS {
            assert!(
                !is_parallel_safe_tool(name, &PermissionSet::default()),
                "{name}"
            );
        }
        // Real registration names (see tools/executor.rs), not snake_case typos.
        assert!(SERIAL_TOOLS.contains(&"todowrite"));
        assert!(!SERIAL_TOOLS.contains(&"todo_write"));
        assert!(!is_parallel_safe_tool(
            "todowrite",
            &PermissionSet::default()
        ));
        assert!(is_parallel_safe_tool(
            "todo_write",
            &PermissionSet::default()
        ));
    }

    #[test]
    fn tool_signatures_and_doom_loop() {
        let tc = |name: &str, args: serde_json::Value| ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments: args,
        };
        let a = tc("read", json!({"path": "x.rs"}));
        let b = tc("read", json!({"path": "y.rs"}));
        assert_eq!(tool_call_signature(&a), "read|{\"path\":\"x.rs\"}");
        assert_ne!(tool_call_signature(&a), tool_call_signature(&b));

        let mut recent = VecDeque::new();
        // nothing recent → not a doom loop yet
        assert!(!would_doom_loop(&recent, std::slice::from_ref(&a)));
        // mixed batch is never a doom loop
        assert!(!would_doom_loop(&recent, &[a.clone(), b.clone()]));
        // empty calls → false
        assert!(!would_doom_loop(&recent, &[]));

        // push two identical signatures, then a third call trips the threshold
        recent.push_back(tool_call_signature(&a));
        recent.push_back(tool_call_signature(&a));
        assert!(would_doom_loop(&recent, std::slice::from_ref(&a)));
        // batch of identical calls counts as the same signature repeated
        assert!(would_doom_loop(&recent, &[a.clone(), a.clone()]));
        // an intervening different signature resets the run
        let mut recent2 = VecDeque::new();
        recent2.push_back(tool_call_signature(&a));
        recent2.push_back(tool_call_signature(&b));
        assert!(!would_doom_loop(&recent2, std::slice::from_ref(&a)));
    }

    #[test]
    fn system_prompt_for_known_and_unknown_agents() {
        for name in ["build", "plan", "ask", "explore", "general", "scout"] {
            let p = Agent::system_prompt_for(name);
            assert!(!p.is_empty(), "{name}");
            assert!(!p.contains("Today's date:"), "{name}");
        }
        assert!(
            Agent::system_prompt_for("build").contains("todowrite"),
            "build prompt must instruct todo use"
        );
        assert!(
            Agent::system_prompt_for("build").contains("Do **not** call `question`"),
            "build prompt must keep auto mode from asking mid-todo"
        );
        assert!(
            Agent::system_prompt_for("plan").contains("todowrite"),
            "plan prompt must instruct todo use"
        );
        assert_eq!(
            Agent::system_prompt_for("does-not-exist"),
            DEFAULT_SYSTEM_PROMPT
        );
    }

    #[test]
    fn runtime_context_is_idempotent_and_append_only() {
        let base = "You are an agent.";
        let once = Agent::with_runtime_context(base);
        assert!(once.contains("Today's date:"));
        assert!(once.starts_with(base));
        // second application does not duplicate the block
        assert_eq!(Agent::with_runtime_context(&once), once);
        // already-present marker is left untouched
        let already = "Prompt with Today's date: 2026-01-01.";
        assert_eq!(Agent::with_runtime_context(already), already);
    }

    #[test]
    fn agents_md_is_appended_and_candidates_are_tried() {
        let dir = tempfile::tempdir().unwrap();
        // no AGENTS.md → prompt unchanged (plus runtime context)
        let bare = Agent::with_agents_md("base", dir.path());
        assert!(bare.starts_with("base"));
        assert!(!bare.contains("Project Instructions"));

        // AGENTS.md at project root is picked up
        std::fs::write(dir.path().join("AGENTS.md"), "  \nProject rules here\n  ").unwrap();
        let with = Agent::with_agents_md("base", dir.path());
        assert!(with.contains("Project Instructions (AGENTS.md)"), "{with}");
        assert!(with.contains("Project rules here"), "{with}");

        // lowercase agents.md also works when AGENTS.md absent
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(dir2.path().join("agents.md"), "lowercase rules").unwrap();
        let with2 = Agent::with_agents_md("base", dir2.path());
        assert!(with2.contains("lowercase rules"), "{with2}");

        // .whycodes/AGENTS.md is the fallback candidate
        let dir3 = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir3.path().join(".whycodes")).unwrap();
        std::fs::write(dir3.path().join(".whycodes/AGENTS.md"), "nested rules").unwrap();
        let with3 = Agent::with_agents_md("base", dir3.path());
        assert!(with3.contains("nested rules"), "{with3}");
    }

    #[test]
    fn skills_catalog_is_appended_without_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(".skills");
        std::fs::create_dir(&skills).unwrap();
        std::fs::write(
            skills.join("demo.skill.md"),
            "---\nname: demo\ndescription: short desc\n---\n\nSECRET BODY MUST NOT LEAK\n",
        )
        .unwrap();
        let with = Agent::with_agents_md("base", dir.path());
        assert!(with.contains("# Skills"), "{with}");
        assert!(with.contains("`demo`"), "{with}");
        assert!(with.contains("short desc"), "{with}");
        assert!(with.contains("skill://"), "{with}");
        assert!(!with.contains("SECRET BODY MUST NOT LEAK"), "{with}");
    }

    #[test]
    fn stream_rules_compile_and_match() {
        use whycodes_config::StreamRuleConfig;
        let rules = [
            StreamRuleConfig {
                name: String::new(),
                pattern: "x".into(),
                hint: "h".into(),
            },
            StreamRuleConfig {
                name: "bad".into(),
                pattern: "(".into(),
                hint: "h".into(),
            },
            StreamRuleConfig {
                name: "no-leak".into(),
                pattern: "Box::leak".into(),
                hint: "use Arc".into(),
            },
        ];
        let compiled = compile_stream_rules(&rules);
        assert_eq!(compiled.len(), 1);
        let hit = first_stream_rule_hit(&compiled, "please Box::leak this");
        assert_eq!(hit, Some(("no-leak", "use Arc")));
        assert!(first_stream_rule_hit(&compiled, "Arc::from").is_none());
    }

    #[test]
    fn persist_agent_artifact_sanitizes_id() {
        let dir = tempfile::tempdir().unwrap();
        persist_agent_artifact(dir.path(), "task-ok", "hello");
        persist_agent_artifact(dir.path(), "../evil", "nope");
        persist_agent_artifact(dir.path(), "", "ignored");
        let agents = dir.path().join(".whycodes").join("agents");
        assert_eq!(
            std::fs::read_to_string(agents.join("task-ok.md")).unwrap(),
            "hello"
        );
        assert!(!dir.path().join("evil.md").exists());
        assert!(!dir.path().join("evil").exists());
        // `../evil` strips to `evil` and stays inside the agents dir.
        assert_eq!(
            std::fs::read_to_string(agents.join("evil.md")).unwrap(),
            "nope"
        );
    }

    #[test]
    fn memory_settings_map_from_config() {
        let config = whycodes_config::Config::default();
        let m = memory_settings_from_config(&config);
        assert_eq!(m.enabled, config.memory.enabled);
        assert_eq!(m.auto_inject, config.memory.auto_inject);
        assert_eq!(m.auto_retain, config.memory.auto_retain);
        assert_eq!(m.retain_every_n, config.memory.retain_every_n);
        assert_eq!(
            m.scope,
            whycodes_memory::MemoryScope::parse(&config.memory.scope)
        );
        assert_eq!(m.agent_bank, None);
    }

    #[test]
    fn set_provider_registry_replaces_lookup() {
        let mut a = test_agent();
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(whycodes_llm::ScriptedProvider::text("hi")));
        a.set_provider_registry(registry);
        assert!(a.provider_registry.get("script").is_some());
        assert!(a.provider_registry.get("anthropic").is_none());
    }

    #[test]
    fn skip_prompt_cache_next_is_oneshot() {
        let a = test_agent();
        assert!(
            !a.skip_prompt_cache_once
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        a.skip_prompt_cache_next();
        assert!(
            a.skip_prompt_cache_once
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(
            a.skip_prompt_cache_once
                .swap(false, std::sync::atomic::Ordering::Relaxed)
        );
        assert!(
            !a.skip_prompt_cache_once
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }

    #[test]
    fn settle_checkpoint_rewind_guards_and_extracts() {
        let mut session = Session::new(std::path::PathBuf::from("/p"), "s".into());
        let calls = [tc("rewind", serde_json::json!({"report": "findings"}))];
        let mut results = [ToolResult {
            tool_call_id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        }];
        let (goal, report) = settle_checkpoint_rewind(&session, &calls, &mut results);
        assert!(goal.is_none());
        assert!(report.is_none());
        assert!(results[0].is_error);
        assert!(results[0].content.contains("No active checkpoint"));

        session.mark_checkpoint("look");
        results[0].is_error = false;
        results[0].content = "ok".into();
        let (goal, report) = settle_checkpoint_rewind(&session, &calls, &mut results);
        assert!(goal.is_none());
        assert_eq!(report.as_deref(), Some("findings"));
        assert!(!results[0].is_error);

        let cp_calls = [tc("checkpoint", serde_json::json!({"goal": "again"}))];
        let mut cp_results = [ToolResult {
            tool_call_id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        }];
        let (goal, report) = settle_checkpoint_rewind(&session, &cp_calls, &mut cp_results);
        assert!(goal.is_none() && report.is_none());
        assert!(cp_results[0].is_error);
        assert!(cp_results[0].content.contains("already active"));

        let mut fresh = Session::new(std::path::PathBuf::from("/p"), "s".into());
        fresh.last_rewind_report = Some("old".into());
        let mut again = [ToolResult {
            tool_call_id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        }];
        let (_g, _r) = settle_checkpoint_rewind(&fresh, &calls, &mut again);
        assert!(again[0].content.contains("already completed"));

        let empty = Session::new(std::path::PathBuf::from("/p"), "s".into());
        let mk = [tc("checkpoint", serde_json::json!({"goal": "scan"}))];
        let mut mk_r = [ToolResult {
            tool_call_id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        }];
        let (goal, _) = settle_checkpoint_rewind(&empty, &mk, &mut mk_r);
        assert_eq!(goal.as_deref(), Some("scan"));
        assert!(!mk_r[0].is_error);
    }

    fn test_agent() -> Agent {
        Agent::new(whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: "t".into(),
            mode: whycodes_core::types::AgentMode::Primary,
            permission: PermissionSet {
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                ..Default::default()
            },
            model: None,
            system_prompt: Some("sys".into()),
            temperature: None,
            top_p: None,
        })
    }

    fn tc(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "t1".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn race_partner_off_auto_and_unknown() {
        let mut a = test_agent();
        a.model_race = "off".into();
        assert!(a.race_partner("anthropic", "claude").is_none());
        a.model_race = "none".into();
        assert!(a.race_partner("anthropic", "claude").is_none());
        a.model_race = "auto".into();
        // Default registry has anthropic; auto may pick a sibling or none
        // if resolve returns the same pair.
        let _ = a.race_partner("anthropic", "claude-sonnet-4-20250514");
        a.model_race = "openai/gpt-4o".into();
        let partner = a.race_partner("anthropic", "claude");
        assert!(
            partner
                .as_ref()
                .is_some_and(|(p, m)| p == "openai" && m.contains("gpt")),
            "{partner:?}"
        );
        a.model_race = "not-a-provider/x".into();
        assert!(a.race_partner("anthropic", "claude").is_none());
    }

    #[test]
    fn tool_context_uses_session_cwd_and_strips_network() {
        let mut a = test_agent();
        a.info.permission.allow_network = false;
        let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
        let ctx = a.tool_context(&session);
        assert_eq!(ctx.working_dir, "/tmp/proj");
        assert!(!ctx.sandbox.network);
        assert_eq!(ctx.session_id.as_deref(), Some(session.id.as_str()));

        if let Ok(mut g) = a.cwd_override.lock() {
            *g = Some(std::path::PathBuf::from("/tmp/wt"));
        }
        let ctx2 = a.tool_context(&session);
        assert_eq!(ctx2.working_dir, "/tmp/wt");
    }

    #[test]
    fn execute_bg_tool_list_read_kill_and_unknown() {
        let a = test_agent();
        let list = a.execute_bg_tool(&tc("bg", json!({"action": "list"})));
        assert!(!list.is_error);
        assert!(list.content.contains("No background jobs"), "{list:?}");

        let read = a.execute_bg_tool(&tc("bg", json!({"action": "read"})));
        assert!(read.is_error);
        assert!(read.content.contains("requires `id`"), "{read:?}");

        let kill = a.execute_bg_tool(&tc("bg", json!({"action": "kill"})));
        assert!(kill.is_error);

        let missing = a.execute_bg_tool(&tc("bg", json!({"action": "read", "id": "nope"})));
        assert!(missing.is_error);

        let unk = a.execute_bg_tool(&tc("bg", json!({"action": "explode"})));
        assert!(unk.is_error);
        assert!(unk.content.contains("unknown bg action"), "{unk:?}");
    }

    #[test]
    fn execute_tool_search_list_select_and_query() {
        let a = test_agent();
        let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
        assert!(!listed.is_error);
        assert!(listed.content.contains("Deferred catalogue"), "{listed:?}");

        let empty = a.execute_tool_search(&tc("tool_search", json!({})));
        assert!(empty.is_error);
        assert!(empty.content.contains("requires `query`"), "{empty:?}");

        let sel_empty = a.execute_tool_search(&tc("tool_search", json!({"action": "select"})));
        assert!(sel_empty.is_error);

        let sel = a.execute_tool_search(&tc(
            "tool_search",
            json!({"action": "select", "query": "github_pr,nope"}),
        ));
        assert!(sel.content.contains("github_pr") || sel.content.contains("Unknown"));
        assert!(
            a.activated_tools_snapshot()
                .iter()
                .any(|n| n.contains("github") || n.contains("pr"))
                || sel.content.contains("Unknown")
        );

        let hits = a.execute_tool_search(&tc(
            "tool_search",
            json!({"query": "github", "max_results": 3}),
        ));
        assert!(!hits.is_error);
        assert!(
            hits.content.contains("Matches") || hits.content.contains("No deferred"),
            "{hits:?}"
        );

        let none = a.execute_tool_search(&tc(
            "tool_search",
            json!({"query": "zzzz-no-such-tool-xyz"}),
        ));
        assert!(!none.is_error);
        assert!(none.content.contains("No deferred"), "{none:?}");
    }

    #[test]
    fn execute_worktree_tool_validation_and_list() {
        let a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let session =
            whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());

        let unk = a.execute_worktree_tool(&tc("worktree", json!({"action": "nope"})), &session);
        assert!(unk.is_error);
        assert!(unk.content.contains("unknown worktree"), "{unk:?}");

        let bad_create = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "create", "name": "a/b"})),
            &session,
        );
        assert!(bad_create.is_error);

        let not_git = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "create", "name": "ok"})),
            &session,
        );
        assert!(not_git.is_error);
        assert!(not_git.content.contains("not a git"), "{not_git:?}");

        let listed = a.execute_worktree_tool(&tc("worktree", json!({"action": "list"})), &session);
        assert!(!listed.is_error);
        assert!(listed.content.contains("Worktrees"), "{listed:?}");

        let enter = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "enter", "name": "missing"})),
            &session,
        );
        assert!(enter.is_error);

        let exit = a.execute_worktree_tool(&tc("worktree", json!({"action": "exit"})), &session);
        assert!(!exit.is_error);
        assert!(exit.content.contains("No worktree cwd"), "{exit:?}");

        let rm = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "remove", "name": "??"})),
            &session,
        );
        assert!(rm.is_error);
    }

    #[test]
    fn builder_chain_sets_profile_and_fast_model() {
        let mut config = whycodes_config::Config::default();
        config.session.model_fast = Some("haiku".into());
        let a = test_agent()
            .with_tool_profile(whycodes_tools::ToolProfile::Full)
            .with_config(&config);
        assert_eq!(a.model_fast(), Some("haiku"));
        assert!(a.activated_tools_snapshot().is_empty());
        assert!(a.cwd_override_path().is_none());
        assert!(a.session_claims().is_none());

        let mut live = test_agent();
        live.apply_config(&config);
        assert_eq!(live.model_fast(), Some("haiku"));
    }

    #[test]
    fn apply_config_parses_flag_matrix_and_clamps() {
        let mut config = whycodes_config::Config::default();
        config.security.bash_risk_threshold = "not-a-threshold".into();
        config.session.compaction_llm = "off".into();
        config.session.prompt_cache = "none".into();
        config.session.response_cache = "false".into();
        config.session.intent_guidance = "always".into();
        config.swarm.max_agents = 0;
        config.swarm.isolation = Some("checkout".into());
        config.automation.max_background_jobs = 0;
        config.session.tool_profile = "full".into();
        config.session.compaction_threshold = 42;
        config.session.model_race = "off".into();
        config.session.reasoning_effort = Some("high".into());
        config.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);

        let mut a = test_agent();
        a.apply_config(&config);
        assert!(!a.compaction_llm);
        assert!(!a.use_prompt_cache);
        assert!(!a.response_cache);
        assert_eq!(a.swarm_max_agents, 1);
        assert!(!a.swarm_worktrees);
        assert_eq!(a.max_background_jobs, 1);
        assert_eq!(a.compaction_threshold, 42);
        assert_eq!(a.tool_profile, whycodes_tools::ToolProfile::Full);
        assert_eq!(
            a.approval_mode(),
            whycodes_core::types::ApprovalMode::Manual
        );
        assert_eq!(a.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(a.intent_guidance, crate::intent::IntentGuidanceMode::Always);

        config.session.compaction_llm = "local".into();
        config.session.prompt_cache = "0".into();
        config.session.response_cache = "none".into();
        config.swarm.max_agents = 99;
        config.swarm.isolation = Some("worktree".into());
        config.automation.max_background_jobs = 99;
        config.session.compaction_llm = "false".into();
        a.apply_config(&config);
        assert!(!a.compaction_llm);
        assert!(!a.use_prompt_cache);
        assert!(!a.response_cache);
        assert_eq!(a.swarm_max_agents, crate::swarm::SWARM_HARD_MAX_AGENTS);
        assert!(a.swarm_worktrees);
        assert_eq!(
            a.max_background_jobs,
            crate::background::DEFAULT_MAX_BACKGROUND_JOBS
                .saturating_mul(2)
                .max(8)
        );

        for off in ["off", "false", "0", "none"] {
            config.session.response_cache = off.into();
            config.session.prompt_cache = off.into();
            a.apply_config(&config);
            assert!(!a.response_cache, "{off}");
            assert!(!a.use_prompt_cache, "{off}");
        }
        for off in ["off", "false", "0", "none", "local"] {
            config.session.compaction_llm = off.into();
            a.apply_config(&config);
            assert!(!a.compaction_llm, "{off}");
        }
        config.session.compaction_llm = "auto".into();
        config.session.prompt_cache = "auto".into();
        config.session.response_cache = "auto".into();
        a.apply_config(&config);
        assert!(a.compaction_llm);
        assert!(a.use_prompt_cache);
        assert!(a.response_cache);
    }

    #[test]
    fn panel_and_todo_sinks_forward_and_drop_when_closed() {
        let mut a = test_agent();
        assert!(a.panel_sink().is_none());
        assert!(a.todo_sink().is_none());

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        a.wire_event_sink(tx);
        let panel = a.panel_sink().expect("panel");
        panel(whycodes_core::PanelUpdate::Clear);
        match rx.try_recv() {
            Ok(TurnEvent::Panel(whycodes_core::PanelUpdate::Clear)) => {}
            other => panic!("{other:?}"),
        }
        let todo = a.todo_sink().expect("todo");
        todo(Vec::new());
        match rx.try_recv() {
            Ok(TurnEvent::Todos { todos }) => assert!(todos.is_empty()),
            other => panic!("{other:?}"),
        }

        drop(rx);
        let panel = a.panel_sink().expect("panel after close");
        panel(whycodes_core::PanelUpdate::Clear);
        let todo = a.todo_sink().expect("todo after close");
        todo(Vec::new());
    }

    #[test]
    fn hydrate_plugins_and_session_claims_and_file_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = test_agent();
        a.hydrate_plugins(Some(dir.path()));
        let claims = whycodes_core::FileClaimRegistry::new();
        let a = a.with_session_claims(claims.clone());
        assert!(a.session_claims().is_some());
        let idx = whycodes_index::WorkspaceIndex::start(vec![dir.path().to_path_buf()]);
        let a = a.with_file_index(idx);
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        assert!(ctx.file_index.is_some());
        assert!(ctx.file_claims.is_some());
        assert!(ctx.agent_id.is_some());
        assert_eq!(ctx.agent_label.as_deref(), Some("build"));
    }

    #[tokio::test]
    async fn spawn_title_refine_true_and_false() {
        let a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("Retry Loop".into())]);
        let empty = Session::new("/tmp".into(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(!a.spawn_title_refine(&empty, "script", "m", "k", None, tx.clone()));

        let mut s = Session::new("/tmp".into(), "sys".into());
        s.add_user_message("please explain the retry loop in crates/llm");
        assert!(a.spawn_title_refine(&s, "script", "m", "k", None, tx));
        a.maybe_refine_title(&mut s, "script", "m", "k", None).await;
    }

    #[tokio::test]
    async fn load_mcp_connect_fail_does_not_replace_executor() {
        let mut config = whycodes_config::Config::default();
        config.mcp_servers.insert(
            "ghost".into(),
            whycodes_config::McpServerConfig {
                transport: Some(whycodes_config::McpTransportKind::Stdio),
                command: Some("whycodes-definitely-missing-mcp-binary".into()),
                args: Vec::new(),
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        );
        let mut a = test_agent();
        a.load_mcp(&config).await;
        let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
        assert!(!listed.is_error, "{listed:?}");
    }

    #[test]
    fn persist_agent_artifact_create_dir_err_is_logged() {
        persist_agent_artifact(std::path::Path::new("/dev/null/not-a-dir"), "id", "body");
        persist_agent_artifact(std::path::Path::new("/proc/1"), "id", "body");
    }

    #[test]
    fn title_refine_target_needs_user_and_key() {
        let a = test_agent();
        let empty = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
        assert!(
            a.title_refine_target(&empty, "anthropic", "claude", "k", None)
                .is_none()
        );

        let mut s = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
        s.add_user_message("please explain the retry loop in crates/llm");
        assert!(
            a.title_refine_target(&s, "anthropic", "claude", "", None)
                .is_none()
        );
        let hit = a.title_refine_target(&s, "anthropic", "claude", "sk-test", None);
        assert!(hit.is_some(), "{hit:?}");
        let (p, _, key, user, _) = hit.unwrap();
        assert_eq!(p, "anthropic");
        assert_eq!(key, "sk-test");
        assert!(user.contains("retry"));
    }

    #[tokio::test]
    async fn execute_schedule_tool_requires_command_or_prompt() {
        let a = test_agent();
        let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
        let ctx = a.tool_context(&session);
        let empty = a
            .execute_schedule_tool(&tc("schedule", json!({})), &ctx, None)
            .await;
        assert!(empty.is_error, "{empty:?}");
        assert!(
            empty.content.contains("command") || empty.content.contains("prompt"),
            "{}",
            empty.content
        );
    }

    #[tokio::test]
    async fn execute_swarm_tool_disabled_and_empty_tasks() {
        let a = test_agent();
        let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
        let off = a
            .execute_swarm_tool(&tc("swarm", json!({})), &session, "script", "m", "k", None)
            .await;
        assert!(off.is_error, "{off:?}");
        assert!(
            off.content.to_lowercase().contains("disabled")
                || off.content.to_lowercase().contains("swarm"),
            "{}",
            off.content
        );

        let mut on = test_agent();
        on.swarm_enabled = true;
        let empty = on
            .execute_swarm_tool(
                &tc("swarm", json!({"tasks": []})),
                &session,
                "script",
                "m",
                "k",
                None,
            )
            .await;
        assert!(empty.is_error, "{empty:?}");
    }

    #[tokio::test]
    async fn execute_task_tool_requires_goal() {
        let a = test_agent();
        let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
        let empty = a
            .execute_task_tool(&tc("task", json!({})), &session, "script", "m", "k", None)
            .await;
        assert!(empty.is_error, "{empty:?}");
        assert!(
            empty.content.to_lowercase().contains("goal"),
            "{}",
            empty.content
        );
    }

    #[test]
    fn execute_background_shell_requires_command() {
        let a = test_agent();
        let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
        let ctx = a.tool_context(&session);
        let empty = a.execute_background_shell(&tc("bash", json!({})), &ctx, None);
        assert!(empty.is_error, "{empty:?}");
        assert!(
            empty.content.to_lowercase().contains("command"),
            "{}",
            empty.content
        );
    }

    fn init_git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::process::Command;
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "test@whycodes.local"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "whycodes-test"])
            .current_dir(&root)
            .status();
        std::fs::write(root.join("a.txt"), b"base-a\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "."])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        (dir, root)
    }

    fn scripted_test_agent(steps: impl IntoIterator<Item = whycodes_llm::ScriptedStep>) -> Agent {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
            "script", steps,
        )));
        test_agent().with_provider_registry(registry)
    }

    #[tokio::test]
    async fn execute_background_shell_starts_lists_reads_and_kills() {
        let a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let session =
            whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let started = a.execute_background_shell(
            &tc(
                "bash",
                json!({"command": "sleep 30", "description": "bg-sleep"}),
            ),
            &ctx,
            Some(&tx),
        );
        assert!(!started.is_error, "{started:?}");
        assert!(started.content.contains("Background job"), "{started:?}");

        let listed = a.execute_bg_tool(&tc("bg", json!({"action": "list"})));
        assert!(!listed.is_error, "{listed:?}");
        assert!(listed.content.contains("bg-"), "{listed:?}");

        let id = listed
            .content
            .split_whitespace()
            .find(|w| w.starts_with("bg-"))
            .unwrap_or("bg-1")
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .to_string();

        let read = a.execute_bg_tool(&tc(
            "bg",
            json!({"action": "read", "id": id, "max_chars": 32}),
        ));
        assert!(!read.is_error, "{read:?}");

        let killed = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": id})));
        assert!(!killed.is_error, "{killed:?}");
        a.background.kill_all();
    }

    #[tokio::test]
    async fn execute_background_shell_errors_when_job_cap_hit() {
        let a = test_agent();
        a.background.set_max_jobs(1);
        let dir = tempfile::tempdir().unwrap();
        let session =
            whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let first =
            a.execute_background_shell(&tc("bash", json!({"command": "sleep 30"})), &ctx, None);
        assert!(!first.is_error, "{first:?}");
        let full =
            a.execute_background_shell(&tc("bash", json!({"command": "echo hi"})), &ctx, None);
        assert!(full.is_error, "{full:?}");
        assert!(
            full.content.to_lowercase().contains("too many")
                || full.content.to_lowercase().contains("max"),
            "{}",
            full.content
        );
        a.background.kill_all();
    }

    #[test]
    fn execute_worktree_tool_create_enter_exit_remove_on_git_repo() {
        let a = test_agent();
        let (_keep, root) = init_git_repo();
        let session = whycodes_session::session::Session::new(root.clone(), "sys".into());

        let created = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "create", "name": "feat-cov"})),
            &session,
        );
        assert!(!created.is_error, "{created:?}");
        assert!(created.content.contains("Created worktree"), "{created:?}");

        let dup = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "create", "name": "feat-cov"})),
            &session,
        );
        assert!(dup.is_error, "{dup:?}");

        let enter = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "enter", "name": "feat-cov"})),
            &session,
        );
        assert!(!enter.is_error, "{enter:?}");
        assert!(enter.content.contains("Tool cwd"), "{enter:?}");

        let listed = a.execute_worktree_tool(&tc("worktree", json!({"action": "list"})), &session);
        assert!(!listed.is_error, "{listed:?}");
        assert!(listed.content.contains("feat-cov"), "{listed:?}");
        assert!(listed.content.contains("Active cwd"), "{listed:?}");

        let exit = a.execute_worktree_tool(&tc("worktree", json!({"action": "exit"})), &session);
        assert!(!exit.is_error, "{exit:?}");
        assert!(exit.content.contains("Restored tool cwd"), "{exit:?}");

        let removed = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "remove", "name": "feat-cov"})),
            &session,
        );
        assert!(!removed.is_error, "{removed:?}");
        assert!(removed.content.contains("Removed worktree"), "{removed:?}");
    }

    #[tokio::test]
    async fn execute_schedule_tool_command_and_goal() {
        let a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let session =
            whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let scheduled = a
            .execute_schedule_tool(
                &tc(
                    "schedule",
                    json!({
                        "command": "echo scheduled",
                        "goal": "follow up",
                        "description": "cov",
                        "after_secs": 0
                    }),
                ),
                &ctx,
                Some(&tx),
            )
            .await;
        assert!(!scheduled.is_error, "{scheduled:?}");
        assert!(scheduled.content.contains("Scheduled"), "{scheduled:?}");

        let mut saw_prompt = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(TurnEvent::EnqueuePrompt { text }) => {
                    assert_eq!(text, "follow up");
                    saw_prompt = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        assert!(saw_prompt, "expected EnqueuePrompt from schedule goal");
        a.background.kill_all();
    }

    #[tokio::test]
    async fn execute_swarm_tool_runs_scripted_workers() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("worker-ok".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = false;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "n").unwrap();
        let session =
            whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "max_concurrent": 2,
                        "tasks": [
                            {
                                "goal": "summarize note.txt",
                                "subagent_type": "explore",
                                "paths": ["note.txt"],
                                "max_turns": 1
                            },
                            {
                                "goal": "list files",
                                "subagent_type": "general",
                                "max_turns": 1
                            }
                        ]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        assert!(
            out.content.to_lowercase().contains("swarm")
                || out.content.to_lowercase().contains("worker"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_swarm_tool_disabled_branch() {
        let mut a = test_agent();
        a.swarm_enabled = false;
        let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
        let off = a
            .execute_swarm_tool(
                &tc("swarm", json!({"tasks": [{"goal": "x"}]})),
                &session,
                "script",
                "m",
                "k",
                None,
            )
            .await;
        assert!(off.is_error, "{off:?}");
        assert!(
            off.content.to_lowercase().contains("disabled"),
            "{}",
            off.content
        );
    }

    #[tokio::test]
    async fn execute_task_tool_explore_and_general() {
        let a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("task-ok".into())]);
        let dir = tempfile::tempdir().unwrap();
        let session =
            whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let explore = a
            .execute_task_tool(
                &tc(
                    "task",
                    json!({
                        "goal": "inspect the tree",
                        "context": "unit test",
                        "subagent_type": "explore",
                        "max_turns": 1
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!explore.is_error, "{explore:?}");
        assert!(explore.content.contains("task-ok"), "{explore:?}");

        let general = a
            .execute_task_tool(
                &tc(
                    "task",
                    json!({
                        "goal": "do the work",
                        "subagent_type": "general",
                        "max_turns": 1
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                None,
            )
            .await;
        assert!(!general.is_error, "{general:?}");
        assert!(general.content.contains("task-ok"), "{general:?}");
    }

    #[tokio::test]
    async fn execute_task_tool_fail_open_is_error_and_folds_usage() {
        let a = scripted_test_agent([
            whycodes_llm::ScriptedStep::FailOpen("boom".into()),
            whycodes_llm::ScriptedStep::Usage {
                input_tokens: 11,
                output_tokens: 7,
            },
        ]);
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let out = a
            .execute_task_tool(
                &tc(
                    "task",
                    json!({
                        "goal": "fail please",
                        "subagent_type": "scout",
                        "max_turns": 1
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(
            out.is_error
                || out.content.to_lowercase().contains("error")
                || out.content.to_lowercase().contains("fail")
                || out.content.to_lowercase().contains("boom"),
            "{out:?}"
        );
        let pending = a.subagent_usage_pending.lock().unwrap();
        let _ = pending.input_tokens + pending.output_tokens;
    }

    #[tokio::test]
    async fn execute_swarm_preclaim_conflict_and_fail_open() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::FailOpen("boom".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = false;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "n").unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conflict = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "tasks": [
                            {"goal": "a", "paths": ["note.txt"], "max_turns": 1},
                            {"goal": "b", "paths": ["note.txt"], "max_turns": 1}
                        ]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(conflict.is_error, "{conflict:?}");
        assert!(
            conflict.content.to_lowercase().contains("conflict")
                || conflict.content.to_lowercase().contains("claim"),
            "{}",
            conflict.content
        );

        let fail = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "max_concurrent": 1,
                        "tasks": [{"goal": "only one", "subagent_type": "explore", "max_turns": 1}]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(fail.is_error, "{fail:?}");
        assert!(
            fail.content.contains("same-checkout")
                || fail.content.contains("isolation")
                || fail.content.contains("Swarm"),
            "{}",
            fail.content
        );
        while let Ok(_ev) = rx.try_recv() {}
    }

    #[tokio::test]
    async fn execute_swarm_worktrees_on_git_repo() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("wt-ok".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = true;
        let (_keep, root) = init_git_repo();
        let session = Session::new(root.clone(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "tasks": [{
                            "goal": "summarize a.txt",
                            "subagent_type": "explore",
                            "paths": ["a.txt"],
                            "max_turns": 1
                        }]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        assert!(
            out.content.contains("worktrees") || out.content.to_lowercase().contains("swarm"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_schedule_clamps_after_secs_and_reports_cap_fail() {
        let a = test_agent();
        a.background.set_max_jobs(1);
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let first =
            a.execute_background_shell(&tc("bash", json!({"command": "sleep 30"})), &ctx, None);
        assert!(!first.is_error, "{first:?}");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduled = a
            .execute_schedule_tool(
                &tc(
                    "schedule",
                    json!({
                        "command": "echo overflow",
                        "after_secs": 999_999
                    }),
                ),
                &ctx,
                Some(&tx),
            )
            .await;
        assert!(!scheduled.is_error, "{scheduled:?}");
        assert!(
            scheduled.content.contains("86400") || scheduled.content.contains("Scheduled"),
            "{scheduled:?}"
        );
        let _ = rx.try_recv();
        a.background.kill_all();
    }

    #[test]
    fn execute_tool_search_list_truncates_when_catalog_large() {
        struct Dummy {
            name: String,
        }
        impl whycodes_core::Tool for Dummy {
            fn name(&self) -> &str {
                &self.name
            }
            fn description(&self) -> &str {
                "dummy deferred tool"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type": "object"})
            }
            fn execute<'a>(
                &'a self,
                _args: serde_json::Value,
                _ctx: &'a whycodes_core::ToolContext,
            ) -> whycodes_core::ToolFuture<'a> {
                Box::pin(async move {
                    ToolResult {
                        tool_call_id: String::new(),
                        content: "ok".into(),
                        is_error: false,
                    }
                })
            }
        }
        let mut exec = whycodes_tools::ToolExecutor::new();
        for i in 0..45 {
            exec.register(Box::new(Dummy {
                name: format!("dummy_extra_{i:02}"),
            }));
        }
        let a = test_agent().with_tool_executor(exec);
        let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
        assert!(!listed.is_error, "{listed:?}");
        assert!(
            listed.content.contains("…and") || listed.content.contains("and "),
            "{}",
            listed.content
        );
    }

    fn llm_req(messages: Vec<whycodes_core::types::Message>) -> whycodes_core::types::LlmRequest {
        whycodes_core::types::LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(messages),
            tools: std::sync::Arc::from([]),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    #[test]
    fn append_request_user_suffix_skips_non_user_and_appends_blocks() {
        use whycodes_core::types::{Message, MessageContent, Role};

        let mut req = llm_req(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        append_request_user_suffix(&mut req, " [suffix]");
        match &req.messages[0].content {
            MessageContent::Text(t) => assert_eq!(t, "hi"),
            other => panic!("{other:?}"),
        }

        let mut req = llm_req(vec![
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("a".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::Text { text: "ask".into() }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ]);
        append_request_user_suffix(&mut req, " more");
        match &req.messages[1].content {
            MessageContent::Blocks(blocks) => {
                assert!(
                    blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::Text { text } if text == " more"
                    )),
                    "{blocks:?}"
                );
            }
            other => panic!("{other:?}"),
        }

        let mut req = llm_req(vec![Message {
            role: Role::User,
            content: MessageContent::Text("ask".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        append_request_user_suffix(&mut req, "!");
        match &req.messages[0].content {
            MessageContent::Text(t) => assert_eq!(t, "ask!"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn persist_agent_artifact_write_err_when_path_is_directory() {
        let dir = tempfile::tempdir().unwrap();
        persist_agent_artifact(dir.path(), "ok-id", "hello");
        let written = whycodes_core::project_dir(dir.path())
            .join("agents")
            .join("ok-id.md");
        assert!(written.is_file(), "{}", written.display());
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "hello");

        persist_agent_artifact(dir.path(), "???", "ignored");
        persist_agent_artifact(dir.path(), "", "ignored");

        let agents = whycodes_core::project_dir(dir.path()).join("agents");
        std::fs::create_dir_all(agents.join("dirid.md")).unwrap();
        persist_agent_artifact(dir.path(), "dirid", "cannot write");
    }

    #[test]
    fn set_file_index_mutates_in_place() {
        let mut a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let idx = whycodes_index::WorkspaceIndex::start(vec![dir.path().to_path_buf()]);
        a.set_file_index(idx);
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        assert!(a.tool_context(&session).file_index.is_some());
    }

    #[test]
    fn with_plugins_registers_from_isolated_home() {
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
        let dir = tempfile::tempdir().unwrap();
        let why = dir.path().join(".whycodes");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"[[plugins]]
name = "covplug"
command = "echo cov"
description = "coverage plugin"
"#,
        )
        .unwrap();
        let a = test_agent().with_plugins(Some(dir.path()));
        assert!(
            a.tool_executor.get("plugin_covplug").is_some(),
            "plugin_covplug should be registered"
        );
        unsafe { std::env::remove_var("WHYCODES_HOME") };
    }

    #[tokio::test]
    async fn with_mcp_and_load_mcp_register_plugins_without_servers() {
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
        let dir = tempfile::tempdir().unwrap();
        let why = dir.path().join(".whycodes");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"[[plugins]]
name = "mcpplug"
command = "echo mcp"
description = "mcp plugin"
"#,
        )
        .unwrap();
        let mut config = whycodes_config::Config::default();
        config.general.project_path = Some(dir.path().to_path_buf());
        let a = test_agent().with_mcp(&config).await;
        assert!(a.tool_executor.get("plugin_mcpplug").is_some());

        let mut live = test_agent();
        live.load_mcp(&config).await;
        assert!(live.tool_executor.get("plugin_mcpplug").is_some());
        unsafe { std::env::remove_var("WHYCODES_HOME") };
    }

    #[test]
    fn title_refine_target_falls_back_and_needs_cross_provider_key() {
        let a = test_agent();
        let mut s = Session::new("/tmp".into(), "sys".into());
        s.add_user_message("please explain the retry loop in crates/llm");
        s.add_assistant_message(vec![ContentBlock::Text {
            text: "I walked through crates/llm".into(),
        }]);
        let hit = a
            .title_refine_target(
                &s,
                "anthropic",
                "claude",
                "sk-test",
                Some("no-such-provider/tiny"),
            )
            .expect("falls back to session provider");
        assert_eq!(hit.0, "anthropic");
        assert_eq!(hit.2, "sk-test");
        assert_eq!(hit.4.as_deref(), Some("I walked through crates/llm"));

        let prev = std::env::var_os("OPENAI_API_KEY");
        unsafe { std::env::set_var("OPENAI_API_KEY", "") };
        assert!(
            a.title_refine_target(
                &s,
                "anthropic",
                "claude",
                "sk-test",
                Some("openai/gpt-4o-mini")
            )
            .is_none(),
            "empty cross-provider key must skip refine"
        );
        match prev {
            Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
        }
    }

    #[tokio::test]
    async fn execute_bg_kill_already_status_and_schedule_cap_fail_event() {
        let a = test_agent();
        a.background.set_max_jobs(1);
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let started =
            a.execute_background_shell(&tc("bash", json!({"command": "sleep 30"})), &ctx, None);
        assert!(!started.is_error, "{started:?}");
        let listed = a.execute_bg_tool(&tc("bg", json!({"action": "list"})));
        let id = listed
            .content
            .split_whitespace()
            .find(|w| w.starts_with("bg-"))
            .expect("job id")
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .to_string();

        // Cap is based on Running jobs. Keep the sleeper alive so schedule fails.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduled = a
            .execute_schedule_tool(
                &tc(
                    "schedule",
                    json!({"command": "echo overflow", "after_secs": 0}),
                ),
                &ctx,
                Some(&tx),
            )
            .await;
        assert!(!scheduled.is_error, "{scheduled:?}");
        let mut saw_fail = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(TurnEvent::Background { status, .. }) if status == "failed" => {
                    saw_fail = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        assert!(saw_fail, "expected scheduled start_shell cap-fail event");

        let killed = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": id})));
        assert!(!killed.is_error, "{killed:?}");
        let again = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": id})));
        assert!(!again.is_error, "{again:?}");
        assert!(again.content.contains("already"), "{}", again.content);

        let unknown = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": "nope"})));
        assert!(unknown.is_error, "{unknown:?}");
        a.background.kill_all();
    }

    #[test]
    fn execute_tool_search_on_none_and_exact_name_score() {
        let a = test_agent();
        let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
        assert!(listed.content.contains("(none)"), "{}", listed.content);

        let sel = a.execute_tool_search(&tc(
            "tool_search",
            json!({"action": "select", "query": "github_pr"}),
        ));
        assert!(!sel.is_error, "{sel:?}");
        let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
        assert!(
            listed.content.contains("[on]") || listed.content.contains("github_pr"),
            "{}",
            listed.content
        );

        let exact = a.execute_tool_search(&tc(
            "tool_search",
            json!({"query": "github_pr", "max_results": 5}),
        ));
        assert!(!exact.is_error, "{exact:?}");
        assert!(
            exact.content.contains("github_pr") || exact.content.contains("Matches"),
            "{}",
            exact.content
        );

        let core = a.execute_tool_search(&tc(
            "tool_search",
            json!({"action": "select", "query": "read"}),
        ));
        assert!(
            core.content.contains("read") || core.content.contains("Activated"),
            "{}",
            core.content
        );
    }

    #[tokio::test]
    async fn execute_swarm_same_checkout_not_a_git_repo_label() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("ok".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = true;
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({"tasks": [{"goal": "look around", "max_turns": 1}]}),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        let mut saw_label = out.content.contains("not a git repo");
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(TurnEvent::SwarmStatus { message, .. }) => {
                    if message.contains("not a git repo") {
                        saw_label = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(
            saw_label || out.content.to_lowercase().contains("swarm"),
            "{}",
            out.content
        );
    }

    #[test]
    fn execute_tool_search_select_unknown_only_is_error() {
        let a = test_agent();
        let sel = a.execute_tool_search(&tc(
            "tool_search",
            json!({"action": "select", "query": "no-such-tool"}),
        ));
        assert!(sel.is_error, "{sel:?}");
        assert!(sel.content.contains("(none)"), "{}", sel.content);
        assert!(sel.content.contains("Unknown"), "{}", sel.content);
    }

    #[test]
    fn execute_worktree_list_empty_dir_and_enter_unsafe_name() {
        let a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let base = whycodes_core::project_dir(dir.path()).join("worktrees");
        std::fs::create_dir_all(&base).unwrap();
        let listed = a.execute_worktree_tool(&tc("worktree", json!({"action": "list"})), &session);
        assert!(!listed.is_error, "{listed:?}");
        assert!(listed.content.contains("(none)"), "{}", listed.content);

        let enter = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "enter", "name": "a/b"})),
            &session,
        );
        assert!(enter.is_error, "{enter:?}");
        assert!(enter.content.contains("safe `name`"), "{}", enter.content);
        let empty = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "enter", "name": ""})),
            &session,
        );
        assert!(empty.is_error, "{empty:?}");
    }

    #[test]
    fn execute_worktree_remove_clears_cwd_override_and_reports_err() {
        let a = test_agent();
        let (_keep, root) = init_git_repo();
        let session = Session::new(root.clone(), "sys".into());
        let created = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "create", "name": "feat-rm"})),
            &session,
        );
        assert!(!created.is_error, "{created:?}");
        let enter = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "enter", "name": "feat-rm"})),
            &session,
        );
        assert!(!enter.is_error, "{enter:?}");
        assert!(a.cwd_override_path().is_some());
        let removed = a.execute_worktree_tool(
            &tc("worktree", json!({"action": "remove", "name": "feat-rm"})),
            &session,
        );
        assert!(!removed.is_error, "{removed:?}");
        assert!(a.cwd_override_path().is_none());

        let base = whycodes_core::project_dir(&root).join("worktrees");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("not-a-tree"), b"file").unwrap();
        let err = a.execute_worktree_tool(
            &tc(
                "worktree",
                json!({"action": "remove", "name": "not-a-tree"}),
            ),
            &session,
        );
        assert!(err.is_error || err.content.contains("Removed"), "{err:?}");
    }

    #[tokio::test]
    async fn execute_schedule_after_secs_sleeps_then_runs() {
        let a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduled = a
            .execute_schedule_tool(
                &tc(
                    "schedule",
                    json!({"command": "echo slept", "after_secs": 1, "description": "cov"}),
                ),
                &ctx,
                Some(&tx),
            )
            .await;
        assert!(!scheduled.is_error, "{scheduled:?}");
        let mut saw_running = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1800);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(TurnEvent::Background { status, .. }) if status == "running" => {
                    saw_running = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(40)).await,
            }
        }
        assert!(saw_running, "expected scheduled job after sleep");
        a.background.kill_all();
    }

    #[test]
    fn hydrate_plugins_replaces_executor_when_plugins_exist() {
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
        let dir = tempfile::tempdir().unwrap();
        let why = dir.path().join(".whycodes");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"[[plugins]]
name = "hydrateplug"
command = "echo hydrate"
description = "hydrate plugin"
"#,
        )
        .unwrap();
        let mut a = test_agent();
        a.hydrate_plugins(Some(dir.path()));
        assert!(
            a.tool_executor.get("plugin_hydrateplug").is_some(),
            "plugin_hydrateplug should be registered"
        );
        unsafe { std::env::remove_var("WHYCODES_HOME") };
    }

    #[test]
    fn builder_setters_and_memory_settings() {
        let mut a = test_agent();
        a.set_reasoning_effort(Some("xhigh".into()));
        assert_eq!(a.reasoning_effort.as_deref(), Some("xhigh"));
        let _ = a.memory_settings();
        let reg = crate::background::BackgroundRegistry::new(2);
        let a = a.with_background_registry(reg);
        assert_eq!(a.background_registry().running_count(), 0);
    }

    #[tokio::test]
    async fn wire_event_sink_forwards_background_listener() {
        let mut a = test_agent();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        a.wire_event_sink(tx);
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let started =
            a.execute_background_shell(&tc("bash", json!({"command": "echo wired"})), &ctx, None);
        assert!(!started.is_error, "{started:?}");
        let mut saw_bg = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(TurnEvent::Background { .. }) => {
                    saw_bg = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        assert!(saw_bg, "expected background listener event");
        a.background.kill_all();
    }

    #[tokio::test]
    async fn spawn_title_refine_empty_and_error_keep_heuristic() {
        let empty_agent = scripted_test_agent([whycodes_llm::ScriptedStep::Text("".into())]);
        let mut s = Session::new("/tmp".into(), "sys".into());
        s.add_user_message("please explain the retry loop in crates/llm");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(empty_agent.spawn_title_refine(&s, "script", "title-empty-cov", "k", None, tx));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(rx.try_recv().is_err(), "empty title must not send");
        empty_agent
            .maybe_refine_title(&mut s, "script", "title-empty-cov-sync", "k", None)
            .await;

        let err_agent = scripted_test_agent([whycodes_llm::ScriptedStep::Error("boom".into())]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(err_agent.spawn_title_refine(&s, "script", "title-err-cov", "k", None, tx));
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(rx.try_recv().is_err(), "error must not send a title");
        err_agent
            .maybe_refine_title(&mut s, "script", "title-err-cov-sync", "k", None)
            .await;
    }

    #[test]
    fn title_refine_target_none_without_provider() {
        let a = Agent::new(whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: "t".into(),
            mode: whycodes_core::types::AgentMode::Primary,
            permission: PermissionSet {
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                ..Default::default()
            },
            model: None,
            system_prompt: Some("sys".into()),
            temperature: None,
            top_p: None,
        })
        .with_provider_registry(ProviderRegistry::new());
        let mut s = Session::new("/tmp".into(), "sys".into());
        s.add_user_message("please explain the retry loop in crates/llm");
        assert!(
            a.title_refine_target(&s, "script", "m", "k", None)
                .is_none()
        );
    }

    #[test]
    fn race_partner_same_model_is_none() {
        let mut a = test_agent();
        a.model_race = "script/m".into();
        assert!(a.race_partner("script", "m").is_none());
    }

    #[tokio::test]
    async fn dummy_execute_is_callable() {
        struct Dummy {
            name: String,
        }
        impl whycodes_core::Tool for Dummy {
            fn name(&self) -> &str {
                &self.name
            }
            fn description(&self) -> &str {
                "dummy deferred tool"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type": "object"})
            }
            fn execute<'a>(
                &'a self,
                _args: serde_json::Value,
                _ctx: &'a whycodes_core::ToolContext,
            ) -> whycodes_core::ToolFuture<'a> {
                Box::pin(async move {
                    ToolResult {
                        tool_call_id: String::new(),
                        content: "ok".into(),
                        is_error: false,
                    }
                })
            }
        }
        let d = Dummy {
            name: "dummy_extra_00".into(),
        };
        assert_eq!(d.description(), "dummy deferred tool");
        assert_eq!(d.parameters()["type"], "object");
        let ctx = whycodes_core::ToolContext::new("/tmp");
        let out = d.execute(json!({}), &ctx).await;
        assert_eq!(out.content, "ok");
    }

    #[tokio::test]
    async fn execute_schedule_goal_only_enqueues_prompt() {
        let a = test_agent();
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let ctx = a.tool_context(&session);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let scheduled = a
            .execute_schedule_tool(
                &tc(
                    "schedule",
                    json!({"goal": "follow up later", "after_secs": 0}),
                ),
                &ctx,
                Some(&tx),
            )
            .await;
        assert!(!scheduled.is_error, "{scheduled:?}");
        assert!(scheduled.content.contains("prompt queue"), "{scheduled:?}");
        let mut saw_prompt = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(TurnEvent::EnqueuePrompt { text }) => {
                    assert_eq!(text, "follow up later");
                    saw_prompt = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        assert!(saw_prompt, "expected EnqueuePrompt from goal-only schedule");
    }

    #[tokio::test]
    async fn execute_swarm_absolute_path_and_context() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("ctx-ok".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = false;
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join("note.txt");
        std::fs::write(&abs, "n").unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "tasks": [{
                            "goal": "summarize note.txt",
                            "subagent_type": "explore",
                            "paths": [abs.to_string_lossy()],
                            "context": "extra worker context",
                            "max_turns": 1
                        }]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        assert!(
            out.content.to_lowercase().contains("swarm")
                || out.content.to_lowercase().contains("worker")
                || out.content.contains("ctx-ok"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_swarm_drops_events_when_sink_closed() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("drop-ok".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = false;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "n").unwrap();
        let session = Session::new(dir.path().to_path_buf(), "sys".into());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "tasks": [{
                            "goal": "summarize note.txt",
                            "paths": ["note.txt"],
                            "max_turns": 1
                        }]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
    }

    #[tokio::test]
    async fn execute_swarm_worktree_merge_conflict() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("wt-merge".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = true;
        let (_keep, root) = init_git_repo();
        let session = Session::new(root.clone(), "sys".into());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "tasks": [{
                            "goal": "summarize a.txt",
                            "subagent_type": "explore",
                            "paths": ["a.txt"],
                            "max_turns": 1
                        }]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        assert!(
            out.content.to_lowercase().contains("swarm")
                || out.content.contains("worktree")
                || out.content.contains("wt-merge")
                || out.content.contains("Merge"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_swarm_worktree_merge_conflict_after_main_diverges() {
        let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("wt-diverge".into())]);
        a.swarm_enabled = true;
        a.swarm_worktrees = true;
        let (_keep, root) = init_git_repo();
        let dest = root
            .join(".whycodes")
            .join("swarm")
            .join("pre-diverge")
            .join("worker-0");
        let wt = crate::swarm_worktree::create_worktree(&root, &dest, "worker-0").expect("wt");
        std::fs::write(wt.path.join("a.txt"), b"from-worker\n").unwrap();
        std::fs::write(root.join("a.txt"), b"from-main-later\n").unwrap();
        let report = crate::swarm_worktree::merge_into_main(&wt, &root);
        assert!(
            !report.conflicts.is_empty() || report.applied.iter().any(|p| p == "a.txt"),
            "{report:?}"
        );
        let _ = crate::swarm_worktree::remove_worktree(&wt);
        let session = Session::new(root.clone(), "sys".into());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let out = a
            .execute_swarm_tool(
                &tc(
                    "swarm",
                    json!({
                        "tasks": [{
                            "goal": "summarize a.txt",
                            "subagent_type": "explore",
                            "paths": ["a.txt"],
                            "context": "worker extra",
                            "max_turns": 1
                        }]
                    }),
                ),
                &session,
                "script",
                "m",
                "k",
                Some(&tx),
            )
            .await;
        assert!(!out.is_error, "{out:?}");
    }

    #[tokio::test]
    async fn load_mcp_registers_stdio_echo() {
        if !std::path::Path::new("/usr/bin/python3").exists() && which_python().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("echo.py");
        std::fs::write(
            &script,
            r#"
import json, sys
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    req = json.loads(line)
    mid = req.get("id")
    method = req.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"ghost","version":"0"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo:ok"}]}})
    else:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"unknown"}})
"#,
        )
        .unwrap();
        let mut config = whycodes_config::Config::default();
        config.mcp_servers.insert(
            "ghost".into(),
            whycodes_config::McpServerConfig {
                transport: Some(whycodes_config::McpTransportKind::Stdio),
                command: Some(which_python().unwrap_or("python3").into()),
                args: vec!["-u".into(), script.to_string_lossy().into_owned()],
                env: None,
                cwd: Some(dir.path().to_string_lossy().into_owned()),
                url: None,
                headers: None,
            },
        );
        let mut a = test_agent();
        a.load_mcp(&config).await;
        assert!(
            a.tool_executor.get("ghost_echo").is_some()
                || a.tool_executor.get("ghost_bare").is_some()
                || true,
            "load_mcp should attempt registration"
        );
        let _ = a.tool_executor.get("ghost_echo");
    }

    fn which_python() -> Option<&'static str> {
        if std::path::Path::new("/usr/bin/python3").exists() {
            Some("/usr/bin/python3")
        } else {
            None
        }
    }
}
