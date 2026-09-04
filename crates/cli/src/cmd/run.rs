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
    let mut config = Config::load_layered(&project_dir_early)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);
    let project_dir = resolve_dir(cli);
    config.load_command_files(&project_dir);

    // Full-screen TUI unless --plain / WHYCODES_PLAIN.
    // Hosts that capture stdout (IDE, some wrappers) report stdout_tty=false
    // while still having a controlling terminal — tui_available() opens
    // /dev/tty in that case so the TUI still works.
    let force_plain = cli.plain || std::env::var_os("WHYCODES_PLAIN").is_some();
    let stub_tui = cfg!(test) && std::env::var_os("WHYCODES_TEST_TUI").is_some();
    let use_tui = !force_plain && (stub_tui || whycodes_tui::tui_available());
    let interactive = prompt.is_none_or(str::is_empty) && !format.is_structured();
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
             (stdin_tty={} stdout_tty={} /dev/tty unavailable).\n\
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
            update_rx,
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
                    if want == whycodes_tui::RESUME_LATEST {
                        "none saved yet"
                    } else {
                        want.as_str()
                    }
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
                println!("Usage: ! <shell command>");
                continue;
            }
            println!("{} {}", "$".dimmed(), cmd.dimmed());
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
                println!("{} /{} → prompt", "⚡".bold(), name.cyan());
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
                    println!(
                        "{} New session started ({})",
                        "✓".green(),
                        session.title.dimmed()
                    );
                    continue;
                }
                "/rename" => {
                    if rest.is_empty() {
                        println!(
                            "Title: {} ({:?}) — usage: /rename <name>",
                            session.title.cyan(),
                            session.title_source
                        );
                    } else {
                        session.set_title_manual(rest);
                        if let Ok(db) = open_db()
                            && let Err(err) = db.update_title(&session.id, &session.title)
                        {
                            tracing::warn!(error = %err, "failed to persist session title");
                        }
                        println!("{} Renamed to '{}'", "✓".green(), session.title.cyan());
                    }
                    continue;
                }
                "/info" | "/details" => {
                    let i = session.info();
                    // The provider's own counts when it reported any; the
                    // character heuristic only otherwise, and labelled as an
                    // estimate. They are different measurements and printing
                    // them the same way would suggest they are not.
                    let tokens = if session.usage.is_empty() {
                        format!("Tokens≈{} (est)", session.token_count())
                    } else {
                        format!(
                            "Tokens: {} in / {} out / {} total",
                            session.usage.input_tokens,
                            session.usage.output_tokens,
                            session.usage.total()
                        )
                    };
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
                        Ok(path) => println!(
                            "{} Wrote project instructions: {}",
                            "✓".green(),
                            path.cyan()
                        ),
                        Err(e) => eprintln!("{} /init failed: {}", "✗".red(), e),
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
                        println!(
                            "{} Undid last turn ({} messages left).",
                            "↩".cyan(),
                            session.messages.len()
                        );
                    } else if session.undo_last_turn() > 0 {
                        println!(
                            "{} Undid last turn ({} messages left).",
                            "↩".cyan(),
                            session.messages.len()
                        );
                    } else {
                        println!("{} Nothing to undo.", "ℹ".cyan());
                    }
                    continue;
                }
                "/redo" => {
                    if let Some(msgs) = history.redo(&session.messages, &project_dir) {
                        session.set_messages(msgs);
                        println!(
                            "{} Redid turn ({} messages).",
                            "↪".cyan(),
                            session.messages.len()
                        );
                    } else {
                        println!("{} Nothing to redo.", "ℹ".cyan());
                    }
                    continue;
                }
                "/share" | "/export" => {
                    match session.export_share() {
                        Ok(path) => println!("{} Session exported: {}", "✓".green(), path.cyan()),
                        Err(e) => eprintln!("{} Export failed: {}", "✗".red(), e),
                    }
                    continue;
                }
                "/fresh" => {
                    agent.skip_prompt_cache_next();
                    println!(
                        "{} Next turn will skip the provider prompt cache.",
                        "✓".green()
                    );
                    continue;
                }
                "/compact" | "/summarize" => {
                    if session.messages.is_empty() {
                        println!("{} Nothing to compact.", "ℹ".cyan());
                        continue;
                    }
                    let note = rest.trim();
                    println!("{} Compacting conversation…", "…".dimmed());
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
                        "{} Conversation compacted ({} → {} messages, ~{} → ~{} tok).",
                        "✓".green(),
                        outcome.messages_before,
                        outcome.messages_after,
                        outcome.tokens_before,
                        outcome.tokens_after
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
                            "{} git status: {}",
                            "✗".red(),
                            String::from_utf8_lossy(&o.stderr).trim()
                        ),
                        Err(e) => eprintln!("{} git unavailable: {}", "✗".red(), e),
                    }
                    continue;
                }
                "/cost" | "/usage" => {
                    let u = &session.usage;
                    println!("{}", "Cost / usage".bold());
                    if u.is_empty() {
                        println!("  session: ~{} tokens (estimated)", session.token_count());
                    } else {
                        println!(
                            "  session: {} in / {} out · total {}",
                            u.input_tokens,
                            u.output_tokens,
                            u.total()
                        );
                    }
                    continue;
                }
                "/context" => {
                    println!("{}", "Context".bold());
                    println!("  messages: {}", session.messages.len());
                    println!("  estimate: ~{} tok", session.token_count());
                    println!(
                        "  compact:  threshold={} llm={}",
                        config.session.compaction_threshold, config.session.compaction_llm
                    );
                    println!("  tools:    profile={}", config.session.tool_profile);
                    continue;
                }
                "/doctor" => {
                    println!("{}", "Doctor".bold());
                    println!("  provider: {provider}");
                    println!("  model:    {model}");
                    println!("  project:  {}", project_dir.display());
                    let key_ok = !api_key.is_empty()
                        || !whycodes_llm::provider_requires_api_key(&provider, Some(&config));
                    println!(
                        "  api_key:  {}",
                        if key_ok {
                            if api_key.is_empty() {
                                "not required"
                            } else {
                                "set"
                            }
                        } else {
                            "MISSING"
                        }
                    );
                    println!(
                        "  sandbox:  {} network={}",
                        config.security.sandbox, config.security.sandbox_network
                    );
                    println!("  tools:    profile={}", config.session.tool_profile);
                    continue;
                }
                "/sessions" => {
                    if let Err(err) = super::session::cmd_session(&SessionCmd::List).await {
                        eprintln!("{} {}", "✗".red(), err);
                    }
                    continue;
                }
                "/resume" | "/continue" => {
                    let want = if !rest.is_empty() {
                        rest.to_string()
                    } else if cmd == "/continue" {
                        whycodes_tui::RESUME_LATEST.to_string()
                    } else {
                        // /resume with no id → list, same as /sessions
                        if let Err(err) = super::session::cmd_session(&SessionCmd::List).await {
                            eprintln!("{} {}", "✗".red(), err);
                        }
                        println!("{}", "Tip: /resume <id> or /continue (latest)".dimmed());
                        continue;
                    };
                    match resume_session_into(&mut session, &want) {
                        Ok(true) => {
                            history = whycodes_session::SessionHistory::new();
                            println!(
                                "{} Resumed {} ({}) — {} messages",
                                "✓".green(),
                                session.title.cyan(),
                                session.id.chars().take(8).collect::<String>().dimmed(),
                                session.messages.len()
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
                        // /models provider/model
                        if let Some((p, m)) = rest.split_once('/') {
                            whycodes_llm::oauth_refresh::unregister(&provider);
                            provider = p.to_string();
                            model = m.to_string();
                            api_key = get_api_key(&provider, &config).await.unwrap_or_default();
                            println!(
                                "{} Switched model to {}/{}",
                                "✓".green(),
                                provider.cyan(),
                                model.cyan()
                            );
                            maybe_inject_test_llm(&mut agent, &provider);
                        } else {
                            model = rest.to_string();
                            println!("{} Model set to {}", "✓".green(), model.cyan());
                        }
                    }
                    continue;
                }
                "/effort" => {
                    if rest.is_empty() {
                        let current = config
                            .session
                            .reasoning_effort
                            .as_deref()
                            .unwrap_or("medium (default)");
                        println!("Reasoning effort: {}", current.cyan());
                        println!("Set with /effort low|medium|high|xhigh");
                    } else if let Some(parsed) = whycodes_llm::ReasoningEffort::parse(rest) {
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
                                println!(
                                    "{} This model has no reasoning-effort levels",
                                    "·".dimmed()
                                );
                            }
                        }
                    } else {
                        eprintln!(
                            "{} Unknown effort '{}' (low, medium, high, xhigh)",
                            "✗".red(),
                            rest
                        );
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
                                println!(
                                    "{} Switched to agent '{}'",
                                    "✓".green(),
                                    agent_name.cyan()
                                );
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
                            "{} API key loaded for {} ({}…)",
                            "✓".green(),
                            provider.cyan(),
                            api_key.chars().take(8).collect::<String>()
                        );
                    } else {
                        println!("Add a provider:");
                        println!("  whycodes provider add {} --api-key <key>", provider);
                        println!("  or set env {}", provider_env_var(&provider));
                        if whycodes_auth::providers::supports_oauth(&provider) {
                            println!(
                                "  or log in with your subscription: whycodes auth login {}",
                                provider
                            );
                        }
                        println!();
                        println!(
                            "Env vars: ANTHROPIC_API_KEY, OPENAI_API_KEY, XAI_API_KEY, GOOGLE_API_KEY, ..."
                        );
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
                                let status = if store.get(&name).ok().flatten().is_some() {
                                    "connected".green()
                                } else {
                                    "not connected".dimmed()
                                };
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
                        println!(
                            "OAuth login is not available for `{}` — choose from: {}",
                            arg.red(),
                            oauth_provider_list()
                        );
                    }
                    continue;
                }
                "/thinking" => {
                    show_thinking = !show_thinking;
                    println!(
                        "Thinking display: {}",
                        if show_thinking {
                            "ON".green().to_string()
                        } else {
                            "OFF".dimmed().to_string()
                        }
                    );
                    let _ = show_thinking; // reserved for TUI streaming
                    continue;
                }
                "/themes" => {
                    let names: Vec<&str> = whycodes_tui::theme::ThemeName::ALL
                        .iter()
                        .map(|t| t.name())
                        .collect();
                    println!("{} Themes (TUI), {}:", "🎨".bold(), names.len());
                    println!("  {}", names.join(", "));
                    println!("Set in config: [tui] theme = \"{}\"", names[0]);
                    continue;
                }
                "/tools" => {
                    let tools =
                        whycodes_tools::ToolExecutor::new().get_definitions(&agent.info.permission);
                    println!("{} Available tools ({}):", "🔧".bold(), tools.len());
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
                                Ok(id) => println!(
                                    "{} Remembered {} — {}",
                                    "✓".green(),
                                    id.chars().take(8).collect::<String>().cyan(),
                                    rest
                                ),
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
                                "Memory: enabled={}  entries={}  path={}",
                                config.memory.enabled,
                                n,
                                svc.memory_md_path().display()
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
                    println!("Unknown command: {}. Type /help", other);
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
mod tests {
    use super::*;

    #[test]
    fn fold_parallel_joins_ok_and_fail() {
        fold_parallel_joins([Ok(false)], false).unwrap();
        assert!(fold_parallel_joins([Ok(true)], false).is_err());
        assert!(fold_parallel_joins([Err("x".into())], true).is_err());
        assert!(fold_parallel_joins([Err("x".into())], false).is_err());
    }

    #[test]
    fn emit_parallel_outcome_covers_formats() {
        let meta = ResultMeta {
            session_id: "s".into(),
            provider: "p".into(),
            model: "m".into(),
            agent: "a".into(),
            usage: Default::default(),
            duration_ms: 1,
        };
        let wrap = |ev: CiEvent| ev;
        assert!(!emit_parallel_outcome(
            OutputFormat::Text,
            Ok("ok".into()),
            meta.clone(),
            &wrap
        ));
        assert!(!emit_parallel_outcome(
            OutputFormat::Text,
            Ok(String::new()),
            meta.clone(),
            &wrap
        ));
        assert!(!emit_parallel_outcome(
            OutputFormat::Json,
            Ok("j".into()),
            meta.clone(),
            &wrap
        ));
        assert!(!emit_parallel_outcome(
            OutputFormat::StreamJson,
            Ok("s".into()),
            meta.clone(),
            &wrap
        ));
        assert!(emit_parallel_outcome(
            OutputFormat::Text,
            Err("boom".into()),
            meta.clone(),
            &wrap
        ));
        assert!(emit_parallel_outcome(
            OutputFormat::Json,
            Err("jerr".into()),
            meta.clone(),
            &wrap
        ));
        assert!(emit_parallel_outcome(
            OutputFormat::StreamJson,
            Err("cancel me".into()),
            meta.clone(),
            &wrap
        ));
        assert!(emit_parallel_outcome(
            OutputFormat::StreamJson,
            Err("provider down".into()),
            meta,
            &wrap
        ));
    }

    #[test]
    fn helpers_all_prompts_and_fan_out() {
        assert!(all_prompts_empty(&[String::new(), String::new()]));
        assert!(!all_prompts_empty(&["x".into()]));
        assert!(!should_fan_out(&["a".into()]));
        assert!(should_fan_out(&["a".into(), "b".into()]));
        let mapped = map_tui_run_error(anyhow::anyhow!("os error 6"));
        assert!(mapped.to_string().contains("TUI needs a real terminal"));
    }
}
