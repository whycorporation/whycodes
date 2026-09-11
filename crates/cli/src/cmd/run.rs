//! Interactive REPL and headless generate.
use super::helpers::*;
use crate::Cli;
use crate::args::*;
use colored::*;
use std::path::PathBuf;
use std::sync::Arc;
use whycodes_agent::agent::Agent;
use whycodes_agent::events::{TurnEvent, TurnOpts, new_cancel_flag};
use whycodes_agent::permission::AutoApprovePrompter;
use whycodes_config::Config;
use whycodes_core::types::AgentInfo;
use whycodes_protocol::{CiEvent, OutputFormat, ResultMeta};

pub(crate) fn force_plain_mode(cli_plain: bool) -> bool {
    cli_plain || std::env::var_os("WHYCODES_PLAIN").is_some()
}

pub(crate) fn should_use_tui(force_plain: bool, stub_tui: bool, tui_available: bool) -> bool {
    !force_plain && (stub_tui || tui_available)
}

pub(crate) fn is_repl_interactive(prompt: Option<&str>, structured: bool) -> bool {
    prompt.is_none_or(str::is_empty) && !structured
}

pub(crate) fn resume_missing_label(want: &str) -> &str {
    if want == whycodes_tui::RESUME_LATEST {
        "none saved yet"
    } else {
        want
    }
}

pub(crate) fn session_token_label(
    usage_empty: bool,
    estimated: usize,
    input: u64,
    output: u64,
    total: u64,
) -> String {
    if usage_empty {
        format!("Tokens≈{estimated} (est)")
    } else {
        format!("Tokens: {input} in / {output} out / {total} total")
    }
}

pub(crate) fn session_cost_line(
    usage_empty: bool,
    estimated: usize,
    input: u64,
    output: u64,
    total: u64,
) -> String {
    if usage_empty {
        format!("  session: ~{estimated} tokens (estimated)")
    } else {
        format!("  session: {input} in / {output} out · total {total}")
    }
}

pub(crate) fn doctor_api_key_status(key_ok: bool, api_key_empty: bool) -> &'static str {
    if key_ok {
        if api_key_empty { "not required" } else { "set" }
    } else {
        "MISSING"
    }
}

pub(crate) enum ResumeSlash {
    List,
    Id(String),
}

pub(crate) fn resume_slash_want(cmd: &str, rest: &str) -> ResumeSlash {
    if !rest.is_empty() {
        ResumeSlash::Id(rest.to_string())
    } else if cmd == "/continue" {
        ResumeSlash::Id(whycodes_tui::RESUME_LATEST.to_string())
    } else {
        ResumeSlash::List
    }
}

pub(crate) enum ModelsSlash {
    ProviderModel(String, String),
    ModelOnly(String),
}

pub(crate) fn parse_models_slash(rest: &str) -> ModelsSlash {
    if let Some((p, m)) = rest.split_once('/') {
        ModelsSlash::ProviderModel(p.to_string(), m.to_string())
    } else {
        ModelsSlash::ModelOnly(rest.to_string())
    }
}

pub(crate) fn thinking_display_label(show_thinking: bool) -> String {
    if show_thinking {
        "ON".green().to_string()
    } else {
        "OFF".dimmed().to_string()
    }
}

pub(crate) enum EffortSlash {
    Show,
    Set(whycodes_llm::ReasoningEffort),
    Unknown,
}

pub(crate) fn parse_effort_slash(rest: &str) -> EffortSlash {
    if rest.is_empty() {
        EffortSlash::Show
    } else if let Some(parsed) = whycodes_llm::ReasoningEffort::parse(rest) {
        EffortSlash::Set(parsed)
    } else {
        EffortSlash::Unknown
    }
}

pub(crate) fn masked_api_key_prefix(api_key: &str) -> String {
    api_key.chars().take(8).collect()
}

pub(crate) fn unknown_slash_line(cmd: &str) -> String {
    format!("Unknown command: {cmd}. Type /help")
}

pub(crate) fn new_session_line(title: &str) -> String {
    format!("{} New session started ({})", "✓".green(), title.dimmed())
}

pub(crate) fn rename_usage_line(title: &str, source: impl std::fmt::Debug) -> String {
    format!(
        "Title: {} ({source:?}) — usage: /rename <name>",
        title.cyan()
    )
}

pub(crate) fn renamed_line(title: &str) -> String {
    format!("{} Renamed to '{}'", "✓".green(), title.cyan())
}

pub(crate) fn undid_turn_line(n: usize) -> String {
    format!("{} Undid last turn ({n} messages left).", "↩".cyan())
}

pub(crate) fn redid_turn_line(n: usize) -> String {
    format!("{} Redid turn ({n} messages).", "↪".cyan())
}

pub(crate) fn compact_ok_line(
    messages_before: usize,
    messages_after: usize,
    tokens_before: usize,
    tokens_after: usize,
) -> String {
    format!(
        "{} Conversation compacted ({messages_before} → {messages_after} messages, ~{tokens_before} → ~{tokens_after} tok).",
        "✓".green()
    )
}

pub(crate) fn context_report_lines(
    message_count: usize,
    estimated: usize,
    compaction_threshold: usize,
    compaction_llm: &str,
    tool_profile: &str,
) -> Vec<String> {
    vec![
        "Context".bold().to_string(),
        format!("  messages: {message_count}"),
        format!("  estimate: ~{estimated} tok"),
        format!("  compact:  threshold={compaction_threshold} llm={compaction_llm}"),
        format!("  tools:    profile={tool_profile}"),
    ]
}

pub(crate) fn doctor_report_lines(
    provider: &str,
    model: &str,
    project: &str,
    api_key: &str,
    sandbox: &str,
    sandbox_network: bool,
    tool_profile: &str,
) -> Vec<String> {
    vec![
        "Doctor".bold().to_string(),
        format!("  provider: {provider}"),
        format!("  model:    {model}"),
        format!("  project:  {project}"),
        format!("  api_key:  {api_key}"),
        format!("  sandbox:  {sandbox} network={sandbox_network}"),
        format!("  tools:    profile={tool_profile}"),
    ]
}

pub(crate) fn resumed_line(title: &str, id: &str, n: usize) -> String {
    format!(
        "{} Resumed {} ({}) — {n} messages",
        "✓".green(),
        title.cyan(),
        id.chars().take(8).collect::<String>().dimmed()
    )
}

pub(crate) fn switched_model_line(provider: &str, model: &str) -> String {
    format!(
        "{} Switched model to {}/{}",
        "✓".green(),
        provider.cyan(),
        model.cyan()
    )
}

pub(crate) fn model_set_line(model: &str) -> String {
    format!("{} Model set to {}", "✓".green(), model.cyan())
}

pub(crate) fn effort_unknown_line(rest: &str) -> String {
    format!(
        "{} Unknown effort '{rest}' (low, medium, high, xhigh)",
        "✗".red()
    )
}

pub(crate) fn effort_no_levels_line() -> String {
    format!("{} This model has no reasoning-effort levels", "·".dimmed())
}

pub(crate) fn switched_agent_line(name: &str) -> String {
    format!("{} Switched to agent '{}'", "✓".green(), name.cyan())
}

pub(crate) fn api_key_loaded_line(provider: &str, prefix: &str) -> String {
    format!(
        "{} API key loaded for {} ({prefix}…)",
        "✓".green(),
        provider.cyan()
    )
}

pub(crate) fn connect_missing_key_lines(provider: &str, oauth: bool) -> Vec<String> {
    let mut lines = vec![
        "Add a provider:".into(),
        format!("  whycodes provider add {provider} --api-key <key>"),
        format!("  or set env {}", provider_env_var(provider)),
    ];
    if oauth {
        lines.push(format!(
            "  or log in with your subscription: whycodes auth login {provider}"
        ));
    }
    lines.push(String::new());
    lines.push(
        "Env vars: ANTHROPIC_API_KEY, OPENAI_API_KEY, XAI_API_KEY, GOOGLE_API_KEY, ...".into(),
    );
    lines
}

pub(crate) fn login_connected_label(connected: bool) -> String {
    if connected {
        "connected".green().to_string()
    } else {
        "not connected".dimmed().to_string()
    }
}

pub(crate) fn oauth_unavailable_line(arg: &str, list: &str) -> String {
    format!(
        "OAuth login is not available for `{arg}` — choose from: {list}",
        arg = arg.red()
    )
}

pub(crate) fn themes_set_hint(first: &str) -> String {
    format!("Set in config: [tui] theme = \"{first}\"")
}

pub(crate) fn tools_list_header(n: usize) -> String {
    format!("{} Available tools ({n}):", "🔧".bold())
}

pub(crate) fn remembered_line(id: &str, text: &str) -> String {
    format!(
        "{} Remembered {} — {text}",
        "✓".green(),
        id.chars().take(8).collect::<String>().cyan()
    )
}

pub(crate) fn repl_memory_status(enabled: bool, n: usize, path: impl std::fmt::Display) -> String {
    format!("Memory: enabled={enabled}  entries={n}  path={path}")
}

pub(crate) fn nothing_to_undo_line() -> String {
    format!("{} Nothing to undo.", "ℹ".cyan())
}

pub(crate) fn nothing_to_redo_line() -> String {
    format!("{} Nothing to redo.", "ℹ".cyan())
}

pub(crate) fn init_wrote_line(path: &str) -> String {
    format!(
        "{} Wrote project instructions: {}",
        "✓".green(),
        path.cyan()
    )
}

pub(crate) fn init_failed_line(err: &str) -> String {
    format!("{} /init failed: {}", "✗".red(), err)
}

pub(crate) fn session_exported_line(path: &str) -> String {
    format!("{} Session exported: {}", "✓".green(), path.cyan())
}

pub(crate) fn export_failed_line(err: &str) -> String {
    format!("{} Export failed: {}", "✗".red(), err)
}

pub(crate) fn skip_prompt_cache_line() -> String {
    format!(
        "{} Next turn will skip the provider prompt cache.",
        "✓".green()
    )
}

pub(crate) fn nothing_to_compact_line() -> String {
    format!("{} Nothing to compact.", "ℹ".cyan())
}

pub(crate) fn compacting_line() -> String {
    format!("{} Compacting conversation…", "…".dimmed())
}

pub(crate) fn bang_usage_line() -> &'static str {
    "Usage: ! <shell command>"
}

pub(crate) fn bang_echo_line(cmd: &str) -> String {
    format!("{} {}", "$".dimmed(), cmd.dimmed())
}

pub(crate) fn custom_command_line(name: &str) -> String {
    format!("{} /{} → prompt", "⚡".bold(), name.cyan())
}

pub(crate) fn git_unavailable_line(err: &str) -> String {
    format!("{} git unavailable: {}", "✗".red(), err)
}

pub(crate) fn git_status_failed_line(stderr: &str) -> String {
    format!("{} git status: {stderr}", "✗".red())
}

pub(crate) fn map_tui_run_error(e: anyhow::Error) -> anyhow::Error {
    let msg = e.to_string();
    if msg.contains("No such device")
        || msg.contains("os error 6")
        || msg.contains("not a terminal")
    {
        anyhow::anyhow!(
            "{msg}\n\n\
             TUI needs a real terminal. Run in a terminal emulator, or:\n\
               whycodes --plain"
        )
    } else {
        e
    }
}

/// Clap-free TUI entry for `whycodes` / `whycodes run -d <dir>`.
///
/// The first-frame harness is this argv. Building clap + a Tokio runtime
/// before paint was tens of ms on Windows (issue #85 follow-up).
pub(crate) fn cmd_run_fast_tui(project_dir: PathBuf) -> anyhow::Result<()> {
    if std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty()) {
        let exit = whycodes_tui::paint_first_frame_sync()
            .map_err(map_tui_run_error)?
            .unwrap_or(whycodes_tui::TuiExit::Quit);
        return match exit {
            whycodes_tui::TuiExit::Quit => Ok(()),
            whycodes_tui::TuiExit::Upgrade => Ok(()),
        };
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .enable_time()
        .build()?;
    let exit = rt
        .block_on(whycodes_tui::run(whycodes_tui::TuiRunOptions {
            project_dir,
            provider: "anthropic".into(),
            model: "claude-sonnet-4-20250514".into(),
            api_key: String::new(),
            agent_name: "build".into(),
            max_turns: None,
            initial_prompt: None,
            config: Config::default(),
            resume_session_id: None,
            remote: None,
            defer_config_load: true,
            provider_from_cli: false,
            model_from_cli: false,
            agent_from_cli: false,
            update_rx: None,
            inject: Default::default(),
        }))
        .map_err(map_tui_run_error)?;
    match exit {
        whycodes_tui::TuiExit::Quit => Ok(()),
        whycodes_tui::TuiExit::Upgrade => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(super::debug::after_tui_exit(exit))
        }
    }
}

pub(crate) async fn cmd_run(
    cli: &Cli,
    prompt: Option<&str>,
    max_turns: Option<usize>,
    format: OutputFormat,
) -> anyhow::Result<()> {
    // Structured output is headless-only; needs a prompt.
    if format.is_structured() {
        let Some(prompt) = prompt.filter(|p| !p.is_empty()) else {
            anyhow::bail!(
                "--format {format} requires a non-empty prompt \
                 (e.g. `whycodes run \"…\" --format {format}` or `whycodes generate \"…\" --format {format}`)"
            );
        };
        let prompt_owned = prompt.to_string();
        return self::cmd_generate(
            cli,
            std::slice::from_ref(&prompt_owned),
            max_turns,
            1,
            format,
        )
        .await;
    }

    let project_dir_early = resolve_dir(cli);
    // Full-screen TUI unless --plain / WHYCODES_PLAIN. Decide *before*
    // layered TOML so the first-frame clock is not waiting on config I/O.
    let force_plain = force_plain_mode(cli.plain);
    let stub_tui = cfg!(test) && std::env::var_os("WHYCODES_TEST_TUI").is_some();
    let use_tui = should_use_tui(force_plain, stub_tui, whycodes_tui::tui_available());
    let project_dir = project_dir_early.clone();

    let mut config;
    let provider;
    let model;
    let agent_name;
    if use_tui {
        // Empty config until after first paint. CLI flags still win via
        // `resolve_*`; `load_layered` runs after `record_draw` (issue #85).
        config = Config::default();
        if cli.no_memory {
            config.memory.enabled = false;
        }
        provider = resolve_provider(cli, &config);
        model = resolve_model(cli, &config);
        agent_name = resolve_agent(cli, &config);
    } else {
        config = Config::load_layered(&project_dir_early)
            .or_else(|_| Config::load())
            .unwrap_or_default();
        if cli.no_memory {
            config.memory.enabled = false;
        }
        provider = resolve_provider(cli, &config);
        model = resolve_model(cli, &config);
        agent_name = resolve_agent(cli, &config);
        config.load_command_files(&project_dir);
    }
    let interactive = is_repl_interactive(prompt, format.is_structured());
    // TUI owns first-run import as a home-screen confirm (same chrome as
    // the update offer). `--plain` REPL still asks on stdin before the loop.
    match super::import::maybe_first_run_import(interactive && !use_tui) {
        Ok(true) => match Config::load_layered(&project_dir_early) {
            Ok(reloaded) => {
                config = reloaded;
                if cli.no_memory {
                    config.memory.enabled = false;
                }
                config.load_command_files(&project_dir_early);
            }
            Err(e) => {
                eprintln!("{} reloading config after import: {e}", "warning:".yellow());
            }
        },
        Ok(false) => {}
        Err(e) => {
            eprintln!("{} first-run import: {e}", "warning:".yellow());
        }
    }
    if !use_tui && !force_plain {
        use std::io::IsTerminal;
        eprintln!(
            "whycodes: no interactive terminal \
             (stdin_tty={} stdout_tty={} controlling console unavailable).\n\
             Falling back to plain mode. Use a real terminal, or pass --plain.",
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        );
    }
    let resume_want = resolve_resume_want(cli);
    // Grok parity: `--max-turns` is a headless cap. Interactive TUI/REPL
    // runs until end-of-turn, cancel, or doom-loop.
    let max_turns = crate::ignore_max_turns_interactive(max_turns);

    // TUI first paint is latency-sensitive; a blocking `auth.json` / token
    // read before `whycodes_tui::run` adds File I/O to TTFF. Defer key fetch
    // until after the first frame — interactive mode already treats a missing
    // key as OK until the first LLM turn (same pattern as MCP/auto-index).
    let mut api_key = if use_tui {
        String::new()
    } else {
        get_api_key(&provider, &config).await.unwrap_or_default()
    };

    if use_tui {
        let update_rx = super::debug::spawn_update_check(cli, &config);
        let exit = whycodes_tui::run(whycodes_tui::TuiRunOptions {
            project_dir,
            provider,
            model,
            api_key,
            agent_name,
            max_turns,
            initial_prompt: prompt.map(|s| s.to_string()),
            config,
            resume_session_id: resume_want,
            remote: None,
            defer_config_load: true,
            provider_from_cli: cli.provider.is_some(),
            model_from_cli: cli.model.is_some(),
            agent_from_cli: cli.agent_flag.is_some(),
            update_rx,
            inject: Default::default(),
        })
        .await
        .map_err(map_tui_run_error)?;
        return super::debug::after_tui_exit(exit).await;
    }

    let agent_info = {
        let mut info = agent_info_for(cli, &config);
        info.permission = config.effective_permission(&info.permission);
        info
    };
    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base_prompt, &project_dir),
        &project_dir,
        &config,
        None,
    );

    // Wall clock for the Cline-style exit summary (process open → quit).
    let session_started = std::time::Instant::now();

    let mut agent_name = agent_name;
    config.general.project_path = Some(project_dir.clone());
    let file_index = whycodes_index::WorkspaceIndex::start(
        whycodes_index::WorkspaceIndex::project_roots(&project_dir),
    );
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index)
        .with_mcp(&config)
        .await;
    maybe_inject_test_llm(&mut agent, &provider);
    let mut session = whycodes_session::session::Session::new(project_dir.clone(), system_prompt);
    maybe_session_auto_index(&project_dir, &config);
    let mut history = whycodes_session::SessionHistory::new();
    let mut provider = provider;
    let mut model = model;
    let mut show_thinking = false;

    // Plain-mode resume (same flags as TUI).
    if let Some(ref want) = resume_want {
        match resume_session_into(&mut session, want) {
            Ok(true) => {
                println!(
                    "{} Resumed session {} ({}) — {} messages",
                    "✓".green(),
                    session.title.cyan(),
                    session.id.chars().take(8).collect::<String>().dimmed(),
                    session.messages.len()
                );
            }
            Ok(false) => {
                eprintln!(
                    "{} No session to resume ({}).",
                    "ℹ".yellow(),
                    resume_missing_label(want)
                );
            }
            Err(e) => eprintln!("{} Resume failed: {e}", "✗".red()),
        }
    }

    println!(
        "{} {}",
        "WhyCodes".cyan().bold(),
        format!(
            "[agent={}, provider={}, model={}]",
            agent_name, provider, model
        )
        .dimmed()
    );
    println!(
        "{} {}",
        "Project:".dimmed(),
        project_dir.display().to_string().dimmed()
    );
    if api_key.is_empty() && whycodes_llm::provider_requires_api_key(&provider, Some(&config)) {
        println!(
            "{} No API key for '{}'. Set {} or run /connect. UI is ready.",
            "ℹ".yellow(),
            provider.cyan(),
            provider_env_var(&provider).cyan()
        );
    }
    println!();

    if let Some(prompt) = prompt {
        if prompt.is_empty() {
            eprintln!("{}", "Error: empty prompt".red());
            return Ok(());
        }
        if api_key.is_empty() && whycodes_llm::provider_requires_api_key(&provider, Some(&config)) {
            eprintln!(
                "{} No API key for '{}'. Set {} then retry.",
                "Error:".red().bold(),
                provider,
                provider_env_var(&provider)
            );
            return Ok(());
        }
        let expanded = expand_user_input(prompt, &project_dir);
        refresh_session_memory(&mut session, &agent, &project_dir, &config, Some(&expanded));
        session.add_user_message(&expanded);
        if config.session.auto_title {
            // Prefer first user message (resume of placeholder-titled sessions).
            let seed = session
                .first_user_text()
                .unwrap_or_else(|| expanded.clone());
            // bool: whether the title changed (not a Result).
            session.apply_heuristic_title(&seed);
        }
        let (run_provider, run_model) = whycodes_agent::resolve_turn_model(
            &provider,
            &model,
            &expanded,
            config.session.model_fast.as_deref(),
        );
        match agent
            .run_turn(&mut session, &run_provider, &run_model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if config.session.auto_title {
                    agent
                        .maybe_refine_title(
                            &mut session,
                            &provider,
                            &model,
                            &api_key,
                            config.session.title_model.as_deref(),
                        )
                        .await;
                }
                if !response.is_empty() {
                    println!("\n{}", response);
                }
                // Retain is spawned inside Agent::run_turn (async; best-effort).
                if let Ok(db) = open_db() {
                    let _ = session.save_to_db(&db);
                }
            }
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                if let Ok(db) = open_db() {
                    let _ = session.save_to_db(&db);
                }
                return Err(anyhow::anyhow!("{}", e));
            }
        }
        let model_label = format!("{provider}/{model}");
        print!(
            "{}",
            session.format_exit_summary(session_started.elapsed(), &model_label, "whycodes")
        );
        return Ok(());
    }

    println!(
        "{}",
        "Interactive mode. Type /help for commands, /agent build|plan to switch. /exit to quit."
            .dimmed()
    );
    loop {
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        match read_repl_line(&mut input) {
            Ok(0) => break,
            Err(_eof_or_closed) => break,
            Ok(_) => {}
        }
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }

        // OpenCode: !command runs bash and injects output into the conversation
        if let Some(cmd) = input.strip_prefix('!') {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                println!("{}", bang_usage_line());
                continue;
            }
            println!("{}", bang_echo_line(cmd));
            let output = run_shell_capture(cmd, &project_dir);
            println!("{}", output);
            session.add_user_message(&format!(
                "I ran the shell command `{}` and got:\n```\n{}\n```",
                cmd, output
            ));
            continue;
        }

        if input.starts_with('/') {
            let (cmd, rest) = split_slash_command(&input);
            // Custom markdown / config commands (OpenCode `/commands`)
            if let Some(name) = cmd.strip_prefix('/')
                && let Some(custom) = config.commands.get(name)
            {
                let rendered = custom.render(rest);
                if !ensure_api_key(&mut api_key, &provider, &config).await {
                    continue;
                }
                println!("{}", custom_command_line(name));
                history.push_before_turn(&session.messages, &project_dir);
                refresh_session_memory(
                    &mut session,
                    &agent,
                    &project_dir,
                    &config,
                    Some(&rendered),
                );
                session.add_user_message(&rendered);
                match agent
                    .run_turn(&mut session, &provider, &model, &api_key, max_turns)
                    .await
                {
                    Ok(response) => {
                        if !response.is_empty() {
                            println!("\n{}", response);
                        }
                        println!();
                    }
                    Err(e) => eprintln!("{} {}", "Error:".red().bold(), e),
                }
                continue;
            }
            match cmd {
                "/exit" | "/quit" | "/q" => break,
                "/help" | "/h" => {
                    print_slash_help();
                    continue;
                }
                "/new" | "/clear" => {
                    history = whycodes_session::SessionHistory::new();
                    session = whycodes_session::session::Session::new(
                        project_dir.clone(),
                        with_project_memory(
                            &Agent::with_agents_md(&agent.system_prompt(), &project_dir),
                            &project_dir,
                            &config,
                            None,
                        ),
                    );
                    println!("{}", new_session_line(&session.title));
                    continue;
                }
                "/rename" => {
                    if rest.is_empty() {
                        println!(
                            "{}",
                            rename_usage_line(&session.title, session.title_source)
                        );
                    } else {
                        session.set_title_manual(rest);
                        if let Ok(db) = open_db()
                            && let Err(err) = db.update_title(&session.id, &session.title)
                        {
                            tracing::warn!(error = %err, "failed to persist session title");
                        }
                        println!("{}", renamed_line(&session.title));
                    }
                    continue;
                }
                "/info" | "/details" => {
                    let i = session.info();
                    // The provider's own counts when it reported any; the
                    // character heuristic only otherwise, and labelled as an
                    // estimate. They are different measurements and printing
                    // them the same way would suggest they are not.
                    let tokens = session_token_label(
                        session.usage.is_empty(),
                        session.token_count(),
                        session.usage.input_tokens,
                        session.usage.output_tokens,
                        session.usage.total(),
                    );
                    println!("Title: {} ({:?})", i.title.cyan(), session.title_source);
                    println!(
                        "ID: {} | Messages: {} | {} | Agent: {} | {}/{}",
                        i.id, i.message_count, tokens, agent_name, provider, model
                    );
                    if let Some(read) = session.usage.cache_read_input_tokens {
                        println!(
                            "  Cache: {} read | {} written",
                            read,
                            session.usage.cache_creation_input_tokens.unwrap_or(0)
                        );
                    }
                    println!(
                        "  Created: {} | Project: {}",
                        i.created_at.format("%Y-%m-%d %H:%M:%S"),
                        project_dir.display()
                    );
                    continue;
                }
                "/init" => {
                    match run_init_agents_md(&project_dir, &agent, &provider, &model, &api_key)
                        .await
                    {
                        Ok(path) => println!("{}", init_wrote_line(&path)),
                        Err(e) => eprintln!("{}", init_failed_line(&e.to_string())),
                    }
                    // Reload system prompt with new AGENTS.md + memory
                    session.set_system_prompt(&with_project_memory(
                        &Agent::with_agents_md(
                            &Agent::system_prompt_for(&agent_name),
                            &project_dir,
                        ),
                        &project_dir,
                        &config,
                        None,
                    ));
                    continue;
                }
                "/undo" => {
                    if let Some(msgs) = history.undo(&session.messages, &project_dir) {
                        session.set_messages(msgs);
                        println!("{}", undid_turn_line(session.messages.len()));
                    } else if session.undo_last_turn() > 0 {
                        println!("{}", undid_turn_line(session.messages.len()));
                    } else {
                        println!("{}", nothing_to_undo_line());
                    }
                    continue;
                }
                "/redo" => {
                    if let Some(msgs) = history.redo(&session.messages, &project_dir) {
                        session.set_messages(msgs);
                        println!("{}", redid_turn_line(session.messages.len()));
                    } else {
                        println!("{}", nothing_to_redo_line());
                    }
                    continue;
                }
                "/share" | "/export" => {
                    match session.export_share() {
                        Ok(path) => println!("{}", session_exported_line(&path)),
                        Err(e) => eprintln!("{}", export_failed_line(&e.to_string())),
                    }
                    continue;
                }
                "/fresh" => {
                    agent.skip_prompt_cache_next();
                    println!("{}", skip_prompt_cache_line());
                    continue;
                }
                "/compact" | "/summarize" => {
                    if session.messages.is_empty() {
                        println!("{}", nothing_to_compact_line());
                        continue;
                    }
                    let note = rest.trim();
                    println!("{}", compacting_line());
                    let outcome = agent
                        .compact_session(
                            &mut session,
                            &provider,
                            &model,
                            &api_key,
                            if note.is_empty() { None } else { Some(note) },
                        )
                        .await;
                    println!(
                        "{}",
                        compact_ok_line(
                            outcome.messages_before,
                            outcome.messages_after,
                            outcome.tokens_before,
                            outcome.tokens_after,
                        )
                    );
                    if let Some(last) = session.messages.last()
                        && let Some(text) = last.content.as_text()
                        && whycodes_session::is_compact_summary_text(text)
                    {
                        println!("{}", whycodes_session::compact_summary_display_text(text));
                    }
                    continue;
                }
                "/diff" => {
                    let status = std::process::Command::new("git")
                        .args(["status", "--short", "--branch"])
                        .current_dir(&project_dir)
                        .output();
                    match status {
                        Ok(o) if o.status.success() => {
                            println!("{}", "Diff".bold());
                            print!("{}", String::from_utf8_lossy(&o.stdout));
                            if let Ok(d) = std::process::Command::new("git")
                                .args(["diff", "--stat", "HEAD"])
                                .current_dir(&project_dir)
                                .output()
                            {
                                let s = String::from_utf8_lossy(&d.stdout);
                                if !s.trim().is_empty() {
                                    println!("{}", s);
                                }
                            }
                        }
                        Ok(o) => eprintln!(
                            "{}",
                            git_status_failed_line(String::from_utf8_lossy(&o.stderr).trim())
                        ),
                        Err(e) => eprintln!("{}", git_unavailable_line(&e.to_string())),
                    }
                    continue;
                }
                "/cost" | "/usage" => {
                    let u = &session.usage;
                    println!("{}", "Cost / usage".bold());
                    println!(
                        "{}",
                        session_cost_line(
                            u.is_empty(),
                            session.token_count(),
                            u.input_tokens,
                            u.output_tokens,
                            u.total(),
                        )
                    );
                    continue;
                }
                "/context" => {
                    for line in context_report_lines(
                        session.messages.len(),
                        session.token_count(),
                        config.session.compaction_threshold,
                        &config.session.compaction_llm,
                        &config.session.tool_profile,
                    ) {
                        println!("{line}");
                    }
                    continue;
                }
                "/doctor" => {
                    let key_ok = !api_key.is_empty()
                        || !whycodes_llm::provider_requires_api_key(&provider, Some(&config));
                    for line in doctor_report_lines(
                        &provider,
                        &model,
                        &project_dir.display().to_string(),
                        doctor_api_key_status(key_ok, api_key.is_empty()),
                        &config.security.sandbox,
                        config.security.sandbox_network,
                        &config.session.tool_profile,
                    ) {
                        println!("{line}");
                    }
                    continue;
                }
                "/sessions" => {
                    if let Err(err) = super::session::cmd_session(&SessionCmd::List).await {
                        eprintln!("{} {}", "✗".red(), err);
                    }
                    continue;
                }
                "/resume" | "/continue" => {
                    let want = match resume_slash_want(cmd, rest) {
                        ResumeSlash::Id(id) => id,
                        ResumeSlash::List => {
                            // /resume with no id → list, same as /sessions
                            if let Err(err) = super::session::cmd_session(&SessionCmd::List).await {
                                eprintln!("{} {}", "✗".red(), err);
                            }
                            println!("{}", "Tip: /resume <id> or /continue (latest)".dimmed());
                            continue;
                        }
                    };
                    match resume_session_into(&mut session, &want) {
                        Ok(true) => {
                            history = whycodes_session::SessionHistory::new();
                            println!(
                                "{}",
                                resumed_line(&session.title, &session.id, session.messages.len())
                            );
                        }
                        Ok(false) => {
                            eprintln!("{} Session not found.", "✗".red());
                        }
                        Err(e) => eprintln!("{} Resume failed: {e}", "✗".red()),
                    }
                    continue;
                }
                "/models" => {
                    let _ = super::provider::cmd_model(&ModelCmd::List).await;
                    println!("Current: {}/{}", provider.cyan(), model.cyan());
                    if !rest.is_empty() {
                        match parse_models_slash(rest) {
                            ModelsSlash::ProviderModel(p, m) => {
                                whycodes_llm::oauth_refresh::unregister(&provider);
                                provider = p;
                                model = m;
                                api_key = get_api_key(&provider, &config).await.unwrap_or_default();
                                println!("{}", switched_model_line(&provider, &model));
                                maybe_inject_test_llm(&mut agent, &provider);
                            }
                            ModelsSlash::ModelOnly(m) => {
                                model = m;
                                println!("{}", model_set_line(&model));
                            }
                        }
                    }
                    continue;
                }
                "/effort" => {
                    match parse_effort_slash(rest) {
                        EffortSlash::Show => {
                            let current = config
                                .session
                                .reasoning_effort
                                .as_deref()
                                .unwrap_or("medium (default)");
                            println!("Reasoning effort: {}", current.cyan());
                            println!("Set with /effort low|medium|high|xhigh");
                        }
                        EffortSlash::Set(parsed) => {
                            let resolved = whycodes_llm::ThinkingConfig::resolve_effort(
                                &provider,
                                &model,
                                Some(parsed.as_str()),
                            );
                            match resolved {
                                Some(level) => {
                                    let value = level.as_str().to_string();
                                    config.session.reasoning_effort = Some(value.clone());
                                    agent.set_reasoning_effort(Some(value.clone()));
                                    if let Err(e) = config.save() {
                                        eprintln!("{} Could not persist: {e}", "✗".red());
                                    }
                                    println!(
                                        "{} Reasoning effort → {}",
                                        "✓".green(),
                                        level.label().cyan()
                                    );
                                }
                                None => {
                                    println!("{}", effort_no_levels_line());
                                }
                            }
                        }
                        EffortSlash::Unknown => {
                            eprintln!("{}", effort_unknown_line(rest));
                        }
                    }
                    continue;
                }
                "/agent" | "/agents" => {
                    if rest.is_empty() {
                        let _ = super::provider::cmd_agent(None).await;
                        println!("Current agent: {}", agent_name.cyan());
                    } else {
                        match switch_agent(rest, &config, &project_dir) {
                            Ok((name, new_agent, prompt)) => {
                                agent_name = name;
                                agent = new_agent;
                                maybe_inject_test_llm(&mut agent, &provider);
                                session.set_system_prompt(&prompt);
                                println!("{}", switched_agent_line(&agent_name));
                            }
                            Err(e) => eprintln!("{} {}", "✗".red(), e),
                        }
                    }
                    continue;
                }
                "/connect" => {
                    // Re-load config + env in case user set a key in another shell
                    if let Ok(cfg) = Config::load() {
                        config = cfg;
                    }
                    if let Some(k) = get_api_key(&provider, &config).await {
                        api_key = k;
                        println!(
                            "{}",
                            api_key_loaded_line(&provider, &masked_api_key_prefix(&api_key))
                        );
                    } else {
                        for line in connect_missing_key_lines(
                            &provider,
                            whycodes_auth::providers::supports_oauth(&provider),
                        ) {
                            println!("{line}");
                        }
                        let _ = super::provider::cmd_provider(&ProviderCmd::List).await;
                    }
                    continue;
                }
                "/login" => {
                    let arg = rest.trim();
                    if arg.is_empty() {
                        println!("{}", "Subscription sign-in (OAuth):".bold());
                        if let Ok(dir) = Config::data_dir() {
                            let store = whycodes_auth::TokenStore::new(&dir);
                            for name in whycodes_auth::oauth_providers() {
                                let label = whycodes_auth::providers::spec_for(&name)
                                    .map(|s| s.label)
                                    .unwrap_or_else(|_| name.clone());
                                let status = login_connected_label(
                                    store.get(&name).ok().flatten().is_some(),
                                );
                                println!(
                                    "  {} {} — {}",
                                    format!("{name:<15}").cyan(),
                                    label,
                                    status
                                );
                            }
                        }
                        println!(
                            "\nSign in: {}  ·  CLI: {}",
                            "/login <provider>".cyan(),
                            "whycodes auth login <provider>".cyan()
                        );
                    } else if whycodes_auth::providers::supports_oauth(arg) {
                        if let Err(e) = super::auth::cmd_auth(&AuthCmd::Login {
                            provider: arg.to_string(),
                            no_browser: false,
                        })
                        .await
                        {
                            eprintln!("{} {e}", "sign-in failed:".red());
                        }
                        if arg == provider.as_str()
                            && let Some(k) = get_api_key(&provider, &config).await
                        {
                            api_key = k;
                        }
                    } else {
                        println!("{}", oauth_unavailable_line(arg, &oauth_provider_list()));
                    }
                    continue;
                }
                "/thinking" => {
                    show_thinking = !show_thinking;
                    println!(
                        "Thinking display: {}",
                        thinking_display_label(show_thinking)
                    );
                    continue;
                }
                "/themes" => {
                    let names: Vec<&str> = whycodes_tui::theme::ThemeName::ALL
                        .iter()
                        .map(|t| t.name())
                        .collect();
                    println!("{} Themes (TUI), {}:", "🎨".bold(), names.len());
                    println!("  {}", names.join(", "));
                    println!("{}", themes_set_hint(names[0]));
                    continue;
                }
                "/tools" => {
                    let tools =
                        whycodes_tools::ToolExecutor::new().get_definitions(&agent.info.permission);
                    println!("{}", tools_list_header(tools.len()));
                    for t in tools.iter() {
                        println!("  {} — {}", t.name.cyan(), t.description);
                    }
                    continue;
                }
                "/remember" => {
                    if rest.is_empty() {
                        println!("Usage: /remember <text to store>");
                    } else {
                        match whycodes_memory::MemoryService::open(
                            &project_dir,
                            Config::data_dir().unwrap_or_else(|_| PathBuf::from(".")),
                            memory_settings(&config),
                        ) {
                            Ok(svc) => match svc.remember(rest, Some(&session.id)) {
                                Ok(id) => println!("{}", remembered_line(&id, rest)),
                                Err(e) => eprintln!("{} {e}", "✗".red()),
                            },
                            Err(e) => eprintln!("{} {e}", "✗".red()),
                        }
                    }
                    continue;
                }
                "/memory" => {
                    match whycodes_memory::MemoryService::open(
                        &project_dir,
                        Config::data_dir().unwrap_or_else(|_| PathBuf::from(".")),
                        memory_settings(&config),
                    ) {
                        Ok(svc) => {
                            let n = svc.list(1000).map(|r| r.len()).unwrap_or(0);
                            println!(
                                "{}",
                                repl_memory_status(
                                    config.memory.enabled,
                                    n,
                                    svc.memory_md_path().display()
                                )
                            );
                            println!("  project_key={}", svc.project_key.dimmed());
                            println!("  CLI: whycodes memory list|search|add|delete|clear");
                            if let Ok(rows) = svc.list(10) {
                                for r in rows {
                                    println!(
                                        "  · {}  {}",
                                        r.id.chars().take(8).collect::<String>().dimmed(),
                                        r.text
                                    );
                                }
                            }
                        }
                        Err(e) => eprintln!("{} {e}", "✗".red()),
                    }
                    continue;
                }
                other => {
                    println!("{}", unknown_slash_line(other));
                    continue;
                }
            }
        }

        // Expand @file references (OpenCode parity)
        let expanded = expand_user_input(&input, &project_dir);

        if !ensure_api_key(&mut api_key, &provider, &config).await {
            continue;
        }

        history.push_before_turn(&session.messages, &project_dir);
        refresh_session_memory(&mut session, &agent, &project_dir, &config, Some(&expanded));
        session.add_user_message(&expanded);
        if config.session.auto_title {
            let seed = session
                .first_user_text()
                .unwrap_or_else(|| expanded.clone());
            // bool: whether the title changed (not a Result).
            session.apply_heuristic_title(&seed);
        }
        match agent
            .run_turn(&mut session, &provider, &model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if config.session.auto_title {
                    agent
                        .maybe_refine_title(
                            &mut session,
                            &provider,
                            &model,
                            &api_key,
                            config.session.title_model.as_deref(),
                        )
                        .await;
                }
                if !response.is_empty() {
                    println!("\n{}", response);
                }
                // Retain is spawned inside Agent::run_turn (async; best-effort).
                println!();
                // Persist session best-effort (success)
                if let Ok(db) = open_db() {
                    if let Err(err) = session.save_to_db(&db) {
                        tracing::warn!(error = %err, "failed to persist session");
                    } else {
                        whycodes_core::logging::emit_sid(
                            "session",
                            "info",
                            "session.persist",
                            Some(session.id.as_str()),
                            Some(serde_json::json!({
                                "reason": "ok",
                                "messages": session.messages.len(),
                                "title": session.title,
                            })),
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                whycodes_core::logging::emit_sid(
                    "cli",
                    "error",
                    "turn.error",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({ "error": e.to_string() })),
                );
                // Persist even on error so a crash mid-debug still has history.
                if let Ok(db) = open_db() {
                    let _ = session.save_to_db(&db);
                }
            }
        }
    }
    // Final flush + Cline-style summary (same shape as the TUI exit path).
    if let Ok(db) = open_db() {
        let _ = session.save_to_db(&db);
    }
    let model_label = format!("{provider}/{model}");
    print!(
        "{}",
        session.format_exit_summary(session_started.elapsed(), &model_label, "whycodes")
    );
    Ok(())
}

pub(crate) async fn run_init_agents_md(
    project_dir: &std::path::Path,
    agent: &Agent,
    provider: &str,
    model: &str,
    api_key: &str,
) -> anyhow::Result<String> {
    let agents_path = project_dir.join("AGENTS.md");
    let existing = std::fs::read_to_string(&agents_path).unwrap_or_default();

    // Quick project snapshot for the prompt
    let mut snapshot = String::new();
    if let Ok(entries) = std::fs::read_dir(project_dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        snapshot.push_str("Top-level entries:\n");
        for n in names.iter().take(40) {
            snapshot.push_str(&format!("- {}\n", n));
        }
    }
    for marker in [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "README.md",
    ] {
        let p = project_dir.join(marker);
        if let Ok(c) = std::fs::read_to_string(&p) {
            let preview: String = c.chars().take(2000).collect();
            snapshot.push_str(&format!("\n## {}\n```\n{}\n```\n", marker, preview));
        }
    }

    let prompt = format!(
        "Create or update an AGENTS.md file for this project. \
         AGENTS.md gives coding agents project-specific instructions \
         (build/test commands, conventions, architecture notes).\n\n\
         Project path: {}\n\n{}\n\n\
         Existing AGENTS.md (may be empty):\n```\n{}\n```\n\n\
         Write a complete AGENTS.md in Markdown. Output ONLY the file contents, no fence.",
        project_dir.display(),
        snapshot,
        existing
    );

    let mut tmp = whycodes_session::session::Session::new(
        project_dir.to_path_buf(),
        "You write clear AGENTS.md project instruction files.".to_string(),
    );
    tmp.add_user_message(&prompt);
    let content = agent
        .run_turn(&mut tmp, provider, model, api_key, Some(5))
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let content = strip_agents_fence(&content);

    if content.is_empty() {
        anyhow::bail!("Model returned empty AGENTS.md");
    }

    std::fs::write(&agents_path, format!("{}\n", content))?;
    Ok(agents_path.display().to_string())
}

/// `generate` — Non-interactive code generation (supports `--format` for CI).
pub(crate) async fn cmd_generate(
    cli: &Cli,
    prompts: &[String],
    max_turns: Option<usize>,
    jobs: usize,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);

    let api_key = match get_api_key(&provider, &config).await {
        Some(k) => k,
        None if !whycodes_llm::provider_requires_api_key(&provider, Some(&config)) => String::new(),
        None => {
            return emit_headless_setup_error(
                format,
                &missing_api_key_message_for(&provider, Some(&config)),
            );
        }
    };

    if all_prompts_empty(prompts) {
        return emit_headless_setup_error(format, "empty prompt");
    }

    // S5: parallel fan-out. Each prompt gets its own Agent + Session; a
    // semaphore caps concurrency at `jobs`. Per-prompt failures never abort
    // siblings; the process exits non-zero if any prompt failed.
    if should_fan_out(prompts) {
        let mut agent_info = agent_info_for(cli, &config);
        agent_info.permission = config.effective_permission(&agent_info.permission);
        return run_generate_parallel(
            prompts,
            &config,
            agent_info,
            &provider,
            &model,
            &agent_name,
            &api_key,
            max_turns,
            jobs.max(1),
            format,
            &project_dir,
        )
        .await;
    }

    let prompt = &prompts[0];

    let mut agent_info = agent_info_for(cli, &config);
    agent_info.permission = config.effective_permission(&agent_info.permission);
    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));
    let expanded = expand_user_input(prompt, &project_dir);
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base_prompt, &project_dir),
        &project_dir,
        &config,
        Some(&expanded),
    );

    // Structured CI formats cannot prompt on stdin; auto-approve tool asks.
    // Catastrophic shell risk still hard-blocks regardless of this.
    let file_index = whycodes_index::WorkspaceIndex::start(
        whycodes_index::WorkspaceIndex::project_roots(&project_dir),
    );
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index)
        .with_mcp(&config)
        .await;
    maybe_inject_test_llm(&mut agent, &provider);
    if format.is_structured() {
        agent = agent
            .with_permission_prompter(Arc::new(AutoApprovePrompter))
            .with_question_prompter(Arc::new(whycodes_agent::AutoAnswerPrompter));
    }

    let mut session = whycodes_session::session::Session::new(project_dir.clone(), system_prompt);

    if format == OutputFormat::Text {
        println!(
            "{} Generating with {}/{}...",
            "⚡".bold(),
            provider.dimmed(),
            model.dimmed()
        );
    }

    session.add_user_message(&expanded);

    run_headless_turn(
        &agent,
        &mut session,
        &provider,
        &model,
        &api_key,
        &agent_name,
        max_turns,
        format,
    )
    .await
}

pub(crate) fn all_prompts_empty(prompts: &[String]) -> bool {
    prompts.iter().all(|p| p.is_empty())
}

pub(crate) fn should_fan_out(prompts: &[String]) -> bool {
    prompts.len() > 1
}

/// S5: run N prompts concurrently, each with its own Agent + Session.
///
/// A semaphore caps in-flight turns at `jobs`. Every prompt always gets a
/// final envelope: `Result` (ok or is_error) for json/stream-json, plain
/// text or an error line for text. One prompt's failure never aborts the
/// others; the process returns Err if any prompt failed.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_generate_parallel(
    prompts: &[String],
    config: &Config,
    agent_info: AgentInfo,
    provider: &str,
    model: &str,
    agent_name: &str,
    api_key: &str,
    max_turns: Option<usize>,
    jobs: usize,
    format: OutputFormat,
    project_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let sem = Arc::new(tokio::sync::Semaphore::new(jobs));
    let structured = format.is_structured();
    let mut handles = Vec::new();

    for prompt in prompts {
        if prompt.is_empty() {
            continue;
        }
        let sem = Arc::clone(&sem);
        let config = config.clone();
        let agent_info = agent_info.clone();
        let provider = provider.to_string();
        let model = model.to_string();
        let agent_name = agent_name.to_string();
        let api_key = api_key.to_string();
        let prompt = prompt.clone();
        let project_dir = project_dir.to_path_buf();

        handles.push(tokio::spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return true;
            };
            run_one_parallel_turn(
                &prompt,
                &config,
                agent_info,
                &provider,
                &model,
                &agent_name,
                &api_key,
                max_turns,
                format,
                &project_dir,
                structured,
            )
            .await
        }));
    }

    let mut outcomes = Vec::new();
    for h in handles {
        outcomes.push(match h.await {
            Ok(failed) => Ok(failed),
            Err(e) => Err(format!("worker panicked: {e}")),
        });
    }
    fold_parallel_joins(outcomes, structured)
}

pub(crate) fn fold_parallel_joins(
    outcomes: impl IntoIterator<Item = Result<bool, String>>,
    structured: bool,
) -> anyhow::Result<()> {
    let mut any_failed = false;
    for outcome in outcomes {
        match outcome {
            Ok(false) => {}
            Ok(true) => any_failed = true,
            Err(msg) => {
                any_failed = true;
                if structured {
                    let _ = CiEvent::Error { message: msg }.emit_stdout();
                } else {
                    eprintln!("{} {}", "Error:".red().bold(), msg);
                }
            }
        }
    }
    if any_failed {
        Err(anyhow::anyhow!("one or more prompts failed"))
    } else {
        Ok(())
    }
}

/// One prompt inside the parallel fan-out. Returns whether it failed.
/// Stdout writes are serialized inside (CiEvent locks stdout per line).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_one_parallel_turn(
    prompt: &str,
    config: &Config,
    agent_info: AgentInfo,
    provider: &str,
    model: &str,
    agent_name: &str,
    api_key: &str,
    max_turns: Option<usize>,
    format: OutputFormat,
    project_dir: &std::path::Path,
    structured: bool,
) -> bool {
    let started = std::time::Instant::now();

    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(agent_name));
    let expanded = expand_user_input(prompt, project_dir);
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base_prompt, project_dir),
        project_dir,
        config,
        Some(&expanded),
    );

    let file_index = whycodes_index::WorkspaceIndex::start(
        whycodes_index::WorkspaceIndex::project_roots(project_dir),
    );
    let mut agent = Agent::new(agent_info)
        .with_config(config)
        .with_file_index(file_index)
        .with_mcp(config)
        .await;
    maybe_inject_test_llm(&mut agent, provider);
    if structured {
        agent = agent
            .with_permission_prompter(Arc::new(AutoApprovePrompter))
            .with_question_prompter(Arc::new(whycodes_agent::AutoAnswerPrompter));
    }

    let mut session =
        whycodes_session::session::Session::new(project_dir.to_path_buf(), system_prompt);
    let session_id = session.id.clone();
    session.add_user_message(&expanded);

    let wrap = |ev: CiEvent| CiEvent::Session {
        session_id: session_id.clone(),
        event: Box::new(ev),
    };

    if format == OutputFormat::StreamJson {
        let _ = wrap(CiEvent::Init {
            session_id: session_id.clone(),
            provider: provider.to_string(),
            model: model.to_string(),
            agent: agent_name.to_string(),
            cwd: project_dir.display().to_string(),
        })
        .emit_stdout();
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let cancel = new_cancel_flag();
    let stream = format == OutputFormat::StreamJson;
    let sid = session_id.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if !stream {
                continue;
            }
            if let Some(ci) = turn_event_to_ci(ev) {
                let _ = CiEvent::Session {
                    session_id: sid.clone(),
                    event: Box::new(ci),
                }
                .emit_stdout();
            }
        }
    });

    let turn_result = agent
        .run_turn_with_events(
            &mut session,
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
    let _ = drain.await;

    let meta = ResultMeta {
        session_id: session.id.clone(),
        provider: provider.to_string(),
        model: model.to_string(),
        agent: agent_name.to_string(),
        usage: session.usage.clone(),
        duration_ms: started.elapsed().as_millis() as u64,
    };

    emit_parallel_outcome(format, turn_result.map_err(|e| e.to_string()), meta, &wrap)
}

pub(crate) fn emit_parallel_outcome(
    format: OutputFormat,
    result: Result<String, String>,
    meta: ResultMeta,
    wrap: &impl Fn(CiEvent) -> CiEvent,
) -> bool {
    match result {
        Ok(response) => {
            match format {
                OutputFormat::Text => {
                    if !response.is_empty() {
                        println!("{response}");
                    }
                }
                OutputFormat::Json => {
                    let _ = meta.ok(response).emit_stdout();
                }
                OutputFormat::StreamJson => {
                    let _ = wrap(meta.ok(response)).emit_stdout();
                }
            }
            false
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
                        let _ = wrap(CiEvent::Cancelled).emit_stdout();
                    } else {
                        let _ = wrap(CiEvent::Error {
                            message: msg.clone(),
                        })
                        .emit_stdout();
                    }
                    let _ = wrap(meta.err(&msg)).emit_stdout();
                }
            }
            true
        }
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
