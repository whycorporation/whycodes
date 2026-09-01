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

    pub fn with_tool_executor(mut self, executor: ToolExecutor) -> Self {
        self.tool_executor = Arc::new(executor);
        self
    }

    /// Share the workspace file index with this agent's file tools.
    pub fn with_file_index(mut self, index: Arc<whycodes_index::WorkspaceIndex>) -> Self {
        self.file_index = Some(index);
        self
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
        self
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
}
