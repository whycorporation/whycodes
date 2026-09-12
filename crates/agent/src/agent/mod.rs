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
use std::sync::{Arc, Mutex, MutexGuard};

use whycodes_core::SandboxSettings;
use whycodes_core::network::NetworkPolicy;
use whycodes_core::tool::ToolContext;
use whycodes_core::types::{AgentInfo, ApprovalMode, ContentBlock, ToolCall, ToolResult};
use whycodes_llm::provider::{LlmProvider, ProviderRegistry};
use whycodes_session::session::Session;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::profile::ToolProfile;

use crate::events::{EventSink, TurnEvent};
use crate::permission::{PermissionPrompter, default_prompter};
use crate::question::{QuestionPrompter, default_question_prompter};
use whycodes_command_risk::RiskThreshold;
use whycodes_config::{HookConfig, NotifyConfig, SystemPromptOverlays};

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
    /// `[lsp]` overlay from config (empty = built-in auto-detect).
    lsp_overlay: whycodes_tools::LspSettings,
    /// Provider/model extras from `prompts/*.md`.
    system_prompt_overlays: SystemPromptOverlays,
    /// Active provider id for overlay lookup (`/models`, session route).
    route_provider: String,
    /// Active model id for overlay lookup.
    route_model: String,
}

/// Recover from a poisoned mutex instead of aborting (`panic = "abort"` in release).
pub(crate) fn recover_lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
    // `load_project` is infallible today (unreadable skill dirs are skipped).
    let catalog = catalog_from_load(whycodes_skill::SkillRegistry::load_project(project_path));
    if catalog.is_empty() {
        return system_prompt.to_string();
    }
    format!("{system_prompt}\n\n{catalog}")
}

fn catalog_from_load(
    loaded: Result<whycodes_skill::SkillRegistry, whycodes_skill::SkillError>,
) -> String {
    match loaded {
        Ok(reg) => reg.catalog_markdown(),
        Err(_load) => String::new(),
    }
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
    let args = json_or_object(&tc.arguments);
    format!("{}|{args}", tc.name)
}

fn json_or_object(value: &serde_json::Value) -> String {
    json_string_or(serde_json::to_string(value), "{}")
}

fn json_string_or(result: Result<String, serde_json::Error>, fallback: &str) -> String {
    result.unwrap_or_else(|_json| fallback.to_string())
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
            lsp_overlay: whycodes_tools::LspSettings::default(),
            system_prompt_overlays: SystemPromptOverlays::default(),
            route_provider: String::new(),
            route_model: String::new(),
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
        self.apply_lsp_overlay(&mut exec);
        let n = exec.register_config_plugins(project_dir);
        if n > 0
            || !self.lsp_overlay.servers.is_empty()
            || self.lsp_overlay.idle_timeout_ms.is_some()
        {
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

    /// Bind the active provider/model so [`Self::system_prompt`] can append
    /// matching `prompts/*.md` extras.
    pub fn set_route(&mut self, provider: &str, model: &str) {
        self.route_provider = provider.to_string();
        self.route_model = model.to_string();
    }

    pub fn route(&self) -> (&str, &str) {
        (&self.route_provider, &self.route_model)
    }

    /// Replace loaded `prompts/*.md` extras (TUI hydrate after first paint).
    pub fn set_system_prompt_overlays(&mut self, overlays: SystemPromptOverlays) {
        self.system_prompt_overlays = overlays;
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
        self.lsp_overlay = lsp_overlay_from_config(&config.lsp);
        self.system_prompt_overlays = config.system_prompt_overlays.clone();
        let sandbox_desc = whycodes_sandbox::describe_backend(&self.sandbox);
        let network_allow = self.network.allowlist.len();
        let network_deny = self.network.denylist.len();
        let hooks = self.hooks.len();
        let compaction_threshold = self.compaction_threshold;
        let tool_profile = self.tool_profile.as_str();
        let use_prompt_cache = self.use_prompt_cache;
        let model_race = self.model_race.as_str();
        let response_cache = self.response_cache;
        let memory_enabled = self.memory.enabled;
        let intent_guidance = self.intent_guidance;
        let swarm_enabled = self.swarm_enabled;
        let swarm_max_agents = self.swarm_max_agents;
        let swarm_worktrees = self.swarm_worktrees;
        let max_background_jobs = self.max_background_jobs;
        tracing::debug!(
            sandbox = %sandbox_desc,
            network_allow,
            network_deny,
            hooks,
            compaction_threshold,
            tool_profile,
            use_prompt_cache,
            model_race,
            response_cache,
            memory_enabled,
            intent_guidance = ?intent_guidance,
            swarm_enabled,
            swarm_max_agents,
            swarm_worktrees,
            max_background_jobs,
            "shell sandbox, network policy, and hooks"
        );
    }

    /// Forward `panel` tool updates onto the turn event channel.
    pub(crate) fn panel_sink(&self) -> Option<whycodes_core::PanelSink> {
        let tx = self.event_sink.clone()?;
        Some(std::sync::Arc::new(move |update| {
            if let Err(e) = tx.send(TurnEvent::Panel(update)) {
                let error = e.to_string();
                tracing::debug!(error = %error, "panel event dropped (listener closed)");
            }
        }))
    }

    /// Forward `todowrite` updates onto the turn event channel.
    pub(crate) fn todo_sink(&self) -> Option<whycodes_core::TodoSink> {
        let tx = self.event_sink.clone()?;
        Some(std::sync::Arc::new(move |todos| {
            if let Err(e) = tx.send(TurnEvent::Todos { todos }) {
                let error = e.to_string();
                tracing::debug!(error = %error, "todo event dropped (listener closed)");
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
        let working_dir = recover_lock(&self.cwd_override)
            .as_ref()
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
        let mut v: Vec<_> = recover_lock(&self.activated_tools)
            .iter()
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// Active tool cwd (worktree enter), if any.
    pub fn cwd_override_path(&self) -> Option<std::path::PathBuf> {
        recover_lock(&self.cwd_override).clone()
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
        self.lsp_overlay = lsp_overlay_from_config(&config.lsp);
        let project = config.general.project_path.as_deref();
        let mut full = ToolExecutor::new();
        self.apply_lsp_overlay(&mut full);
        let n_plug = full.register_config_plugins(project);
        let n_mcp = if config.mcp_servers.is_empty() {
            0
        } else {
            crate::mcp_load::register_mcp_tools(&mut full, config).await
        };
        let lsp_custom =
            !self.lsp_overlay.servers.is_empty() || self.lsp_overlay.idle_timeout_ms.is_some();
        if n_plug > 0 || n_mcp > 0 || lsp_custom {
            self.tool_executor = Arc::new(full);
            log_registered_count(n_plug, "shell plugins registered");
            log_registered_count(n_mcp, "MCP tools registered");
        }
    }

    /// Load shell plugins only (when not calling [`Self::with_mcp`]).
    pub fn with_plugins(mut self, project_dir: Option<&std::path::Path>) -> Self {
        let mut exec = ToolExecutor::new();
        self.apply_lsp_overlay(&mut exec);
        let n = exec.register_config_plugins(project_dir);
        apply_plugin_count(&mut self, exec, n);
        self
    }

    fn apply_lsp_overlay(&self, exec: &mut ToolExecutor) {
        if self.lsp_overlay.servers.is_empty() && self.lsp_overlay.idle_timeout_ms.is_none() {
            return;
        }
        exec.configure_lsp(&self.lsp_overlay);
    }

    /// Get the system prompt for this agent (includes runtime context such as today's date).
    pub fn system_prompt(&self) -> String {
        let base = self
            .info
            .system_prompt
            .clone()
            .unwrap_or_else(|| Self::system_prompt_for(&self.info.name));
        Self::with_runtime_context(&Self::with_prompt_overlays(
            &base,
            &self.system_prompt_overlays,
            &self.route_provider,
            &self.route_model,
        ))
    }

    /// Role prompt plus provider/model extras from `prompts/*.md` (no runtime).
    pub fn system_prompt_for_route(&self, provider: &str, model: &str) -> String {
        let base = self
            .info
            .system_prompt
            .clone()
            .unwrap_or_else(|| Self::system_prompt_for(&self.info.name));
        Self::with_prompt_overlays(&base, &self.system_prompt_overlays, provider, model)
    }

    /// Append provider then model extras. Empty overlay is a no-op.
    pub fn with_prompt_overlays(
        system_prompt: &str,
        overlays: &SystemPromptOverlays,
        provider: &str,
        model: &str,
    ) -> String {
        let sections = overlays.sections(provider, model);
        if sections.is_empty() {
            return system_prompt.to_string();
        }
        let mut out = String::from(system_prompt);
        for (label, body) in sections {
            out.push_str("\n\n# ");
            out.push_str(&label);
            out.push_str("\n\n");
            out.push_str(&body);
        }
        out
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
        // `title_refine_target` already observed this id; treat a later miss as
        // an empty title so the arm shares the existing empty-result path.
        let title = title_from_optional_provider(
            self.provider_registry.get(&use_provider_name),
            &key,
            &use_model,
            &user,
            assistant.as_deref(),
        )
        .await;
        match title {
            Ok(title) => crate::title::apply_refine_result(session, &title, &use_model),
            Err(e) => {
                let error = e.to_string();
                tracing::debug!(error = %error, "session title refine failed");
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
            // `title_refine_target` already observed this id; a later miss is
            // treated as an empty title (same path as a blank model reply).
            let title = title_from_optional_provider(
                registry.get(&use_provider_name),
                &key,
                &use_model,
                &user,
                assistant.as_deref(),
            )
            .await;
            match title {
                Ok(title) if !title.is_empty() => {
                    tracing::debug!(%title, model = %use_model, "session title refined (async)");
                    let _ = title_tx.send((session_id, title));
                }
                Ok(_) => {
                    tracing::debug!("title model returned empty; keeping heuristic/default");
                }
                Err(e) => {
                    let error = e.to_string();
                    tracing::debug!(error = %error, "session title refine failed (async)");
                }
            }
        });
        true
    }
}

fn apply_plugin_count(agent: &mut Agent, mut exec: ToolExecutor, n: usize) {
    agent.apply_lsp_overlay(&mut exec);
    let lsp_custom =
        !agent.lsp_overlay.servers.is_empty() || agent.lsp_overlay.idle_timeout_ms.is_some();
    if n == 0 && !lsp_custom {
        return;
    }
    agent.tool_executor = Arc::new(exec);
    log_registered_count(n, "shell plugins registered");
}

fn lsp_overlay_from_config(cfg: &whycodes_config::LspConfig) -> whycodes_tools::LspSettings {
    let (idle_timeout_ms, servers) = cfg.to_runtime_settings();
    whycodes_tools::LspSettings {
        idle_timeout_ms,
        servers: servers
            .into_iter()
            .map(|(name, s)| {
                (
                    name,
                    whycodes_tools::LspServerSpec {
                        command: s.command,
                        args: s.args,
                        file_types: s.file_types,
                        language_id: s.language_id,
                        root_markers: s.root_markers,
                        init_options: s.init_options,
                        settings: s.settings,
                        disabled: s.disabled,
                        is_linter: s.is_linter,
                    },
                )
            })
            .collect(),
    }
}

fn log_registered_count(count: usize, message: &'static str) {
    if count == 0 {
        return;
    }
    tracing::info!(count, "{message}");
}

async fn title_from_optional_provider(
    provider: Option<&dyn LlmProvider>,
    api_key: &str,
    model: &str,
    user: &str,
    assistant: Option<&str>,
) -> whycodes_core::Result<String> {
    match provider {
        Some(provider) => {
            crate::title::generate_title(provider, api_key, model, user, assistant).await
        }
        None => Ok(String::new()),
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
#[path = "mod_tests.rs"]
mod tests;
