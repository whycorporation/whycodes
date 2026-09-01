//! Shared CLI helpers used by command bodies and tests.
use crate::Cli;
use colored::*;
use std::path::PathBuf;
use whycodes_agent::agent::Agent;
use whycodes_agent::events::{TurnEvent, TurnOpts, new_cancel_flag};
use whycodes_config::Config;
use whycodes_core::types::{AgentInfo, AgentMode, ModelConfig, PermissionSet};
use whycodes_protocol::{CiEvent, OutputFormat, ResultMeta};

pub(crate) fn cmd_completions(shell: clap_complete::Shell) -> anyhow::Result<()> {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "whycodes", &mut std::io::stdout());
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────
// Resolve helpers
// ────────────────────────────────────────────────────────────────────────

pub(crate) fn resolve_provider(cli: &Cli, config: &Config) -> String {
    cli.provider
        .clone()
        .or_else(|| {
            config
                .default_model
                .as_ref()
                .map(|m| m.provider_id.clone())
                .filter(|id| !id.is_empty())
        })
        .or_else(|| config.providers.keys().next().cloned())
        .unwrap_or_else(|| "anthropic".to_string())
}

pub(crate) fn resolve_model(cli: &Cli, config: &Config) -> String {
    cli.model.clone().unwrap_or_else(|| {
        config
            .default_model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string())
    })
}

pub(crate) fn resolve_agent(cli: &Cli, config: &Config) -> String {
    cli.agent_flag
        .clone()
        .unwrap_or_else(|| config.default_agent.clone())
}

pub(crate) fn resolve_dir(cli: &Cli) -> PathBuf {
    match &cli.dir {
        Some(d) if d != "." => PathBuf::from(d),
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Resolve the credential for `provider`: env var → config `api_key` →
/// OAuth token store (`whycodes auth login`), refreshing the token when it
/// is near expiry. Env and config win so a stored subscription login never
/// overrides an explicit key.
pub(crate) async fn get_api_key(provider: &str, config: &Config) -> Option<String> {
    if let Some(key) = key_from_env_and_config(provider, config, |k| std::env::var(k).ok()) {
        whycodes_llm::oauth_refresh::unregister(provider);
        return Some(key);
    }
    // OAuth subscription login (`whycodes auth login <provider>`).
    if whycodes_auth::providers::supports_oauth(provider)
        && let Ok(data_dir) = Config::data_dir()
        && let Some(token) = whycodes_auth::providers::access_token(provider, &data_dir).await
    {
        // A 401 on this credential may trigger one forced refresh + retry.
        whycodes_llm::oauth_refresh::register(provider, data_dir);
        return Some(token);
    }
    None
}

pub(crate) fn key_from_env_and_config(
    provider: &str,
    config: &Config,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let env_var = provider_env_var(provider);
    if let Some(key) = env(&env_var).filter(|k| !k.is_empty()) {
        return Some(key);
    }
    if let Some(pc) = config.get_provider(provider)
        && let Some(key) = &pc.api_key
        && !key.is_empty()
    {
        return Some(key.clone());
    }
    // openai: empty OPENAI_API_KEY still wins over a missing config key.
    if provider == "openai"
        && let Some(key) = env("OPENAI_API_KEY")
    {
        return Some(key);
    }
    None
}

pub(crate) fn missing_api_key_message_for(provider: &str, config: Option<&Config>) -> String {
    if !whycodes_llm::provider_requires_api_key(provider, config) {
        return format!(
            "Provider '{provider}' talks to a local host (loopback `base_url` / Ollama). No API key required."
        );
    }
    let oauth_hint = if whycodes_auth::providers::supports_oauth(provider) {
        format!(" Or log in with your subscription: `whycodes auth login {provider}`.")
    } else {
        String::new()
    };
    format!(
        "No API key for provider '{}'. Set {} env var.{}",
        provider,
        provider_env_var(provider),
        oauth_hint
    )
}

pub(crate) fn provider_env_var(provider: &str) -> String {
    format!("{}_API_KEY", provider.to_uppercase())
}

/// True when opening the database failed because there is nothing there yet,
/// as opposed to failing for a reason the user should hear about.
pub(crate) fn is_missing_database(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        e.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// Map CLI `--continue` / `--resume` onto a session lookup key.
///
/// `--resume` wins when both are set. Returns `None` when neither flag is used.
pub(crate) fn resolve_resume_want(cli: &Cli) -> Option<String> {
    if let Some(id) = cli
        .resume
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Some(id.to_string());
    }
    if cli.continue_session {
        return Some(whycodes_tui::RESUME_LATEST.to_string());
    }
    None
}

/// Load `want` into `session`, preserving the current system prompt.
///
/// Returns `Ok(true)` when a session was loaded, `Ok(false)` when none matched.
pub(crate) fn resume_session_into(
    session: &mut whycodes_session::session::Session,
    want: &str,
) -> anyhow::Result<bool> {
    let db = open_db()?;
    let Some(loaded) =
        whycodes_tui::resolve_and_load_session(&db, want).map_err(|e| anyhow::anyhow!("{e}"))?
    else {
        return Ok(false);
    };
    let system_prompt = session.system_prompt.clone();
    *session = loaded;
    session.system_prompt = system_prompt;
    // Legacy `New session - …` / placeholder titles: name from first user msg.
    if session.maybe_upgrade_title_from_history()
        && let Err(err) = session.save_to_db(&db)
    {
        tracing::warn!(error = %err, "failed to persist backfilled session title");
    }
    Ok(true)
}

pub(crate) fn open_db() -> anyhow::Result<whycodes_storage::db::Database> {
    let data_dir = Config::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("whycodes.db");
    whycodes_storage::db::Database::open(&db_path.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("Failed to open database: {}", e))
}

pub(crate) fn agent_info_for(cli: &Cli, config: &Config) -> AgentInfo {
    let agent_name = resolve_agent(cli, config);
    let provider = resolve_provider(cli, config);
    let model = resolve_model(cli, config);

    config
        .get_agent(&agent_name)
        .cloned()
        .unwrap_or_else(|| AgentInfo {
            name: "build".to_string(),
            description: "Default build agent".to_string(),
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
            model: Some(ModelConfig {
                model_id: model,
                provider_id: provider,
                max_tokens: None,
                context_window: None,
                temperature: None,
                top_p: None,
                thinking: None,
                supports_tools: Some(true),
                supports_images: None,
            }),
            system_prompt: None,
            temperature: None,
            top_p: None,
        })
}

// ────────────────────────────────────────────────────────────────────────
// Command implementations
// ────────────────────────────────────────────────────────────────────────

/// Refresh API key from env/config/OAuth store; print how to connect if
/// still missing. Returns false if no key is available (caller should not
/// call the LLM).
pub(crate) async fn ensure_api_key(api_key: &mut String, provider: &str, config: &Config) -> bool {
    if !api_key.is_empty() {
        return true;
    }
    if let Some(k) = get_api_key(provider, config).await {
        *api_key = k;
        return true;
    }
    if !whycodes_llm::provider_requires_api_key(provider, Some(config)) {
        return true;
    }
    let env = provider_env_var(provider);
    let oauth_hint = if whycodes_auth::providers::supports_oauth(provider) {
        format!("\n  → whycodes auth login {provider}  (subscription)")
    } else {
        String::new()
    };
    eprintln!(
        "{}\n  {}\n  {}{}\n  {}",
        format!("Setup needed · no API key for `{provider}`")
            .yellow()
            .bold(),
        format!("→ export {env}=…").dimmed(),
        format!("→ whycodes provider add {provider} --api-key <key>").dimmed(),
        oauth_hint.dimmed(),
        "Then /connect and try again.".dimmed(),
    );
    false
}

pub(crate) fn print_slash_help() {
    println!("{}", "Slash commands (OpenCode-compatible):".bold());
    println!("  /help, /h              — Show this help");
    println!("  /exit, /quit, /q       — Exit");
    println!("  /new, /clear           — Start a new session");
    println!("  /rename <name>         — Set session title (locks auto-title)");
    println!("  /init                  — Create/update AGENTS.md for this project");
    println!("  /undo                  — Undo last message + file changes (git)");
    println!("  /redo                  — Redo previously undone turn");
    println!("  /share, /export        — Export session JSON");
    println!("  /compact [context]     — Compact conversation (LLM summary)");
    println!("  /summarize             — Alias for /compact");
    println!("  /fresh                 — Skip provider prompt cache on the next turn");
    println!("  /diff                  — Git status + diff --stat");
    println!("  /context               — Context window breakdown");
    println!("  /review                — AI review of git changes");
    println!("  /security-review       — Security-focused review");
    println!("  /commit                — Draft a git commit");
    println!("  /cost, /usage          — Session token usage");
    println!("  /doctor                — Environment diagnostics");
    println!("  /remember <text>       — Save a durable project memory");
    println!("  /memory                — Show memory path and entry count");
    println!("  /sessions              — List saved sessions");
    println!("  /resume [id]           — Resume a session (list if no id)");
    println!("  /continue              — Resume the most recent session");
    println!("  /models [provider/id]  — List or switch models");
    println!("  /effort [low|medium|high|xhigh] — Reasoning effort");
    println!("  /agent [name]          — List or switch agents (build|plan|…)");
    println!("  /connect               — Provider setup help");
    println!("  /login [provider]      — Subscription sign-in (list if none)");
    println!("  /thinking              — Toggle thinking display");
    println!("  /themes                — Theme info");
    println!("  /tools                 — List tools for current agent");
    println!("  /info, /details        — Session info");
    println!();
    println!("{}", "Also:".bold());
    println!("  !cmd                   — Run shell command, add output to chat");
    println!("  @path/to/file          — Include file contents in your message");
    println!("  Custom commands        — .whycodes/commands/*.md or config [commands]");
    println!("  whycodes memory …       — list|search|add|delete|clear|path");
    println!("  whycodes --no-memory    — disable memory for this process");
    println!("  whycodes --plain        — readline REPL instead of TUI");
}

pub(crate) fn split_slash_command(input: &str) -> (&str, &str) {
    let s = input.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

/// Settings bag for `whycodes-memory` from config (config does not depend on memory).
pub(crate) fn memory_settings(config: &Config) -> whycodes_memory::MemorySettings {
    memory_settings_for(config, None)
}

pub(crate) fn memory_settings_for(
    config: &Config,
    agent_bank: Option<String>,
) -> whycodes_memory::MemorySettings {
    let mut s = whycodes_agent::memory_settings_from_config(config);
    s.agent_bank = agent_bank;
    s
}

/// Best-effort code index on session start (skips if already indexed).
pub(crate) fn maybe_session_auto_index(project_dir: &std::path::Path, config: &Config) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(n) =
        whycodes_memory::maybe_auto_index(project_dir, &data_dir, &memory_settings(config))
    {
        println!("{} Auto-indexed {n} code chunks", "📇".dimmed());
    }
}

pub(crate) fn with_project_memory(
    system_prompt: &str,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) -> String {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    whycodes_memory::apply_memory_prompt(
        system_prompt,
        project_dir,
        &data_dir,
        &memory_settings(config),
        query,
    )
}

/// Rebuild system prompt with AGENTS.md + memory recall for the current query.
pub(crate) fn refresh_session_memory(
    session: &mut whycodes_session::session::Session,
    agent: &Agent,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) {
    let base = Agent::with_agents_md(&agent.system_prompt(), project_dir);
    session.set_system_prompt(&with_project_memory(&base, project_dir, config, query));
}

pub(crate) fn open_memory_service(
    cli: &Cli,
    config: &Config,
) -> anyhow::Result<whycodes_memory::MemoryService> {
    let project_dir = resolve_dir(cli);
    let data_dir = Config::data_dir()?;
    Ok(whycodes_memory::MemoryService::open(
        project_dir,
        data_dir,
        memory_settings(config),
    )?)
}
pub(crate) fn switch_agent(
    name: &str,
    config: &Config,
    project_dir: &std::path::Path,
) -> anyhow::Result<(String, Agent, String)> {
    let name = name.trim();
    let info = config.get_agent(name).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown agent '{}'. Try: build, plan, explore, general, scout",
            name
        )
    })?;
    let base = info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(name));
    let prompt = with_project_memory(
        &Agent::with_agents_md(&base, project_dir),
        project_dir,
        config,
        None,
    );
    let agent = Agent::new(info);
    Ok((name.to_string(), agent, prompt))
}

/// Max chars inlined per `@file` (matches TUI; keeps prefill bounded).
pub(crate) const AT_FILE_MAX_CHARS: usize = 24_000;

/// Expand `@path` file references and return the full prompt text.
pub(crate) fn expand_user_input(input: &str, project_dir: &std::path::Path) -> String {
    let mut result = String::new();
    let mut rest = input;
    while let Some(at) = rest.find('@') {
        result.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        // path continues until whitespace or end
        let end = after
            .find(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .unwrap_or(after.len());
        let path_str = &after[..end];
        if path_str.is_empty() {
            result.push('@');
            rest = after;
            continue;
        }
        let path = if std::path::Path::new(path_str).is_absolute() {
            std::path::PathBuf::from(path_str)
        } else {
            project_dir.join(path_str)
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let n = content.chars().count();
                let body = if n <= AT_FILE_MAX_CHARS {
                    content
                } else {
                    let mut t: String = content.chars().take(AT_FILE_MAX_CHARS).collect();
                    t.push_str(&format!(
                        "\n\n[... {} characters omitted from @{} — use the read tool for the rest]",
                        n - AT_FILE_MAX_CHARS,
                        path_str
                    ));
                    t
                };
                result.push_str(&format!(
                    "\n\n--- file: {path_str} ---\n{body}\n--- end file ---\n\n"
                ));
            }
            Err(_missing) => {
                // keep as plain text if file missing
                result.push('@');
                result.push_str(path_str);
            }
        }
        rest = &after[end..];
    }
    result.push_str(rest);
    result
}

pub(crate) fn run_shell_capture(cmd: &str, cwd: &std::path::Path) -> String {
    #[cfg(windows)]
    let output = std::process::Command::new("cmd")
        .args(["/C", cmd])
        .current_dir(cwd)
        .output();
    #[cfg(not(windows))]
    let output = std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .output();

    match output {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.is_empty() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(&err);
            }
            if s.is_empty() {
                s = format!("(exit {})", o.status.code().unwrap_or(-1));
            }
            s
        }
        Err(e) => format!("Failed to run command: {}", e),
    }
}

/// `/init` — analyze project and write AGENTS.md (OpenCode parity).
/// Parse `--format` / `--output-format` CLI values.
pub(crate) fn parse_output_format(s: &str) -> Result<OutputFormat, String> {
    s.parse()
}

/// Setup failures before a turn starts (missing key, empty prompt, …).
pub(crate) fn emit_headless_setup_error(format: OutputFormat, message: &str) -> anyhow::Result<()> {
    match format {
        OutputFormat::Text => {
            eprintln!("{} {}", "Error:".red().bold(), message);
            Err(anyhow::anyhow!("{}", message))
        }
        OutputFormat::Json | OutputFormat::StreamJson => {
            let _ = CiEvent::Error {
                message: message.to_string(),
            }
            .emit_stdout();
            Err(anyhow::anyhow!("{}", message))
        }
    }
}

/// Run one agent turn and write stdout according to `format`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_headless_turn(
    agent: &Agent,
    session: &mut whycodes_session::session::Session,
    provider: &str,
    model: &str,
    api_key: &str,
    agent_name: &str,
    max_turns: Option<usize>,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let session_id = session.id.clone();
    let cwd = session.project_path.display().to_string();

    if format == OutputFormat::StreamJson {
        let _ = CiEvent::Init {
            session_id: session_id.clone(),
            provider: provider.to_string(),
            model: model.to_string(),
            agent: agent_name.to_string(),
            cwd,
        }
        .emit_stdout();
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let cancel = new_cancel_flag();

    // Drain TurnEvents → CiEvent while the agent runs.
    let stream = format == OutputFormat::StreamJson;
    let drain = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if !stream {
                continue;
            }
            if let Some(ci) = turn_event_to_ci(ev)
                && let Err(e) = ci.emit_stdout()
            {
                tracing::debug!(error = %e, "ci event emit skipped");
            }
        }
    });

    let turn_result = agent
        .run_turn_with_events(
            session,
            TurnOpts {
                provider_name: provider,
                model,
                api_key,
                max_turns,
                events: Some(event_tx),
                cancel: Some(cancel),
            },
        )
        .await;

    // Drop the sender side (inside agent) already closed; wait for drain.
    if let Err(e) = drain.await {
        tracing::debug!(error = %e, "ci event drain skipped");
    }

    let duration_ms = started.elapsed().as_millis() as u64;
    let meta = ResultMeta {
        session_id: session.id.clone(),
        provider: provider.to_string(),
        model: model.to_string(),
        agent: agent_name.to_string(),
        usage: session.usage.clone(),
        duration_ms,
    };

    emit_turn_outcome(format, turn_result.map_err(|e| e.to_string()), meta)
        .map_err(|m| anyhow::anyhow!("{}", m))
}

pub(crate) fn emit_turn_outcome(
    format: OutputFormat,
    result: Result<String, String>,
    meta: ResultMeta,
) -> Result<(), String> {
    match result {
        Ok(response) => {
            match format {
                OutputFormat::Text => {
                    if !response.is_empty() {
                        println!("{response}");
                    }
                }
                OutputFormat::Json | OutputFormat::StreamJson => {
                    let _ = meta.ok(response).emit_stdout();
                }
            }
            Ok(())
        }
        Err(msg) => {
            log_cli_turn_error(&meta, &msg);
            match format {
                OutputFormat::Text => {
                    eprintln!("{} {}", "Error:".red().bold(), msg);
                }
                OutputFormat::Json => {
                    let _ = meta.err(&msg).emit_stdout();
                }
                OutputFormat::StreamJson => {
                    if is_cancel_message(&msg) {
                        let _ = CiEvent::Cancelled.emit_stdout();
                    } else {
                        let _ = CiEvent::Error {
                            message: msg.clone(),
                        }
                        .emit_stdout();
                    }
                    let _ = meta.err(&msg).emit_stdout();
                }
            }
            Err(msg)
        }
    }
}

pub(crate) fn is_cancel_message(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains("cancel")
}

pub(crate) fn log_cli_turn_error(meta: &ResultMeta, msg: &str) {
    whycodes_core::logging::emit_sid(
        "cli",
        "error",
        "turn.error",
        Some(meta.session_id.as_str()),
        Some(serde_json::json!({
            "error": msg,
            "provider": meta.provider,
            "model": meta.model,
        })),
    );
}

pub(crate) fn strip_agents_fence(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("```markdown")
        .trim_start_matches("```md")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string()
}

/// Map an agent turn event onto a CI wire event (skips Cancelled mid-stream).
pub(crate) fn turn_event_to_ci(ev: TurnEvent) -> Option<CiEvent> {
    match ev {
        TurnEvent::TextDelta(text) => Some(CiEvent::TextDelta { text }),
        TurnEvent::ThinkingDelta(text) => Some(CiEvent::ThinkingDelta { text }),
        TurnEvent::ToolStart { id, name, input } => Some(CiEvent::ToolStart { id, name, input }),
        TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        } => Some(CiEvent::ToolEnd {
            id,
            content,
            is_error,
        }),
        TurnEvent::Usage(u) => Some(CiEvent::Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_input_tokens: u.cache_creation_input_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
        }),
        TurnEvent::Status(message) => Some(CiEvent::Status { message }),
        TurnEvent::Intent {
            kind,
            confidence,
            badge,
            notice_kind,
            notice,
        } => {
            // Surface as status so CI consumers see intent without a schema break.
            let mut message = format!("intent:{kind} conf={confidence:.2}");
            if !badge.is_empty() {
                message.push_str(&format!(" badge={badge}"));
            }
            if !notice.is_empty() {
                message.push_str(&format!(" [{notice_kind}] {notice}"));
            }
            Some(CiEvent::Status { message })
        }
        TurnEvent::Cancelled => Some(CiEvent::Cancelled),
        // Surface swarm coordination as status lines (no CI schema break).
        TurnEvent::FileConflict {
            path,
            claimant,
            owner,
        } => Some(CiEvent::Status {
            message: format!("file_conflict path={path} claimant={claimant} owner={owner}"),
        }),
        TurnEvent::SwarmStatus {
            active,
            total,
            message,
        } => Some(CiEvent::Status {
            message: if message.is_empty() {
                format!("swarm active={active} total={total}")
            } else {
                message
            },
        }),
        TurnEvent::Background {
            id,
            status,
            summary,
        } => Some(CiEvent::Status {
            message: format!("bg {id} {status}: {summary}"),
        }),
        TurnEvent::EnqueuePrompt { text } => Some(CiEvent::Status {
            message: format!("enqueue_prompt: {text}"),
        }),
        TurnEvent::SwarmMessage { from, to, text } => Some(CiEvent::Status {
            message: format!("swarm_msg from={from} to={to}: {text}"),
        }),
        TurnEvent::FileStale {
            path,
            reader,
            writer,
        } => Some(CiEvent::Status {
            message: format!("file_stale path={path} reader={reader} writer={writer}"),
        }),
        TurnEvent::PermissionAsk {
            request_id,
            tool_name,
            ..
        } => Some(CiEvent::Status {
            message: format!("permission_request id={request_id} tool={tool_name}"),
        }),
        TurnEvent::QuestionAsk { request_id, .. } => Some(CiEvent::Status {
            message: format!("question_request id={request_id}"),
        }),
        TurnEvent::Panel(update) => Some(CiEvent::Status {
            message: match update {
                whycodes_core::PanelUpdate::Clear => "panel clear".into(),
                whycodes_core::PanelUpdate::File { path, .. } => format!("panel file={path}"),
                whycodes_core::PanelUpdate::Diff { path, .. } => format!("panel diff={path}"),
                whycodes_core::PanelUpdate::Mermaid { .. } => "panel mermaid".into(),
            },
        }),
        TurnEvent::Subagent {
            id,
            kind,
            description,
            status,
            ..
        } => Some(CiEvent::Status {
            message: format!("subagent {id} {status} ({kind}): {description}"),
        }),
        TurnEvent::Todos { todos } => {
            let done = whycodes_core::todo::terminal_count(&todos);
            Some(CiEvent::Status {
                message: format!("todos {done}/{}", todos.len()),
            })
        }
    }
}

/// `acp` — Agent Client Protocol stub (deferred until after product launch).
/// Real target: editor ↔ agent (JSON-RPC), not agent-to-agent. See docs/roadmap.md.
pub(crate) fn load_auth_plugins(cli: &Cli) {
    let mut dirs = Vec::new();
    if let Some(global) = whycodes_plugin::global_plugins_dir() {
        dirs.push(global);
    }
    dirs.push(whycodes_plugin::project_plugins_dir(&resolve_dir(cli)));
    let n = whycodes_auth::plugin::load_from_dirs(&dirs);
    if n > 0 {
        tracing::info!(count = n, "loaded auth plugins");
    }
}

pub(crate) fn oauth_provider_list() -> String {
    let names = whycodes_auth::oauth_providers();
    if names.is_empty() {
        "none — install an auth plugin (plugin.json with kind \"auth\")".into()
    } else {
        names.join(", ")
    }
}
// ────────────────────────────────────────────────────────────────────────
// Utility helpers
// ────────────────────────────────────────────────────────────────────────

pub(crate) fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated = s.chars().take(max_len - 3).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}

/// First/last 4 bytes of a secret, floored/ceiled to char boundaries.
/// Byte slices (`&val[..4]`) panic when a multi-byte char straddles the cut.
pub(crate) fn mask_secret(val: &str) -> String {
    if val.len() <= 8 {
        return "***".to_string();
    }
    let start = val.floor_char_boundary(4);
    let end = val.ceil_char_boundary(val.len().saturating_sub(4));
    format!("{}...{}", &val[..start], &val[end..])
}
