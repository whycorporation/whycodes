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

use std::collections::VecDeque;
use std::time::Instant;
use whycode_session::session::Session;

use super::events::{CancelFlag, EventSink, TurnEvent, emit, is_cancelled, wait_until_cancelled};
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
                        let pretty = serde_json::to_string_pretty(other)
                            .unwrap_or_else(|_| other.to_string());
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

/// Worktree names: short, no path separators / traversal.
fn is_safe_worktree_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Path argument for file mutators / readers (permission path globs).
fn file_tool_path(tc: &ToolCall) -> Option<String> {
    let key = match tc.name.as_str() {
        "read" | "write" | "edit" | "glob" | "list" => "path",
        "apply_patch" => "path", // may also use multi-file; path optional
        "grep" => "path",
        _ => return None,
    };
    tc.arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().replace('\\', "/"))
        .filter(|s| !s.is_empty())
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
    "bg",
    "schedule",
    "tool_search",
    "worktree",
    "plan",
    "browser",
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
    memory: whycode_memory::MemorySettings,
    /// Heuristic intent posture for build turns (`auto` / `off` / `always`).
    intent_guidance: crate::intent::IntentGuidanceMode,
    /// Hidden per-turn notices for standalone prose keywords.
    magic_keywords: whycode_config::MagicKeywordsConfig,
    /// Session `reasoning_effort` (`low`/`medium`/`high`/`xhigh`). Empty = family default.
    reasoning_effort: Option<String>,
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
    subagent_usage_pending: Arc<std::sync::Mutex<whycode_core::types::Usage>>,
    /// Resident workspace file index shared with file tools (warm fast path
    /// for glob/grep/list enumeration). Started by the host (TUI/CLI).
    file_index: Option<Arc<whycode_index::WorkspaceIndex>>,
    /// Process-wide claims for parallel TUI sessions (Ctrl+N).
    session_claims: Option<whycode_core::FileClaimRegistry>,
    /// Swarm mailbox when this agent is a worker (or parent mid-swarm).
    swarm_hub: Option<whycode_core::SwarmHub>,
}

/// Stop auto-compact after this many consecutive ineffective passes (Claude Code).
const MAX_CONSECUTIVE_COMPACT_FAILURES: u32 = 3;

/// Identical tool name+args this many times in a row → refuse (OpenCode doom_loop).
const DOOM_LOOP_THRESHOLD: usize = 3;

/// Validate checkpoint/rewind tool results and return pending side effects.
///
/// Side effects run after `add_tool_results` so the checkpoint boundary includes
/// the successful checkpoint tool result.
fn settle_checkpoint_rewind(
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

fn persist_agent_artifact(project: &std::path::Path, id: &str, body: &str) {
    let id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if id.is_empty() {
        return;
    }
    let dir = project.join(".whycode").join("agents");
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
    rules: &[whycode_config::StreamRuleConfig],
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

fn first_stream_rule_hit<'a>(
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
    let Ok(reg) = whycode_skill::SkillRegistry::load_project(project_path) else {
        return system_prompt.to_string();
    };
    let catalog = reg.catalog_markdown();
    if catalog.is_empty() {
        return system_prompt.to_string();
    }
    format!("{system_prompt}\n\n{catalog}")
}

fn append_request_user_suffix(request: &mut whycode_core::types::LlmRequest, suffix: &str) {
    use whycode_core::types::{MessageContent, Role};
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

/// Structured compact prompt (Grok full-replace), with optional `/compact` note.
fn build_compact_summary_prompt(transcript: &str, user_context: Option<&str>) -> String {
    let user_context_section = match user_context.map(str::trim).filter(|s| !s.is_empty()) {
        Some(context) => format!(
            "\n\n**User-provided context for this compaction:**\n{context}\n\n\
             Please incorporate this context into your summary, ensuring it is \
             prominently addressed in the relevant sections.\n"
        ),
        None => String::new(),
    };
    format!(
        "Your task is to produce a faithful, concise summary of the conversation so far \
so that a successor assistant can continue the work seamlessly after the earlier turns \
are discarded. The successor will see the user's original query plus this summary. \
Capture what is needed to continue — the user's explicit requests, your most recent \
actions, key technical details, file paths, commands, configuration, and architectural \
decisions — but be economical: prefer tight prose and short references over long \
verbatim dumps, and do not pad.
{user_context_section}
CRITICAL: If earlier turns include a prior compaction summary (marked with a \
\"This session is being continued\" preamble or \"[Compacted\" stub), treat it as \
authoritative for the early history and carry its still-relevant information forward.

Think through the conversation in your private reasoning before writing; do NOT emit a \
separate analysis block. Output the final summary inside a single <summary>...</summary> \
block, organized into the following numbered sections. Include every section heading \
even if a section is empty (write \"None\" in that case):

1. Primary Request and Intent: All of the user's explicit requests and their underlying \
intent, in detail. Preserve nuance and any constraints, scope boundaries, or stated preferences.
2. Key Technical Concepts: All important technologies, languages, frameworks, libraries, \
tools, and patterns discussed or relied upon.
3. Files and Code Sections: Every file examined, created, or modified. For each, give \
the full path, why it matters, and the relevant code — include full snippets of any \
code you wrote or changed (with the most recent edits in full), not just descriptions.
4. Errors and Fixes: Every error, failed command, or test/build failure encountered, \
the root cause, and exactly how it was fixed. Note any fix that came from user feedback verbatim.
5. Problem Solving: Problems already solved and any in-progress diagnosis or troubleshooting.
6. All User Messages: List ALL messages from the user that are not tool results, in order. \
Do NOT include this summarization instruction itself.
7. Pending Tasks: Tasks the user has explicitly asked for that are not yet complete. \
Do not invent tasks the user never requested.
8. Current Work: Precisely what you were doing immediately before this summary request.
9. Optional Next Step: The single next step that directly continues the most recent work.

IMPORTANT: Do NOT call or use any tools. Respond with ONLY the <summary>...</summary> \
block as your text output.

Conversation:
{transcript}"
    )
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
            compaction_llm: true,
            tool_profile: ToolProfile::Core,
            use_prompt_cache: true,
            model_fast: None,
            model_race: "off".into(),
            race_after: std::time::Duration::from_millis(800),
            response_cache: true,
            memory: whycode_memory::MemorySettings::default(),
            intent_guidance: crate::intent::IntentGuidanceMode::default(),
            magic_keywords: whycode_config::MagicKeywordsConfig::default(),
            reasoning_effort: None,
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
                whycode_core::types::Usage::default(),
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
    pub fn with_file_index(mut self, index: Arc<whycode_index::WorkspaceIndex>) -> Self {
        self.file_index = Some(index);
        self
    }

    /// Share a process-wide claim registry (parallel TUI sessions).
    pub fn with_session_claims(mut self, claims: whycode_core::FileClaimRegistry) -> Self {
        self.session_claims = Some(claims);
        self
    }

    pub fn session_claims(&self) -> Option<whycode_core::FileClaimRegistry> {
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
            sandbox = %whycode_sandbox::describe_backend(&self.sandbox),
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
    fn panel_sink(&self) -> Option<whycode_core::PanelSink> {
        let tx = self.event_sink.clone()?;
        Some(std::sync::Arc::new(move |update| {
            if let Err(e) = tx.send(TurnEvent::Panel(update)) {
                tracing::debug!(error = %e, "panel event dropped (listener closed)");
            }
        }))
    }

    /// Forward `todowrite` updates onto the turn event channel.
    fn todo_sink(&self) -> Option<whycode_core::TodoSink> {
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

    /// Resolve an optional first-token race partner (`off` / `auto` / ref).
    fn race_partner(&self, provider_name: &str, model: &str) -> Option<(String, String)> {
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
    fn tool_context(&self, session: &Session) -> ToolContext {
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

    /// Grok-style full-replace compact: summarize the whole conversation,
    /// keep the last real user query + current-turn tail, replace the rest
    /// with a continuation carrier.
    ///
    /// Manual `/compact [context]` always runs this (no token threshold).
    /// Auto-compact uses the same path when over `compaction_threshold`.
    /// Falls back to a local stub when LLM is off, the key is missing, or
    /// sampling fails.
    pub async fn compact_session(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        user_context: Option<&str>,
    ) -> whycode_session::CompactOutcome {
        session.truncate_large_tool_results();
        session.prune_old_tool_results();
        if session.messages.is_empty() {
            return whycode_session::CompactOutcome::default();
        }

        let transcript = session.transcript_for_full_summary(0);
        let local = session.local_full_replace_summary();
        let want_llm = self.compaction_llm && !api_key.is_empty() && !transcript.trim().is_empty();
        let summary = if want_llm {
            self.llm_compact_summary(&transcript, provider_name, model, api_key, user_context)
                .await
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(local)
        } else {
            local
        };
        session.apply_full_replace(&summary)
    }

    /// Structured summarizer used by full-replace compact (session model).
    async fn llm_compact_summary(
        &self,
        transcript: &str,
        provider_name: &str,
        model: &str,
        api_key: &str,
        user_context: Option<&str>,
    ) -> Option<String> {
        if transcript.trim().is_empty() {
            return None;
        }
        let provider = self.provider_registry.get(provider_name)?;
        use whycode_core::types::{LlmRequest, Message, MessageContent, Role};
        let request = LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text(build_compact_summary_prompt(
                    transcript,
                    user_context,
                )),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: vec![],
            max_tokens: Some(4_096),
            temperature: Some(0.2),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        };
        let transport = whycode_llm::LlmTransport {
            complete_timeout: Some(std::time::Duration::from_secs(60)),
            retry: whycode_llm::RetryPolicy {
                max_retries: 2,
                initial_backoff: std::time::Duration::from_millis(200),
                max_backoff: std::time::Duration::from_secs(3),
                max_elapsed: std::time::Duration::from_secs(90),
                full_jitter: true,
            },
        };
        match transport.complete(provider, &request, api_key, model).await {
            Ok(resp) => {
                let text = resp
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string();
                if text.is_empty() { None } else { Some(text) }
            }
            Err(e) => {
                tracing::warn!(error = %e, "LLM compact summary failed");
                None
            }
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
    pub async fn with_mcp(mut self, config: &whycode_config::Config) -> Self {
        self.load_mcp(config).await;
        self
    }

    /// Like [`Self::with_mcp`] but for an already-owned agent (TUI deferred load).
    pub async fn load_mcp(&mut self, config: &whycode_config::Config) {
        let project = config.general.project_path.as_deref();
        let mut full = ToolExecutor::new();
        let n_plug = full.register_config_plugins(project);
        let n_mcp = if config.mcp_servers.is_empty() {
            0
        } else {
            super::mcp_load::register_mcp_tools(&mut full, config).await
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

    /// Run a single conversation turn (no streaming UI events).
    ///
    /// `max_turns` is a headless safety cap (`None` = unlimited, Grok TUI
    /// parity). Interactive sessions pass `None` and stop on end-of-turn,
    /// cancel, or doom-loop instead.
    pub async fn run_turn(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: Option<usize>,
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
        max_turns: Option<usize>,
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
        let magic = crate::magic_keywords::scan(&last_user, &self.magic_keywords);
        let skip_cache = self
            .skip_prompt_cache_once
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        let (role_provider, role_model) = crate::routing::resolve_agent_model(
            provider_name,
            model,
            &self.info.name,
            self.model_plan.as_deref(),
        );
        let provider_name = role_provider.as_str();
        let model = role_model.as_str();
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

        let provider = self
            .provider_registry
            .get(provider_name)
            .ok_or_else(|| {
                whycode_core::Error::Llm(format!(
                    "Unknown provider: {}. Available: anthropic, openai, google, google-antigravity, and configured custom providers",
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
        // Autocompact circuit breaker: stop retrying after N ineffective passes.
        let mut compact_failures: u32 = 0;
        let mut compact_paused = false;
        let mut overflow_retries: u32 = 0;

        loop {
            // Rebuild each step so tool_search activations and worktree cwd apply.
            let tools = if tools_free_chat {
                Vec::new()
            } else {
                let extra = self.activated_tools_snapshot();
                let mut defs = self.tool_executor.get_definitions_profile_extra(
                    &self.info.permission,
                    self.tool_profile,
                    &extra,
                );
                if !self.swarm_enabled {
                    defs.retain(|d| d.name != "swarm");
                }
                defs
            };
            let tool_ctx = self.tool_context(session);
            if is_cancelled(&cancel) {
                emit(&events, TurnEvent::Cancelled);
                return Err(whycode_core::Error::Agent("Cancelled".into()));
            }

            turn_count += 1;
            if let Some(max) = max_turns
                && turn_count > max
            {
                return Err(whycode_core::Error::Agent(format!(
                    "Exceeded maximum turns ({max})"
                )));
            }

            // Always shrink oversized / old tool dumps before prefill (cheap).
            // When still hot, shake older tool bodies harder so overflow is less likely.
            // Full-replace compact when over the configured token threshold —
            // and only while the circuit breaker has not tripped.
            let _ = session.truncate_large_tool_results();
            let _ = session.prune_old_tool_results();
            if self.compaction_threshold > 0
                && session.token_count() > self.compaction_threshold.saturating_mul(3) / 4
            {
                let shaken = session.shake_old_tool_results();
                if shaken > 0 {
                    tracing::debug!(shaken, "shook old tool results before LLM step");
                }
            }
            if self.compaction_threshold > 0 && !compact_paused {
                let before = session.token_count();
                if before > self.compaction_threshold {
                    let outcome = self
                        .compact_session(session, provider_name, model, api_key, None)
                        .await;
                    if outcome.reduced() || outcome.dropped_messages() {
                        emit(
                            &events,
                            TurnEvent::Status(format!(
                                "Compacted context ({} → {} msgs, ~{} → ~{} tok)…",
                                outcome.messages_before,
                                outcome.messages_after,
                                outcome.tokens_before,
                                outcome.tokens_after
                            )),
                        );
                        tracing::info!(
                            before_tokens = outcome.tokens_before,
                            after_tokens = outcome.tokens_after,
                            messages_before = outcome.messages_before,
                            messages_after = outcome.messages_after,
                            "auto-compact before LLM step"
                        );
                    }
                    if outcome.still_over(self.compaction_threshold) {
                        compact_failures = compact_failures.saturating_add(1);
                        if compact_failures >= MAX_CONSECUTIVE_COMPACT_FAILURES {
                            compact_paused = true;
                            emit(
                                &events,
                                TurnEvent::Status(format!(
                                    "Auto-compact paused after {MAX_CONSECUTIVE_COMPACT_FAILURES} \
                                     passes (~{} tok still over threshold)",
                                    outcome.tokens_after
                                )),
                            );
                            tracing::warn!(
                                failures = compact_failures,
                                tokens = outcome.tokens_after,
                                "autocompact circuit breaker tripped"
                            );
                        }
                    } else {
                        compact_failures = 0;
                    }
                }
            }

            emit(
                &events,
                TurnEvent::Status(format!("LLM request (step {turn_count})…")),
            );

            let mut request =
                session.build_request(&tools, None, self.info.temperature, Some(true));
            request.use_prompt_cache = self.use_prompt_cache && !skip_cache;
            crate::thinking_acc::attach_thinking_request(
                &mut request,
                provider_name,
                model,
                self.info.model.as_ref(),
                self.reasoning_effort.as_deref(),
            );
            if magic.ultrathink {
                crate::thinking_acc::apply_ultrathink(&mut request);
            }

            // First LLM step: ephemeral intent posture (not stored in session;
            // keeps system prompt cache-stable). Notice is already on Intent event.
            if turn_count == 1
                && crate::intent::should_inject(self.intent_guidance, &turn_intent)
                && let Some(suffix) = crate::intent::posture_suffix(&turn_intent, &self.info.name)
            {
                append_request_user_suffix(&mut request, &suffix);
                tracing::debug!(
                    intent = turn_intent.intent.as_str(),
                    confidence = turn_intent.confidence,
                    agent = %self.info.name,
                    "intent posture injected into request"
                );
            }
            if turn_count == 1 && magic.any() {
                let notice = magic.notice();
                append_request_user_suffix(&mut request, &notice);
                tracing::debug!(
                    ultrathink = magic.ultrathink,
                    orchestrate = magic.orchestrate,
                    "magic keyword notice injected into request"
                );
            }

            let mut accumulated_text = String::new();
            let mut thinking_acc = crate::thinking_acc::ThinkingAccumulator::new();
            let mut turn_usage = whycode_core::types::Usage::default();
            let mut assembler = ToolCallAssembler::new();
            let mut speculative_reads: Vec<crate::speculative_read::SpeculativeRead> = Vec::new();
            let step_t0 = Instant::now();

            // Professional transport: classify + full-jitter backoff + Retry-After.
            // Only the HTTP open is retried — mid-stream drops stay single-shot.
            // Race the open against cancel so a hung gateway cannot ignore Esc.
            // Bind transport so `stream()`'s future is not tied to a temporary.
            let transport = whycode_llm::default_transport();
            let race_ids = self.race_partner(provider_name, model);
            let race_provider = race_ids
                .as_ref()
                .and_then(|(p, _)| self.provider_registry.get(p.as_str()));
            let race_target = match (race_ids.as_ref(), race_provider) {
                (Some((_, m)), Some(rp)) => Some(whycode_llm::StreamTarget {
                    provider: rp,
                    api_key,
                    model: m.as_str(),
                }),
                _ => None,
            };
            let opened = tokio::select! {
                biased;
                _ = wait_until_cancelled(&cancel) => {
                    emit(&events, TurnEvent::Cancelled);
                    return Err(whycode_core::Error::Agent("Cancelled".into()));
                }
                opened = transport.stream_turn(
                    whycode_llm::StreamTarget {
                        provider,
                        api_key,
                        model,
                    },
                    &request,
                    whycode_llm::StreamTurnOpts {
                        cache: self.response_cache && request.tools.is_empty() && !skip_cache,
                        race: race_target,
                        race_after: self.race_after,
                    },
                ) => opened,
            };
            let turn = match opened {
                Ok(t) => t,
                Err(e)
                    if whycode_llm::classify(&e).kind
                        == whycode_llm::ErrorKind::ContextOverflow
                        && overflow_retries < 1 =>
                {
                    overflow_retries = overflow_retries.saturating_add(1);
                    emit(
                        &events,
                        TurnEvent::Status(
                            "Context overflow — compacting and retrying this step…".into(),
                        ),
                    );
                    let outcome = self
                        .compact_session(session, provider_name, model, api_key, None)
                        .await;
                    tracing::info!(
                        after_tokens = outcome.tokens_after,
                        "compacted after context overflow"
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            let cache_hit = turn.cache_hit;
            let race_tag = turn.race.as_str();
            if cache_hit {
                emit(&events, TurnEvent::Status("Response cache hit".into()));
            } else if turn.race.raced() {
                let partner = race_ids.as_ref().map(|(_, m)| m.as_str()).unwrap_or("?");
                emit(
                    &events,
                    TurnEvent::Status(format!("First-token race: {partner} ({race_tag})")),
                );
            }
            let mut event_stream = turn.events;
            let mut stream_rule_retry = false;

            // Stream body: check cancel between tokens *and* while idle waiting
            // for the next SSE line (select! with wait_until_cancelled).
            loop {
                let event = tokio::select! {
                    biased;
                    _ = wait_until_cancelled(&cancel) => {
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        let mut blocks = thinking_acc.into_blocks();
                        if !accumulated_text.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: accumulated_text.clone(),
                            });
                            final_text.push_str(&accumulated_text);
                        }
                        if !blocks.is_empty() {
                            session.add_assistant_message(blocks);
                        }
                        emit(&events, TurnEvent::Cancelled);
                        return Err(whycode_core::Error::Agent("Cancelled".into()));
                    }
                    next = event_stream.next() => next,
                };

                let Some(event) = event else {
                    break;
                };

                let event = match event {
                    Ok(ev) => ev,
                    Err(e)
                        if whycode_llm::classify(&e).kind
                            == whycode_llm::ErrorKind::ContextOverflow
                            && overflow_retries < 1 =>
                    {
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        overflow_retries = overflow_retries.saturating_add(1);
                        emit(
                            &events,
                            TurnEvent::Status(
                                "Context overflow — compacting and retrying this step…".into(),
                            ),
                        );
                        let outcome = self
                            .compact_session(session, provider_name, model, api_key, None)
                            .await;
                        tracing::info!(
                            after_tokens = outcome.tokens_after,
                            "compacted after streamed context overflow"
                        );
                        stream_rule_retry = true;
                        break;
                    }
                    Err(e) => {
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        whycode_core::logging::emit_sid(
                            "agent",
                            "error",
                            "turn.stream_error",
                            Some(session.id.as_str()),
                            Some(serde_json::json!({
                                "provider": provider_name,
                                "model": model,
                                "error": e.to_string(),
                            })),
                        );
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::TextDelta { text } => {
                        thinking_acc.flush();
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        emit(&events, TurnEvent::TextDelta(text.clone()));
                        accumulated_text.push_str(&text);
                        if let Some((name, hint)) =
                            first_stream_rule_hit(&self.stream_rules, &accumulated_text)
                        {
                            crate::speculative_read::abort_all(&mut speculative_reads);
                            emit(
                                &events,
                                TurnEvent::Status(format!(
                                    "Stream rule `{name}` interrupted the draft"
                                )),
                            );
                            session.add_user_message(&format!(
                                "<whycode_rule name=\"{name}\">\n{hint}\n\
                                 The previous draft was discarded. Continue without violating this rule.\n\
                                 </whycode_rule>"
                            ));
                            stream_rule_retry = true;
                            break;
                        }
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        thinking_acc.flush();
                        // Defer ToolStart until after argument fragments are
                        // merged — OpenAI streams send null/empty args first.
                        assembler.on_tool_use(id, name, input);
                        // Complete objects (Anthropic non-streamed) can start I/O now.
                        if let Some((cid, cname, buf)) = assembler.last_updated() {
                            crate::speculative_read::maybe_start(
                                &mut speculative_reads,
                                &cid,
                                &cname,
                                &buf,
                                &tool_ctx,
                            );
                        }
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        assembler.on_tool_use_delta(&id, &input_json_delta);
                        // Path often closes mid-stream — start `read` I/O early.
                        if let Some((cid, cname, buf)) = assembler.last_updated() {
                            crate::speculative_read::maybe_start(
                                &mut speculative_reads,
                                &cid,
                                &cname,
                                &buf,
                                &tool_ctx,
                            );
                        }
                    }
                    StreamEvent::Thinking { text } => {
                        if text.is_empty() {
                            continue;
                        }
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        thinking_acc.push_text(&text);
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::trace!(n = text.len(), "thinking delta");
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        if text.is_empty() {
                            continue;
                        }
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        thinking_acc.push_text(&text);
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::trace!(n = text.len(), "thinking delta");
                    }
                    StreamEvent::ThinkingSignature { signature } => {
                        thinking_acc.push_signature(&signature);
                    }
                    StreamEvent::RedactedThinking { data } => {
                        thinking_acc.push_redacted(&data);
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        // Snapshot fold (max), not sum: Anthropic splits
                        // input/output across events; OpenAI-compat gateways
                        // often repeat the full usage object.
                        turn_usage.absorb_stream(input_tokens, output_tokens);
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        turn_usage.absorb_stream_cache(creation_input_tokens, read_input_tokens);
                    }
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        if whycode_llm::classify_message(&message).kind
                            == whycode_llm::ErrorKind::ContextOverflow
                            && overflow_retries < 1
                        {
                            crate::speculative_read::abort_all(&mut speculative_reads);
                            overflow_retries = overflow_retries.saturating_add(1);
                            emit(
                                &events,
                                TurnEvent::Status(
                                    "Context overflow — compacting and retrying this step…".into(),
                                ),
                            );
                            let outcome = self
                                .compact_session(session, provider_name, model, api_key, None)
                                .await;
                            tracing::info!(
                                after_tokens = outcome.tokens_after,
                                "compacted after streamed context overflow"
                            );
                            stream_rule_retry = true;
                            break;
                        }
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        return Err(whycode_core::Error::Llm(message));
                    }
                }
            }

            if stream_rule_retry {
                continue;
            }

            // Merge streamed argument fragments into parsed JSON objects.
            let tool_calls = assembler.finish();
            let step_ms = step_t0.elapsed().as_millis();

            if self.response_cache
                && !cache_hit
                && request.tools.is_empty()
                && tool_calls.is_empty()
                && !accumulated_text.trim().is_empty()
            {
                whycode_llm::ResponseCache::global().store(&request, model, &accumulated_text);
            }

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

            let mut blocks: Vec<ContentBlock> = thinking_acc.into_blocks();

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
                crate::speculative_read::abort_all(&mut speculative_reads);
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
                        "response_cache_hit": cache_hit,
                        "race": race_tag,
                        "done": true,
                    })),
                );
                break;
            }

            // Doom-loop: refuse identical tool+args repeated DOOM_LOOP_THRESHOLD times
            // (OpenCode processor.ts doom_loop permission pattern).
            let results = if would_doom_loop(&recent_tool_sigs, &tool_calls) {
                crate::speculative_read::abort_all(&mut speculative_reads);
                emit(
                    &events,
                    TurnEvent::Status("Doom loop: identical tool call repeated — refusing".into()),
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
                        &mut speculative_reads,
                    )
                    .await?;
                crate::speculative_read::abort_all(&mut speculative_reads);
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
                        "response_cache_hit": cache_hit,
                        "race": race_tag,
                        "done": false,
                    })),
                );
                results
            };

            let mut results = results;
            let (checkpoint_goal, rewind_report) =
                settle_checkpoint_rewind(session, &tool_calls, &mut results);

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
            if let Some(goal) = checkpoint_goal {
                session.mark_checkpoint(goal);
            }
            if let Some(report) = rewind_report {
                if session.apply_rewind(&report) {
                    tracing::debug!("collapsed exploratory context after rewind");
                } else {
                    tracing::debug!("rewind requested with no active checkpoint");
                }
            }

            // Fold subagent tokens into this turn + parent session (plan-performance).
            if let Ok(mut pending) = self.subagent_usage_pending.lock()
                && !pending.is_empty()
            {
                let fold = std::mem::take(&mut *pending);
                turn_usage.add(&fold);
                session.add_usage(&fold);
                tracing::debug!(
                    input = fold.input_tokens,
                    output = fold.output_tokens,
                    "folded subagent usage into parent session"
                );
            }

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
                "response_cache": self.response_cache,
                "model_race": self.model_race,
            })),
        );

        // Hindsight-style auto-retain (heuristic + optional LLM). Best-effort
        // and **async** — never await here. LLM extract can take 5–12s and used
        // to keep the TUI on `generating` after the answer was already on screen
        // (same pitfall as title refine; see docs/knowhow.md).
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
    ///
    /// `speculative` holds early `read` jobs started while args were still
    /// streaming; matching calls skip a second disk pass.
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
        speculative: &mut Vec<crate::speculative_read::SpeculativeRead>,
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
            let result =
                if let Some(early) = self.take_speculative_read(tc, tool_ctx, speculative).await {
                    early
                } else {
                    tokio::select! {
                        biased;
                        _ = wait_until_cancelled(cancel) => {
                            emit(events, TurnEvent::Cancelled);
                            return Err(whycode_core::Error::Agent("Cancelled".into()));
                        }
                        r = self.execute_with_permission(
                            tc,
                            session,
                            tool_ctx,
                            provider_name,
                            model,
                            api_key,
                            turn_intent,
                            events.as_ref(),
                        ) => r,
                    }
                };
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
            // Consume matching speculative reads first (I/O already overlapped
            // the LLM stream). Remaining calls run in parallel as before.
            let mut early: Vec<Option<ToolResult>> = Vec::with_capacity(tool_calls.len());
            for tc in tool_calls {
                early.push(self.take_speculative_read(tc, tool_ctx, speculative).await);
            }
            // ToolStart already emitted by the caller for every call.
            let futs: Vec<_> = tool_calls
                .iter()
                .zip(early)
                .map(|(tc, pre)| {
                    let this = self;
                    async move {
                        if let Some(r) = pre {
                            return r;
                        }
                        this.execute_with_permission(
                            tc,
                            session,
                            tool_ctx,
                            provider_name,
                            model,
                            api_key,
                            turn_intent,
                            events.as_ref(),
                        )
                        .await
                    }
                })
                .collect();
            let results = tokio::select! {
                biased;
                _ = wait_until_cancelled(cancel) => {
                    emit(events, TurnEvent::Cancelled);
                    return Err(whycode_core::Error::Agent("Cancelled".into()));
                }
                r = futures::future::join_all(futs) => r,
            };
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
            let result =
                if let Some(early) = self.take_speculative_read(tc, tool_ctx, speculative).await {
                    early
                } else {
                    tokio::select! {
                        biased;
                        _ = wait_until_cancelled(cancel) => {
                            emit(events, TurnEvent::Cancelled);
                            return Err(whycode_core::Error::Agent("Cancelled".into()));
                        }
                        r = self.execute_with_permission(
                            tc,
                            session,
                            tool_ctx,
                            provider_name,
                            model,
                            api_key,
                            turn_intent,
                            events.as_ref(),
                        ) => r,
                    }
                };
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

    /// Use a speculative early `read` if path/window still match the final call.
    async fn take_speculative_read(
        &self,
        tc: &ToolCall,
        tool_ctx: &ToolContext,
        speculative: &mut Vec<crate::speculative_read::SpeculativeRead>,
    ) -> Option<ToolResult> {
        if tc.name != "read" || speculative.is_empty() {
            return None;
        }
        let path = tc.arguments.get("path")?.as_str()?;
        let (offset, limit) = crate::speculative_read::window_from_args(&tc.arguments);
        let result = crate::speculative_read::take_matching(
            speculative,
            &tc.id,
            path,
            offset,
            limit,
            &tool_ctx.working_dir,
        )
        .await?;
        tracing::debug!(id = %tc.id, path, "speculative early read hit");
        Some(result)
    }

    /// Apply the shell risk gate, then allow/ask/deny, then execute (or spawn
    /// a task subagent).
    ///
    /// `pub(crate)` so the risk gate can be tested at this level: the unit
    /// tests in `command-risk` cover classification, but only this method
    /// proves that a catastrophic command is refused even when the permission
    /// map says `allow`.
    #[allow(clippy::too_many_arguments)]
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
            return run_question_tool(self.question_prompter.as_ref(), &tc.arguments, &tc.id).await;
        }

        // Shell commands (and `schedule` with a delayed shell payload) are gated
        // on what the command would destroy. The permission map below only sees
        // the tool name, so on its own `allow` would run anything the model emits.
        // Shell-scoped rules (`bash(git *)`) can skip or force prompts for Safe cmds.
        let mut risk_confirmed = false;
        let scheduled_shell = (tc.name == "schedule")
            .then(|| {
                tc.arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .filter(|c| !c.trim().is_empty())
            })
            .flatten();
        if SHELL_TOOLS.contains(&tc.name.as_str()) || scheduled_shell.is_some() {
            let command = scheduled_shell.unwrap_or_else(|| {
                tc.arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
            });
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

            // Shell-scoped permission rules (Claude Code `Bash(git *)` spirit).
            if let Some(shell_act) = self.info.permission.action_for_shell(command) {
                match shell_act {
                    PermissionAction::Deny => {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!(
                                "Permission denied for shell command by rule matching `{command}`."
                            ),
                            is_error: true,
                        };
                    }
                    PermissionAction::Allow => {
                        // Safe path only: skip further tool-level Ask when risk allowed.
                        // Destructive Confirm already handled above.
                        if matches!(decide(&assessment, self.risk_threshold), Decision::Allow) {
                            risk_confirmed = true;
                        }
                    }
                    PermissionAction::Ask if !risk_confirmed => {
                        let detail =
                            format!("Shell rule requires confirmation\n\nCommand:\n{command}");
                        if !self.permission_prompter.ask(&tc.name, &detail).await {
                            return ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: format!("User denied permission for tool '{}'.", tc.name),
                                is_error: true,
                            };
                        }
                        risk_confirmed = true;
                    }
                    PermissionAction::Ask => {}
                }
            }
        }

        // Intent authorization (Claude-style): question/plan/ambiguous-always
        // turns must not silently mutate. After blast-radius, before permission.
        if let Some(intent) = turn_intent {
            let command = tc.arguments.get("command").and_then(|v| v.as_str());
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

        // Path-scoped rules: `edit(src/**)`, `write(docs/**)`, …
        if let Some(path) = file_tool_path(tc)
            && let Some(path_act) = self.info.permission.action_for_path(&tc.name, &path)
        {
            match path_act {
                PermissionAction::Deny => {
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Permission denied for `{}` on path `{path}` by path rule.",
                            tc.name
                        ),
                        is_error: true,
                    };
                }
                PermissionAction::Allow => {
                    risk_confirmed = true;
                }
                PermissionAction::Ask if !risk_confirmed => {
                    let detail = format!(
                        "Path rule requires confirmation\n\nTool: {}\nPath: {path}",
                        tc.name
                    );
                    if !self.permission_prompter.ask(&tc.name, &detail).await {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!("User denied permission for tool '{}'.", tc.name),
                            is_error: true,
                        };
                    }
                    risk_confirmed = true;
                }
                PermissionAction::Ask => {}
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
            self.execute_task_tool(tc, session, provider_name, model, api_key, events)
                .await
        } else if tc.name == "swarm" {
            self.execute_swarm_tool(tc, session, provider_name, model, api_key, events)
                .await
        } else if tc.name == "bg" {
            self.execute_bg_tool(tc)
        } else if tc.name == "schedule" {
            self.execute_schedule_tool(tc, tool_ctx, events).await
        } else if tc.name == "tool_search" {
            self.execute_tool_search(tc)
        } else if tc.name == "worktree" {
            self.execute_worktree_tool(tc, session)
        } else if (tc.name == "bash" || tc.name == "shell")
            && tc
                .arguments
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            self.execute_background_shell(tc, tool_ctx, events)
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

    /// `bash`/`shell` with `background: true` — return job id immediately.
    fn execute_background_shell(
        &self,
        call: &ToolCall,
        tool_ctx: &ToolContext,
        events: Option<&EventSink>,
    ) -> ToolResult {
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if command.trim().is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "background shell requires a non-empty `command`".into(),
                is_error: true,
            };
        }
        let label = call
            .arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // Prefer long-lived sink; fall back to turn events for listener (optional).
        if self.event_sink.is_none()
            && let Some(tx) = events
        {
            let tx = tx.clone();
            self.background
                .set_listener(Some(std::sync::Arc::new(move |ev| {
                    let _ = tx.send(TurnEvent::Background {
                        id: ev.id,
                        status: ev.status.as_str().to_string(),
                        summary: ev.summary,
                    });
                })));
        }
        match self.background.start_shell(
            &command,
            std::path::PathBuf::from(&tool_ctx.working_dir),
            tool_ctx.sandbox.clone(),
            label,
        ) {
            Ok(id) => {
                emit(
                    &events.cloned().or_else(|| self.event_sink.clone()),
                    TurnEvent::Background {
                        id: id.clone(),
                        status: "running".into(),
                        summary: truncate_permission_detail(&command),
                    },
                );
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!(
                        "Background job `{id}` started.\n\
                         Command: {command}\n\
                         Use tool `bg` with action=list|read|kill (id={id})."
                    ),
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: call.id.clone(),
                content: e,
                is_error: true,
            },
        }
    }

    fn execute_bg_tool(&self, call: &ToolCall) -> ToolResult {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("list");
        match action {
            "list" => {
                let jobs = self.background.list();
                if jobs.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "No background jobs.".into(),
                        is_error: false,
                    };
                }
                let mut lines = vec![format!(
                    "Background jobs ({} running):",
                    self.background.running_count()
                )];
                for j in jobs {
                    lines.push(format!(
                        "- {} [{}] {:.1}s · {}{}",
                        j.id,
                        j.status.as_str(),
                        j.elapsed.as_secs_f64(),
                        j.label,
                        j.exit_code
                            .map(|c| format!(" exit={c}"))
                            .unwrap_or_default()
                    ));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "read" => {
                let id = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if id.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "bg read requires `id`".into(),
                        is_error: true,
                    };
                }
                let max = call
                    .arguments
                    .get("max_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(8_000) as usize;
                match self.background.read(id, max) {
                    Ok(s) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: s,
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            "kill" => {
                let id = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if id.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "bg kill requires `id`".into(),
                        is_error: true,
                    };
                }
                match self.background.kill(id) {
                    Ok(s) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: s,
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            other => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!("unknown bg action `{other}` (list|read|kill)"),
                is_error: true,
            },
        }
    }

    fn execute_tool_search(&self, call: &ToolCall) -> ToolResult {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("search");
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let max = call
            .arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .clamp(1, 40) as usize;

        let catalog = self.tool_executor.deferred_catalog(&self.info.permission);
        let activated = self.activated_tools_snapshot();

        match action {
            "list" => {
                let mut lines = vec![format!(
                    "Activated ({}): {}",
                    activated.len(),
                    if activated.is_empty() {
                        "(none)".into()
                    } else {
                        activated.join(", ")
                    }
                )];
                lines.push(format!("Deferred catalogue ({}):", catalog.len()));
                for (name, desc) in catalog.iter().take(40) {
                    let active = if activated.iter().any(|a| a == name) {
                        " [on]"
                    } else {
                        ""
                    };
                    let short: String = desc.chars().take(72).collect();
                    lines.push(format!("- {name}{active} — {short}"));
                }
                if catalog.len() > 40 {
                    lines.push(format!("…and {} more", catalog.len() - 40));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "select" => {
                if query.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "tool_search select requires `query` (tool name or comma list)"
                            .into(),
                        is_error: true,
                    };
                }
                let names: Vec<String> = query
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                let mut added = Vec::new();
                let mut missing = Vec::new();
                let mut guard = self
                    .activated_tools
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                for name in names {
                    if self.tool_executor.get(&name).is_some() {
                        if !ToolProfile::Core.includes(&name) {
                            guard.insert(name.clone());
                        }
                        added.push(name);
                    } else {
                        missing.push(name);
                    }
                }
                drop(guard);
                let mut content = format!(
                    "Activated for this session: {}\nThey appear on the next LLM step.",
                    if added.is_empty() {
                        "(none)".into()
                    } else {
                        added.join(", ")
                    }
                );
                if !missing.is_empty() {
                    content.push_str(&format!("\nUnknown: {}", missing.join(", ")));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content,
                    is_error: !missing.is_empty() && added.is_empty(),
                }
            }
            _ => {
                if query.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "tool_search search requires `query` keywords".into(),
                        is_error: true,
                    };
                }
                let q = query.to_ascii_lowercase();
                let terms: Vec<&str> = q.split_whitespace().collect();
                let mut scored: Vec<(i32, &str, &str)> = catalog
                    .iter()
                    .filter_map(|(name, desc)| {
                        let hay = format!("{name} {desc}").to_ascii_lowercase();
                        let mut score = 0i32;
                        for t in &terms {
                            if name.to_ascii_lowercase() == *t {
                                score += 10;
                            } else if name.to_ascii_lowercase().contains(t) {
                                score += 5;
                            } else if hay.contains(t) {
                                score += 2;
                            }
                        }
                        if score > 0 {
                            Some((score, name.as_str(), desc.as_str()))
                        } else {
                            None
                        }
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
                scored.truncate(max);
                if scored.is_empty() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "No deferred tools match `{query}`. Try action=list for the catalogue."
                        ),
                        is_error: false,
                    };
                }
                let mut lines = vec![format!(
                    "Matches for `{query}` (select with action=select query=<name>):"
                )];
                for (_, name, desc) in scored {
                    let short: String = desc.chars().take(80).collect();
                    lines.push(format!("- {name} — {short}"));
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
        }
    }

    fn execute_worktree_tool(&self, call: &ToolCall, session: &Session) -> ToolResult {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = call
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let root = crate::swarm_worktree::git_toplevel(&session.project_path)
            .unwrap_or_else(|| session.project_path.clone());
        let base = root.join(".whycode").join("worktrees");

        match action {
            "list" => {
                let mut lines = vec![format!("Worktrees under {}", base.display())];
                if let Some(cwd) = self.cwd_override_path() {
                    lines.push(format!("Active cwd override: {}", cwd.display()));
                }
                match std::fs::read_dir(&base) {
                    Ok(rd) => {
                        let mut names: Vec<_> = rd
                            .flatten()
                            .filter(|e| e.path().is_dir())
                            .map(|e| e.file_name().to_string_lossy().to_string())
                            .collect();
                        names.sort();
                        if names.is_empty() {
                            lines.push("(none)".into());
                        } else {
                            for n in names {
                                lines.push(format!("- {n}"));
                            }
                        }
                    }
                    Err(_) => lines.push("(directory missing — create one first)".into()),
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "create" => {
                if name.is_empty() || !is_safe_worktree_name(name) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "worktree create needs a safe `name` (alnum, -, _)".into(),
                        is_error: true,
                    };
                }
                if !crate::swarm_worktree::is_git_repo(&root) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "not a git repository".into(),
                        is_error: true,
                    };
                }
                let dest = base.join(name);
                match crate::swarm_worktree::create_worktree(&root, &dest, name) {
                    Ok(wt) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "Created worktree `{name}` at {}\nbase HEAD {}\nUse action=enter name={name} to switch tool cwd.",
                            wt.path.display(),
                            wt.base_head
                        ),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            "remove" => {
                if name.is_empty() || !is_safe_worktree_name(name) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "worktree remove needs a safe `name`".into(),
                        is_error: true,
                    };
                }
                let dest = base.join(name);
                if let Ok(mut g) = self.cwd_override.lock()
                    && g.as_ref().is_some_and(|p| p.starts_with(&dest))
                {
                    *g = None;
                }
                let wt = crate::swarm_worktree::SwarmWorktree {
                    path: dest,
                    repo_root: root,
                    base_head: String::new(),
                    worker_id: name.to_string(),
                };
                match crate::swarm_worktree::remove_worktree(&wt) {
                    Ok(()) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!("Removed worktree `{name}`"),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        content: e,
                        is_error: true,
                    },
                }
            }
            "enter" => {
                if name.is_empty() || !is_safe_worktree_name(name) {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: "worktree enter needs a safe `name`".into(),
                        is_error: true,
                    };
                }
                let dest = base.join(name);
                if !dest.is_dir() {
                    return ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!(
                            "worktree `{name}` not found — create it first ({})",
                            dest.display()
                        ),
                        is_error: true,
                    };
                }
                if let Ok(mut g) = self.cwd_override.lock() {
                    *g = Some(dest.clone());
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!(
                        "Tool cwd → {}\nUse action=exit to restore project root.",
                        dest.display()
                    ),
                    is_error: false,
                }
            }
            "exit" => {
                let prev = self.cwd_override.lock().ok().and_then(|mut g| g.take());
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: match prev {
                        Some(p) => {
                            format!("Restored tool cwd to project root (was {})", p.display())
                        }
                        None => "No worktree cwd override was active.".into(),
                    },
                    is_error: false,
                }
            }
            other => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!(
                    "unknown worktree action `{other}` (create|list|remove|enter|exit)"
                ),
                is_error: true,
            },
        }
    }

    /// Delay then either start a background shell or enqueue a user prompt.
    async fn execute_schedule_tool(
        &self,
        call: &ToolCall,
        tool_ctx: &ToolContext,
        events: Option<&EventSink>,
    ) -> ToolResult {
        let after_secs = call
            .arguments
            .get("after_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(86_400);
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let goal = call
            .arguments
            .get("goal")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if command.is_none() && goal.is_none() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "schedule requires `command` and/or `goal`".into(),
                is_error: true,
            };
        }

        let background = self.background.clone();
        let sandbox = tool_ctx.sandbox.clone();
        let cwd = std::path::PathBuf::from(&tool_ctx.working_dir);
        let sink = events.cloned().or_else(|| self.event_sink.clone());
        let label = call
            .arguments
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        tokio::spawn(async move {
            if after_secs > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(after_secs)).await;
            }
            if let Some(cmd) = command {
                match background.start_shell(&cmd, cwd, sandbox, label) {
                    Ok(id) => {
                        if let Some(ref tx) = sink {
                            let _ = tx.send(TurnEvent::Background {
                                id: id.clone(),
                                status: "running".into(),
                                summary: format!("scheduled: {cmd}"),
                            });
                        }
                    }
                    Err(e) => {
                        if let Some(ref tx) = sink {
                            let _ = tx.send(TurnEvent::Background {
                                id: "schedule".into(),
                                status: "failed".into(),
                                summary: e,
                            });
                        }
                    }
                }
            }
            if let Some(g) = goal
                && let Some(ref tx) = sink
            {
                let _ = tx.send(TurnEvent::EnqueuePrompt { text: g });
            }
        });

        let mut parts = vec![format!("Scheduled in {after_secs}s")];
        if let Some(ref c) = call.arguments.get("command").and_then(|v| v.as_str()) {
            parts.push(format!("shell: {c}"));
        }
        if let Some(ref g) = call.arguments.get("goal").and_then(|v| v.as_str()) {
            parts.push(format!("prompt queue: {g}"));
        }
        ToolResult {
            tool_call_id: call.id.clone(),
            content: parts.join("\n"),
            is_error: false,
        }
    }

    /// Execute the `swarm` tool: parallel subagents + file-claim / worktree isolation.
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
        use tokio::sync::Semaphore;
        use whycode_core::types::{AgentInfo, AgentMode, PermissionSet, ToolResult};
        use whycode_core::{ClaimResult, FileClaimRegistry};

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

        // Worktrees when configured and project is a git repo; else same-checkout + claims.
        let repo_root = crate::swarm_worktree::git_toplevel(&session.project_path)
            .unwrap_or_else(|| session.project_path.clone());
        let use_worktrees =
            self.swarm_worktrees && crate::swarm_worktree::is_git_repo(&session.project_path);
        let run_id = format!(
            "{}-{}",
            session.id.chars().take(8).collect::<String>(),
            chrono::Utc::now().format("%H%M%S")
        );
        let swarm_run_dir = crate::swarm_worktree::run_dir(&repo_root, &run_id);

        let claims = FileClaimRegistry::new();
        let hub = whycode_core::SwarmHub::new();
        hub.ensure("parent");
        if let Some(tx) = events {
            let tx_c = tx.clone();
            claims.set_listener(Some(std::sync::Arc::new(move |ev| {
                if let Err(e) = tx_c.send(TurnEvent::FileConflict {
                    path: ev.path,
                    claimant: ev.claimant_label,
                    owner: ev.owner_label,
                }) {
                    tracing::debug!(error = %e, "swarm conflict event dropped");
                }
            })));
            let tx_s = tx.clone();
            claims.set_stale_listener(Some(std::sync::Arc::new(move |ev| {
                if let Err(e) = tx_s.send(TurnEvent::FileStale {
                    path: ev.path,
                    reader: ev.reader_id,
                    writer: ev.writer_label,
                }) {
                    tracing::debug!(error = %e, "swarm stale event dropped");
                }
            })));
            let tx_m = tx.clone();
            hub.set_listener(Some(std::sync::Arc::new(move |msg| {
                if let Err(e) = tx_m.send(TurnEvent::SwarmMessage {
                    from: msg.from,
                    to: msg.to,
                    text: msg.text,
                }) {
                    tracing::debug!(error = %e, "swarm message event dropped");
                }
            })));
        }

        let wall_t0 = Instant::now();
        let total = specs.len();
        let mode_label = if use_worktrees {
            "worktrees"
        } else if self.swarm_worktrees {
            "same-checkout (not a git repo)"
        } else {
            "same-checkout (worktrees off)"
        };
        emit(
            &events.cloned(),
            TurnEvent::SwarmStatus {
                active: 0,
                total,
                message: format!(
                    "Starting swarm: {total} workers, {mode_label}, max {max_concurrent} concurrent…"
                ),
            },
        );

        // Pre-claim optional paths (logical ownership for merge / same-checkout).
        for (i, spec) in specs.iter().enumerate() {
            let worker_id = format!("worker-{i}");
            let label = format!("{worker_id}/{}", spec.subagent_type);
            for rel in &spec.paths {
                let full = if std::path::Path::new(rel).is_absolute() {
                    std::path::PathBuf::from(rel)
                } else {
                    // Claim against main checkout paths so ownership is shared across worktrees.
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
        let (worker_provider, worker_model) =
            crate::routing::resolve_worker_model(provider_name, model, self.model_smol.as_deref());
        let provider_name: std::sync::Arc<str> = worker_provider.into();
        let model: std::sync::Arc<str> = worker_model.into();
        let api_key: std::sync::Arc<str> = api_key.into();
        let project_path = session.project_path.clone();
        let registry = Arc::clone(&self.provider_registry);
        let executor = Arc::clone(&self.tool_executor);
        let sandbox = self.sandbox.clone();
        let network = self.network.clone();
        let memory = self.memory.clone();
        let parent_permission = self.info.permission.clone();
        let agents_md_path = session.project_path.clone();
        let repo_root_arc = repo_root.clone();
        let swarm_run_dir = swarm_run_dir.clone();
        let file_index = self.file_index.clone();
        let panel = self.panel_sink();
        let hub = hub.clone();

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
            let repo_root = repo_root_arc.clone();
            let swarm_run_dir = swarm_run_dir.clone();
            let file_index = file_index.clone();
            let panel = panel.clone();
            let hub = hub.clone();
            hub.ensure(&worker_id);

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
                            whycode_core::types::Usage::default(),
                            None,
                            project_path,
                            events_tx,
                            label,
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

                // Optional isolated checkout.
                let mut worktree = None;
                let worker_cwd = if use_worktrees {
                    let dest = swarm_run_dir.join(&worker_id);
                    match crate::swarm_worktree::create_worktree(&repo_root, &dest, &worker_id) {
                        Ok(wt) => {
                            let path = wt.path.clone();
                            worktree = Some(wt);
                            path
                        }
                        Err(e) => {
                            return (
                                worker_id,
                                spec.subagent_type,
                                spec.goal,
                                false,
                                0.0,
                                format!("Failed to create git worktree: {e}"),
                                whycode_core::types::Usage::default(),
                                None,
                                project_path,
                                events_tx,
                                label,
                            );
                        }
                    }
                } else {
                    project_path.clone()
                };

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
                                "swarm_msg".into(),
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
                        for t in ["todowrite", "todo", "todoread", "task", "swarm"] {
                            if !denied.iter().any(|x| x == t) {
                                denied.push(t.to_string());
                            }
                        }
                        perm.denied_tools = Some(denied);
                        (perm, Agent::system_prompt_for("general"))
                    }
                };

                let isolation_note = if worktree.is_some() {
                    "\n\nYou are running in an isolated git worktree. Edit freely; \
                     changes merge back into the main checkout when you finish. \
                     Prefer staying within your assigned paths."
                        .to_string()
                } else {
                    "\n\nYou share the main checkout with sibling workers. \
                     File claims block double-writes. Use `swarm_msg` to tell \
                     siblings what you changed. A `read` of a file another \
                     worker wrote will be marked stale."
                        .to_string()
                };
                let claim_note = if spec.paths.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nYou own these paths exclusively for this swarm run: {}.\
                         \nDo not edit other workers' files.",
                        spec.paths.join(", ")
                    )
                };
                let context = {
                    let extra = format!("{isolation_note}{claim_note}");
                    match spec.context {
                        Some(c) if extra.is_empty() => Some(c),
                        Some(c) => Some(format!("{c}{extra}")),
                        None if extra.is_empty() => None,
                        None => Some(extra.trim().to_string()),
                    }
                };

                let info = AgentInfo {
                    name: spec.subagent_type.clone(),
                    description: format!("Swarm worker {worker_id}"),
                    mode: AgentMode::Subagent,
                    permission,
                    model: None,
                    system_prompt: Some(Agent::with_agents_md(&system_prompt, &agents_md_path)),
                    temperature: None,
                    top_p: None,
                };

                let task = SubagentTask {
                    goal: spec.goal.clone(),
                    context,
                    tools: None,
                    max_turns: spec.max_turns,
                };

                // File claims apply in same-checkout mode; with worktrees,
                // physical isolation holds during the run and merge does 3-way.
                let mut runner =
                    SubagentRunner::new(registry, executor, info, worker_cwd, sandbox, network)
                        .with_memory(memory)
                        .with_file_index(file_index.clone())
                        .with_panel(panel.clone())
                        .with_swarm_hub(Some(hub.clone()));
                if !use_worktrees {
                    runner =
                        runner.with_file_claims(claims.clone(), worker_id.clone(), label.clone());
                }

                let t0 = Instant::now();
                if let Some(ref tx) = events_tx
                    && let Err(e) = tx.send(TurnEvent::Subagent {
                        id: worker_id.clone(),
                        kind: spec.subagent_type.clone(),
                        description: spec.goal.clone(),
                        status: "running".into(),
                        activity: "Thinking".into(),
                        elapsed_ms: 0,
                        output: String::new(),
                    })
                {
                    tracing::debug!(error = %e, "subagent running event dropped");
                }
                let result = runner.run(task, &pn, &m, &ak).await;
                let secs = t0.elapsed().as_secs_f64();
                claims.release_agent(&worker_id);

                let (success, body, worker_usage) = match result {
                    Ok(r) => (r.success, r.output, r.usage),
                    Err(e) => (
                        false,
                        format!("Swarm worker error: {e}"),
                        whycode_core::types::Usage::default(),
                    ),
                };
                if success {
                    persist_agent_artifact(&project_path, &worker_id, &body);
                }
                if let Some(ref tx) = events_tx
                    && let Err(e) = tx.send(TurnEvent::Subagent {
                        id: worker_id.clone(),
                        kind: spec.subagent_type.clone(),
                        description: spec.goal.clone(),
                        status: if success { "completed" } else { "failed" }.into(),
                        activity: String::new(),
                        elapsed_ms: (secs * 1000.0) as u64,
                        output: body.clone(),
                    })
                {
                    tracing::debug!(error = %e, "subagent finished event dropped");
                }

                (
                    worker_id,
                    spec.subagent_type,
                    spec.goal,
                    success,
                    secs,
                    body,
                    worker_usage,
                    worktree,
                    project_path,
                    events_tx,
                    label,
                )
            }));
        }

        let mut sections = Vec::with_capacity(handles.len());
        let mut ok = 0usize;
        let mut merge_conflicts = 0usize;
        for handle in handles {
            match handle.await {
                Ok((
                    worker_id,
                    kind,
                    goal,
                    mut success,
                    secs,
                    mut body,
                    worker_usage,
                    worktree,
                    project_path,
                    events_tx,
                    label,
                )) => {
                    if !worker_usage.is_empty()
                        && let Ok(mut pending) = self.subagent_usage_pending.lock()
                    {
                        pending.add(&worker_usage);
                    }
                    if let Some(wt) = worktree {
                        let merge = crate::swarm_worktree::merge_into_main(&wt, &project_path);
                        for c in &merge.conflicts {
                            if let Some(ref tx) = events_tx {
                                let _ = tx.send(TurnEvent::FileConflict {
                                    path: c.path.clone(),
                                    claimant: label.clone(),
                                    owner: "main".into(),
                                });
                            }
                        }
                        if !merge.conflicts.is_empty() {
                            success = false;
                        }
                        let merge_txt = crate::swarm_worktree::format_merge_report(&merge);
                        if !merge_txt.is_empty() {
                            body = format!("{body}\n\n{merge_txt}");
                        }
                        if let Err(e) = crate::swarm_worktree::remove_worktree(&wt) {
                            body = format!("{body}\n\n_Worktree cleanup warning: {e}_");
                        }
                    }

                    if success {
                        ok += 1;
                    }
                    if body.contains("**Merge conflicts:**") {
                        merge_conflicts += 1;
                    }
                    sections.push(crate::swarm::format_worker_report(
                        &worker_id, &kind, success, secs, &goal, &body,
                    ));
                }
                Err(e) => {
                    sections.push(format!("### worker join error\n\n{e}\n"));
                }
            }
        }

        claims.clear();
        // Best-effort prune empty swarm run dir.
        let _ = std::fs::remove_dir_all(&swarm_run_dir);

        let wall = wall_t0.elapsed().as_secs_f64();
        emit(
            &events.cloned(),
            TurnEvent::SwarmStatus {
                active: 0,
                total,
                message: format!(
                    "Swarm done: {ok}/{total} ok in {wall:.1}s ({mode_label}{})",
                    if merge_conflicts > 0 {
                        format!(", {merge_conflicts} merge conflict(s)")
                    } else {
                        String::new()
                    }
                ),
            },
        );

        let mut report = crate::swarm::format_swarm_header(total, ok, wall);
        report.push_str(&format!("\n_isolation: {mode_label}_\n\n"));
        report.push_str(&sections.join("\n"));

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
        events: Option<&EventSink>,
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
        .with_memory(self.memory.clone())
        .with_file_index(self.file_index.clone())
        .with_panel(self.panel_sink());

        let child_id = format!("task-{}", call.id);
        let started = std::time::Instant::now();
        emit(
            &events.cloned(),
            TurnEvent::Subagent {
                id: child_id.clone(),
                kind: subagent_type.to_string(),
                description: goal.clone(),
                status: "running".into(),
                activity: "Thinking".into(),
                elapsed_ms: 0,
                output: String::new(),
            },
        );

        let (worker_provider, worker_model) =
            crate::routing::resolve_worker_model(provider_name, model, self.model_smol.as_deref());
        match runner
            .run(task, &worker_provider, &worker_model, api_key)
            .await
        {
            Ok(result) => {
                if !result.usage.is_empty()
                    && let Ok(mut pending) = self.subagent_usage_pending.lock()
                {
                    pending.add(&result.usage);
                }
                let usage_note = if result.usage.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\n[subagent usage: {} in / {} out]",
                        result.usage.input_tokens, result.usage.output_tokens
                    )
                };
                let status = if result.success {
                    "completed"
                } else {
                    "failed"
                };
                emit(
                    &events.cloned(),
                    TurnEvent::Subagent {
                        id: child_id.clone(),
                        kind: subagent_type.to_string(),
                        description: goal.clone(),
                        status: status.into(),
                        activity: String::new(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        output: result.output.clone(),
                    },
                );
                if result.success {
                    persist_agent_artifact(&session.project_path, &child_id, &result.output);
                }
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: if result.success {
                        format!(
                            "Subagent ({}) completed in {:.1}s. Re-read with `read agent://{child_id}`.\n\n{}{usage_note}",
                            subagent_type,
                            result.duration.as_secs_f64(),
                            result.output
                        )
                    } else {
                        format!(
                            "Subagent ({}) finished with errors:\n\n{}{usage_note}",
                            subagent_type, result.output
                        )
                    },
                    is_error: !result.success,
                }
            }
            Err(e) => {
                emit(
                    &events.cloned(),
                    TurnEvent::Subagent {
                        id: child_id,
                        kind: subagent_type.to_string(),
                        description: goal.clone(),
                        status: "failed".into(),
                        activity: String::new(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        output: e.to_string(),
                    },
                );
                ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!("Failed to run subagent: {}", e),
                    is_error: true,
                }
            }
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
        .with_memory(self.memory.clone())
        .with_file_index(self.file_index.clone())
        .with_panel(self.panel_sink());

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
            .with_memory(self.memory.clone())
            .with_file_index(self.file_index.clone())
            .with_panel(self.panel_sink()),
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
pub fn memory_settings_from_config(
    config: &whycode_config::Config,
) -> whycode_memory::MemorySettings {
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
    use serde_json::json;
    use whycode_core::types::PermissionSet;

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
        for name in SERIAL_TOOLS {
            assert!(
                !is_parallel_safe_tool(name, &PermissionSet::default()),
                "{name}"
            );
        }
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

        // .whycode/AGENTS.md is the fallback candidate
        let dir3 = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir3.path().join(".whycode")).unwrap();
        std::fs::write(dir3.path().join(".whycode/AGENTS.md"), "nested rules").unwrap();
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
        use whycode_config::StreamRuleConfig;
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
        let agents = dir.path().join(".whycode").join("agents");
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
        let config = whycode_config::Config::default();
        let m = memory_settings_from_config(&config);
        assert_eq!(m.enabled, config.memory.enabled);
        assert_eq!(m.auto_inject, config.memory.auto_inject);
        assert_eq!(m.auto_retain, config.memory.auto_retain);
        assert_eq!(m.retain_every_n, config.memory.retain_every_n);
        assert_eq!(
            m.scope,
            whycode_memory::MemoryScope::parse(&config.memory.scope)
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
        Agent::new(whycode_core::types::AgentInfo {
            name: "build".into(),
            description: "t".into(),
            mode: whycode_core::types::AgentMode::Primary,
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
        let session = whycode_session::session::Session::new("/tmp/proj".into(), "sys".into());
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
            whycode_session::session::Session::new(dir.path().to_path_buf(), "sys".into());

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
        let mut config = whycode_config::Config::default();
        config.session.model_fast = Some("haiku".into());
        let a = test_agent()
            .with_tool_profile(whycode_tools::ToolProfile::Full)
            .with_config(&config);
        assert_eq!(a.model_fast(), Some("haiku"));
        assert!(a.activated_tools_snapshot().is_empty());
        assert!(a.cwd_override_path().is_none());
        assert!(a.session_claims().is_none());
    }

    #[test]
    fn title_refine_target_needs_user_and_key() {
        let a = test_agent();
        let empty = whycode_session::session::Session::new("/tmp".into(), "sys".into());
        assert!(
            a.title_refine_target(&empty, "anthropic", "claude", "k", None)
                .is_none()
        );

        let mut s = whycode_session::session::Session::new("/tmp".into(), "sys".into());
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
