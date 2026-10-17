//! Agent facade: identity, config, prompts, and the conversation loop.
//!
//! Turn execution, tool gates, compaction, and swarm/task dispatch live in
//! sibling modules so this file stays the public surface.

pub mod cache_warm;
mod compact;
mod dispatch;
mod gate;
mod lifecycle;
pub(crate) mod prewalk;
mod spawn;
mod spill;
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
    /// MCP servers connected by [`Self::load_mcp`] (prompts, resources).
    mcp_servers: Vec<crate::mcp_load::McpServerHandle>,
    permission_prompter: Arc<dyn PermissionPrompter>,
    question_prompter: Arc<dyn QuestionPrompter>,
    risk_threshold: RiskThreshold,
    /// Restores secret placeholders inside tool arguments; the transport
    /// redacts outgoing requests with the same redactor.
    secrets: Option<Arc<whycodes_llm::Redactor>>,
    /// A summary of the current prefix prepared before the threshold.
    speculative_summary: Mutex<Option<compact::SpeculativeSummary>>,
    sandbox: SandboxSettings,
    network: NetworkPolicy,
    /// Config-driven tool and lifecycle hooks (empty by default).
    hooks: Vec<HookConfig>,
    /// Last LLM step's prompt-cache read per session id (cache-miss notice).
    cache_reads: Mutex<std::collections::HashMap<String, u64>>,
    /// Session ids whose `session_start` hooks already ran in this process.
    session_starts: Mutex<std::collections::HashSet<String>>,
    /// Discord / Telegram session notifications (off by default).
    notify: NotifyConfig,
    /// When session estimate exceeds this, compact before the next LLM call
    /// `0` disables auto-compact.
    compaction_threshold: usize,
    /// `"auto"` = LLM summary on compact (full-replace); `"off"` = local stub.
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
    /// `[session] fallback_models`: `provider/model` tried when a stream fails to open.
    fallback_models: Vec<String>,
    /// Config `api_key` per provider, for fallback targets on another provider.
    fallback_config_keys: std::collections::HashMap<String, String>,
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
    /// Structured (`--format json`) runs: deny permission `ask` unless allow.
    fail_closed_ask: bool,
    /// `/fresh`: skip provider prompt cache (and local response cache) once.
    skip_prompt_cache_once: std::sync::atomic::AtomicBool,
    /// Cheap model for task/swarm (`provider/model` or bare id).
    model_smol: Option<String>,
    /// Model used while the `plan` agent is active.
    model_plan: Option<String>,
    /// `[session] prewalk`: plan on the strong model, edit on the cheap one.
    prewalk: bool,
    /// The strong route and cheap target of the last prewalk hand-off; later
    /// turns start on it until `/prewalk restart`.
    prewalk_handoff: Mutex<Option<prewalk::PrewalkHandoff>>,
    /// Compiled stream-interrupt rules (name, regex, hint).
    stream_rules: Vec<(String, regex::Regex, String)>,
    /// Parallel multi-agent swarm (config-driven).
    swarm_enabled: bool,
    swarm_max_agents: usize,
    /// Isolate workers in git worktrees when the project is a repo.
    swarm_worktrees: bool,
    /// Background shell jobs (`bash` background=true, `bg`, `schedule`).
    background: crate::background::BackgroundRegistry,
    /// User messages sent while a turn runs (read at the next step).
    steer: crate::steer::SteerInbox,
    /// Optional long-lived event sink (TUI) for bg completion + enqueue.
    event_sink: Option<EventSink>,
    /// Max concurrent background jobs.
    max_background_jobs: usize,
    /// Detach a still-running foreground `bash` after idle (issue #140).
    bash_auto_background: bool,
    bash_auto_background_after: std::time::Duration,
    /// Named credential this session stuck to after the first successful call.
    sticky_credential: Arc<std::sync::Mutex<Option<String>>>,
    /// Per-provider named credential env mapping (secrets stay in env).
    provider_credentials:
        std::collections::HashMap<String, whycodes_core::types::ProviderCredentials>,
    /// Deferred tools activated via `tool_search` for this agent session.
    activated_tools: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Optional tool cwd override (`worktree enter`).
    cwd_override: Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Subagent token usage waiting to be folded into the parent session.
    subagent_usage_pending: Arc<std::sync::Mutex<whycodes_core::types::Usage>>,
    /// USD for that subagent usage, priced with the subagent's own model.
    subagent_cost_pending: Arc<std::sync::Mutex<whycodes_session::SessionCost>>,
    /// `[pricing]` from config; wins over catalog prices.
    price_overrides: std::collections::HashMap<String, whycodes_llm::ModelPrice>,
    /// Resident workspace file index shared with file tools (warm fast path
    /// for glob/grep/list enumeration). Started by the host (TUI/CLI).
    file_index: Option<Arc<whycodes_index::WorkspaceIndex>>,
    /// Process-wide claims for parallel TUI sessions (Ctrl+N).
    session_claims: Option<whycodes_core::FileClaimRegistry>,
    /// Swarm mailbox when this agent is a worker (or parent mid-swarm).
    swarm_hub: Option<whycodes_core::SwarmHub>,
    /// `[lsp]` overlay from config (empty = built-in auto-detect).
    lsp_overlay: whycodes_tools::LspSettings,
    /// `[tools.format] enabled`, re-applied to every rebuilt executor.
    format_after_write: bool,
    /// Provider/model extras from `prompts/*.md`.
    system_prompt_overlays: SystemPromptOverlays,
    /// Active provider id for overlay lookup (`/models`, session route).
    route_provider: String,
    /// Active model id for overlay lookup.
    route_model: String,
}

/// Built-in, plugin and MCP tools plus their connections (MCP servers, LSP
/// clients), shareable between agents loaded from the same config.
#[derive(Clone)]
pub struct SharedTools {
    executor: Arc<ToolExecutor>,
    mcp_servers: Vec<crate::mcp_load::McpServerHandle>,
}

/// Recover from a poisoned mutex instead of aborting (`panic = "abort"` in release).
pub(crate) fn recover_lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stop auto-compact after this many consecutive ineffective passes.
pub(crate) const MAX_CONSECUTIVE_COMPACT_FAILURES: u32 = 3;

/// Identical tool name+args this many times in a row → refuse (doom loop).
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
                } else {
                    let goal = tc
                        .arguments
                        .get("goal")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if !goal.is_empty() {
                        checkpoint_goal = Some(goal.to_string());
                    }
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
                } else {
                    let report = tc
                        .arguments
                        .get("report")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if !report.is_empty() {
                        rewind_report = Some(report.to_string());
                    }
                }
            }
            other => {
                let _other = other;
            }
        }
    }
    (checkpoint_goal, rewind_report)
}

pub(crate) fn persist_agent_artifact(project: &std::path::Path, id: &str, body: &str) {
    write_agent_file(project, id, "md", body);
}

/// A worker's validated structured result, next to its Markdown artifact.
pub(crate) fn persist_agent_json(project: &std::path::Path, id: &str, value: &serde_json::Value) {
    write_agent_file(project, id, "json", &format!("{value:#}"));
}

fn write_agent_file(project: &std::path::Path, id: &str, ext: &str, body: &str) {
    let mut id_clean = String::new();
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            id_clean.push(c);
        }
    }
    let id = id_clean;
    if id.is_empty() {
        return;
    }
    let dir = whycodes_core::project_dir(project).join("agents");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, "agent artifact dir");
        return;
    }
    let path = dir.join(format!("{id}.{ext}"));
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

fn append_custom_agents_catalog(system_prompt: &str, project_path: &std::path::Path) -> String {
    let catalog = crate::custom_agents::catalog_markdown(&crate::custom_agents::load(project_path));
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

/// This turn's memory recall, on the latest user message at every step.
///
/// Every step of the turn sends the same suffix, so the steps share the
/// provider's cached prefix; the system prompt never carries it.
pub(crate) fn append_turn_context(
    request: &mut whycodes_core::types::LlmRequest,
    context: Option<&str>,
) {
    match context {
        Some(text) => append_request_user_suffix(request, text),
        None => skip_turn_context(),
    }
}

fn skip_turn_context() {}

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
            mcp_servers: Vec::new(),
            permission_prompter: default_prompter(),
            question_prompter: default_question_prompter(),
            risk_threshold: RiskThreshold::default(),
            secrets: whycodes_llm::secrets::installed(),
            speculative_summary: Mutex::new(None),
            sandbox: SandboxSettings::default(),
            network: NetworkPolicy::unrestricted(),
            hooks: Vec::new(),
            cache_reads: Mutex::new(std::collections::HashMap::new()),
            session_starts: Mutex::new(std::collections::HashSet::new()),
            notify: NotifyConfig::default(),
            // Match config default when `with_config` is not used.
            compaction_threshold: 150_000,
            compaction_llm: true,
            tool_profile: ToolProfile::Core,
            use_prompt_cache: true,
            model_fast: None,
            model_race: "off".into(),
            race_after: std::time::Duration::from_millis(800),
            fallback_models: Vec::new(),
            fallback_config_keys: std::collections::HashMap::new(),
            response_cache: true,
            memory: crate::subagent::default_memory(),
            intent_guidance: crate::intent::IntentGuidanceMode::default(),
            magic_keywords: whycodes_config::MagicKeywordsConfig::default(),
            reasoning_effort: None,
            approval_mode: ApprovalMode::Auto,
            fail_closed_ask: false,
            skip_prompt_cache_once: std::sync::atomic::AtomicBool::new(false),
            model_smol: None,
            model_plan: None,
            prewalk: false,
            prewalk_handoff: Mutex::new(None),
            stream_rules: Vec::new(),
            swarm_enabled: true,
            swarm_max_agents: 4,
            swarm_worktrees: true,
            background: crate::background::BackgroundRegistry::default(),
            steer: crate::steer::SteerInbox::default(),
            event_sink: None,
            max_background_jobs: crate::background::DEFAULT_MAX_BACKGROUND_JOBS,
            bash_auto_background: true,
            bash_auto_background_after: std::time::Duration::from_secs(20),
            sticky_credential: Arc::new(std::sync::Mutex::new(None)),
            provider_credentials: std::collections::HashMap::new(),
            activated_tools: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            cwd_override: Arc::new(std::sync::Mutex::new(None)),
            subagent_usage_pending: Arc::new(std::sync::Mutex::new(
                whycodes_core::types::Usage::default(),
            )),
            subagent_cost_pending: Arc::new(std::sync::Mutex::new(
                whycodes_session::SessionCost::default(),
            )),
            price_overrides: std::collections::HashMap::new(),
            file_index: None,
            session_claims: None,
            swarm_hub: None,
            lsp_overlay: whycodes_tools::LspSettings::default(),
            format_after_write: true,
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

    /// Whether `name` is in the current registry (built-in, config, or test).
    pub fn has_provider(&self, name: &str) -> bool {
        self.provider_registry.get(name).is_some()
    }

    /// Open `provider`'s connection in the background (see
    /// [`whycodes_llm::preconnect`]). `false` when nothing was started.
    pub fn preconnect(&self, provider: &str) -> bool {
        self.provider_registry
            .get(provider)
            .is_some_and(|p| whycodes_llm::preconnect::spawn(p.default_base_url()))
    }

    pub(crate) fn provider_registry_get(&self, name: &str) -> Option<&dyn LlmProvider> {
        self.provider_registry.get(name)
    }

    pub(crate) fn tool_executor_defs_readonly(&self) -> Vec<whycodes_core::types::ToolDefinition> {
        self.tool_executor
            .get_definitions(&self.info.permission)
            .iter()
            .cloned()
            .collect()
    }

    pub(crate) async fn execute_readonly_tool(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
    ) -> ToolResult {
        self.tool_executor
            .execute(call, ctx, &self.info.permission)
            .await
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

    /// Structured / CI runs: permission `ask` is denied instead of auto-allowed.
    pub fn set_fail_closed_ask(&mut self, fail_closed: bool) {
        self.fail_closed_ask = fail_closed;
    }

    pub fn with_fail_closed_ask(mut self, fail_closed: bool) -> Self {
        self.fail_closed_ask = fail_closed;
        self
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
        self.system_prompt_overlays = overlays
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

    /// Use `redactor` instead of the process default for this agent and
    /// its workers.
    pub fn with_redactor(mut self, redactor: Option<Arc<whycodes_llm::Redactor>>) -> Self {
        self.secrets = redactor;
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
        self.price_overrides = price_overrides_from_config(&config.pricing);
        self.race_after = std::time::Duration::from_millis(config.session.race_after_ms);
        self.fallback_models = config.session.fallback_models.clone();
        self.fallback_config_keys = config
            .providers
            .iter()
            .filter_map(|(name, pc)| Some((name.clone(), pc.api_key.clone()?)))
            .filter(|(_, key)| !key.is_empty())
            .collect();
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
        self.prewalk = config.session.prewalk.unwrap_or(false);
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
        self.bash_auto_background = config.tools.bash.auto_background;
        self.bash_auto_background_after =
            std::time::Duration::from_secs(config.tools.bash.auto_background_after_secs.max(1));
        self.provider_credentials = config
            .providers
            .iter()
            .map(|(k, v)| (k.clone(), v.credentials.clone()))
            .collect();
        self.lsp_overlay = lsp_overlay_from_config(&config.lsp);
        self.format_after_write = config.tools.format.enabled;
        self.tool_executor
            .set_format_after_write(self.format_after_write);
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
        let bash_auto_background = self.bash_auto_background;
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
            bash_auto_background,
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
                if let Err(e) = tx.send(TurnEvent::Background {
                    id: ev.id,
                    status: ev.status.as_str().to_string(),
                    summary: ev.summary,
                }) {
                    let error = e.to_string();
                    tracing::debug!(error = %error, "background event dropped (listener closed)");
                }
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

    pub fn sticky_credential(&self) -> Option<String> {
        recover_lock(&self.sticky_credential).clone()
    }

    pub fn set_sticky_credential(&self, name: Option<String>) {
        *recover_lock(&self.sticky_credential) = name;
    }

    pub(crate) fn failover_api_key(
        &self,
        provider: &str,
        current: &str,
    ) -> Option<(String, String)> {
        let creds = self.provider_credentials.get(provider)?;
        let names = creds.names_for_lane(whycodes_core::types::process_is_ci());
        let mut secrets: Vec<(String, String)> = Vec::new();
        for name in names {
            let var = creds.env_var_for(&name, provider);
            let secret = match std::env::var(&var) {
                Ok(secret) => secret,
                Err(err) => {
                    let _missing = err;
                    String::new()
                }
            };
            if !secret.is_empty() {
                secrets.push((name, secret));
            }
        }
        let found = secrets.iter().position(|(_, secret)| secret == current);
        #[allow(clippy::question_mark)]
        let idx = match found {
            Some(idx) => idx,
            None => return None,
        };
        secrets.into_iter().nth(idx + 1)
    }

    /// History-free side turn (`/btw`): read-only tools, 2k-token cap, no session write.
    pub async fn run_side_turn(
        &self,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        question: &str,
    ) -> whycodes_core::Result<String> {
        crate::side_turn::run(self, session, provider_name, model, api_key, question).await
    }

    pub(crate) fn inject_auto_background_completions(&self, session: &mut Session) {
        let done = self.background.take_auto_completions();
        if done.is_empty() {
            return;
        }
        for (id, status, output) in done {
            let tail: String = output
                .chars()
                .rev()
                .take(4_000)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            session.add_user_message(&format!(
                "Background job `{id}` finished ({status}). Output:\n{tail}",
                status = status.as_str()
            ));
        }
    }

    /// Inbox for messages sent while a turn runs; keep a clone before the
    /// agent moves into the turn task.
    pub fn steer_inbox(&self) -> crate::steer::SteerInbox {
        self.steer.clone()
    }

    /// Add messages the user sent during this turn, so the next request
    /// carries them. Called at the start of a step, after tool results.
    pub(crate) fn inject_steering(&self, session: &mut Session, events: &Option<EventSink>) {
        for content in self.steer.drain(&session.id) {
            crate::steer::add_to_session(session, content);
            crate::events::emit(
                events,
                TurnEvent::Status("message delivered to the running turn".into()),
            );
        }
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

    /// USD for one usage record on `provider/model`; `None` when unpriced.
    pub(crate) fn step_cost(
        &self,
        provider: &str,
        model: &str,
        usage: &whycodes_core::types::Usage,
    ) -> Option<f64> {
        let price = self
            .price_overrides
            .get(&format!("{provider}/{model}"))
            .copied()
            .or_else(|| whycodes_llm::lookup_price(provider, model))?;
        Some(price.cost(usage))
    }

    /// First `[session] fallback_models` entry at or after `from` that names a
    /// registered provider, differs from the active target, and has a key.
    ///
    /// Key order: the active key (same provider), config `api_key`, then
    /// `<PROVIDER>_API_KEY`. OAuth-only providers on another provider are
    /// skipped — the agent does not reach the token store.
    pub(crate) fn next_fallback(
        &self,
        from: usize,
        active_provider: &str,
        active_model: &str,
        active_key: &str,
    ) -> Option<(usize, FallbackTarget)> {
        for (idx, raw) in self.fallback_models.iter().enumerate().skip(from) {
            let Some((provider, model)) = raw.trim().split_once('/') else {
                tracing::warn!(entry = %raw, "fallback_models entry is not provider/model");
                continue;
            };
            if (provider == active_provider && model == active_model)
                || self.provider_registry.get(provider).is_none()
            {
                continue;
            }
            let Some(api_key) = self.target_key(provider, active_provider, active_key) else {
                continue;
            };
            return Some((
                idx,
                FallbackTarget {
                    provider: provider.to_string(),
                    model: model.to_string(),
                    api_key,
                },
            ));
        }
        None
    }

    /// Key for a switch to `provider`: the active key (same provider), config
    /// `api_key`, then a non-empty `<PROVIDER>_API_KEY`.
    pub(crate) fn target_key(
        &self,
        provider: &str,
        active_provider: &str,
        active_key: &str,
    ) -> Option<String> {
        if provider == active_provider {
            return Some(active_key.to_string());
        }
        if let Some(key) = self.fallback_config_keys.get(provider) {
            return Some(key.clone());
        }
        let var = format!("{}_API_KEY", provider.to_uppercase());
        match std::env::var(&var) {
            Ok(key) if !key.is_empty() => Some(key),
            Ok(_) => None,
            Err(missing) => {
                tracing::debug!(%var, "no key for the target: {missing}");
                None
            }
        }
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
            let (n, servers) = crate::mcp_load::register_mcp_tools(&mut full, config).await;
            self.mcp_servers = servers;
            n
        };
        if n_plug > 0 || n_mcp > 0 || lsp_overlay_is_custom(&self.lsp_overlay) {
            self.tool_executor = Arc::new(full);
            log_registered_count(n_plug, "shell plugins registered");
            log_registered_count(n_mcp, "MCP tools registered");
        }
    }

    /// MCP servers connected by [`Self::with_mcp`] / [`Self::load_mcp`].
    pub fn mcp_servers(&self) -> &[crate::mcp_load::McpServerHandle] {
        &self.mcp_servers
    }

    /// This agent's loaded tools, for another agent with the same config.
    pub fn shared_tools(&self) -> SharedTools {
        SharedTools {
            executor: Arc::clone(&self.tool_executor),
            mcp_servers: self.mcp_servers.clone(),
        }
    }

    /// Reuse another agent's tools instead of loading them again: one set of
    /// MCP server processes and LSP clients serves both.
    pub fn with_shared_tools(mut self, tools: SharedTools) -> Self {
        self.tool_executor = tools.executor;
        self.mcp_servers = tools.mcp_servers;
        self
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
        exec.set_format_after_write(self.format_after_write);
        if !lsp_overlay_is_custom(&self.lsp_overlay) {
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
        Self::with_runtime_context_in(system_prompt, None)
    }

    /// [`Self::with_runtime_context`] that also names the working directory.
    /// The platform and the `bash` tool's shell are always stated: on Windows
    /// that shell is Git Bash or `cmd.exe`, and the model cannot tell which.
    pub fn with_runtime_context_in(
        system_prompt: &str,
        working_dir: Option<&std::path::Path>,
    ) -> String {
        if system_prompt.contains("Today's date:") {
            return system_prompt.to_string();
        }
        let today = chrono::Local::now().format("%Y-%m-%d");
        let os = std::env::consts::OS;
        let shell = whycodes_sandbox::host_shell_name();
        let cwd = working_dir
            .map(|dir| format!("Working directory: {}\n", dir.display()))
            .unwrap_or_default();
        format!(
            "{system_prompt}\n\n# Environment\n\n\
             Today's date: {today}.\n\
             Platform: {os}. The `bash` tool runs commands in {shell}.\n\
             {cwd}\
             When searching for the current or latest version of software, do not pin the query to a past year; \
             prefer canonical sources (npm registry, GitHub Releases, official docs)."
        )
    }

    /// Append project instruction files (AGENTS.md and sibling conventions)
    /// plus runtime context to a system prompt.
    pub fn with_agents_md(system_prompt: &str, project_path: &std::path::Path) -> String {
        Self::with_agents_md_in(system_prompt, project_path, project_path)
    }

    /// [`Self::with_agents_md`] for an agent that works somewhere other than
    /// the project root, e.g. a swarm worker in its own git worktree.
    pub fn with_agents_md_in(
        system_prompt: &str,
        project_path: &std::path::Path,
        working_dir: &std::path::Path,
    ) -> String {
        let with_files =
            crate::context_files::append_project_instructions(system_prompt, project_path);
        let with_skills = append_skills_catalog(&with_files, project_path);
        let with_agents = append_custom_agents_catalog(&with_skills, project_path);
        Self::with_runtime_context_in(&with_agents, Some(working_dir))
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
                    if let Err(e) = title_tx.send((session_id, title)) {
                        let error = e.to_string();
                        tracing::debug!(error = %error, "session title dropped (listener closed)");
                    }
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
    if n == 0 && !lsp_overlay_is_custom(&agent.lsp_overlay) {
        return;
    }
    agent.tool_executor = Arc::new(exec);
    log_registered_count(n, "shell plugins registered");
}

/// `[pricing."provider/model"]` → prices keyed the same way.
fn price_overrides_from_config(
    pricing: &std::collections::HashMap<String, whycodes_config::PriceConfig>,
) -> std::collections::HashMap<String, whycodes_llm::ModelPrice> {
    pricing
        .iter()
        .filter(|(key, _)| {
            let valid = key.split_once('/').is_some();
            if !valid {
                tracing::warn!(key = %key, "[pricing] key is not provider/model");
            }
            valid
        })
        .map(|(key, p)| {
            let price = whycodes_llm::ModelPrice {
                input: p.input,
                output: p.output,
                cache_read: p.cache_read,
                cache_write: p.cache_write,
            };
            (key.clone(), price)
        })
        .collect()
}

/// A model to switch to after the active one failed to open a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FallbackTarget {
    pub provider: String,
    pub model: String,
    pub api_key: String,
}

impl Agent {
    /// Record this step's prompt-cache read; a notice when it dropped from
    /// the previous step's. `/fresh` (`skip_cache`) misses on purpose.
    pub(crate) fn note_cache_read(
        &self,
        session_id: &str,
        usage: &whycodes_core::types::Usage,
        skip_cache: bool,
    ) -> Option<String> {
        let read = usage.cache_read_input_tokens.unwrap_or(0);
        let prev = recover_lock(&self.cache_reads).insert(session_id.to_string(), read)?;
        if skip_cache {
            return None;
        }
        crate::cache_watch::cache_miss_notice(prev, usage)
    }
}

/// Any `[lsp]` key set: the executor must carry a configured `lsp` tool.
fn lsp_overlay_is_custom(overlay: &whycodes_tools::LspSettings) -> bool {
    !overlay.servers.is_empty()
        || overlay.idle_timeout_ms.is_some()
        || overlay.edit_diagnostics_ms.is_some()
}

fn lsp_overlay_from_config(cfg: &whycodes_config::LspConfig) -> whycodes_tools::LspSettings {
    let (idle_timeout_ms, servers) = cfg.to_runtime_settings();
    whycodes_tools::LspSettings {
        idle_timeout_ms,
        edit_diagnostics_ms: cfg.edit_diagnostics_ms,
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
