//! TUI event loop — streaming agent + permission dialogs (OpenCode-style).

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    size as term_size, supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use whycode_agent::agent::Agent;
use whycode_agent::permission::ChannelPermissionPrompter;
use whycode_agent::{CancelFlag, TurnEvent, new_cancel_flag, request_cancel};
use whycode_core::config::Config;
use whycode_core::types::AgentMode;
use whycode_session::SessionHistory;
use whycode_session::session::Session;

use crate::app::{
    AgentState, AppMode, ChatRole, DialogKind, TuiApp, format_elapsed_ms, format_token_count,
    format_usage_short,
};
use crate::config::TuiAppConfig;
use crate::input;
use crate::keymap::KeymapContext;
use crate::ui::render;

/// Context struct for slash command handling, reducing parameter count.
pub struct SlashContext<'a> {
    pub app: &'a mut TuiApp,
    pub session: &'a mut Session,
    pub history: &'a mut SessionHistory,
    pub agent: &'a mut Agent,
    pub config: &'a Config,
    pub project_dir: &'a std::path::Path,
    pub provider: &'a mut String,
    pub model: &'a mut String,
    pub api_key: &'a mut String,
    pub perm_prompter: Arc<ChannelPermissionPrompter>,
}

/// Options for launching the interactive TUI.
pub struct TuiRunOptions {
    pub project_dir: PathBuf,
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub agent_name: String,
    pub max_turns: usize,
    pub initial_prompt: Option<String>,
    pub config: Config,
}

/// True when a full-screen TUI can attach to a real terminal.
///
/// Prefer the controlling terminal (`/dev/tty`) so IDEs/wrappers that capture
/// stdout (`stdout_tty=false`) still get a normal TUI. Falls back to stdout
/// when it is itself a TTY.
pub fn tui_available() -> bool {
    open_tui_writer().is_ok()
}

/// Concrete writer for ratatui/crossterm (`execute!` needs `Sized`).
enum TuiWriter {
    #[cfg(unix)]
    Tty(std::fs::File),
    Stdout(io::Stdout),
}

impl Write for TuiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Tty(f) => f.write(buf),
            Self::Stdout(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Tty(f) => f.flush(),
            Self::Stdout(s) => s.flush(),
        }
    }
}

/// Writer for alt-screen / draw / mouse: `/dev/tty` first, else stdout if TTY.
fn open_tui_writer() -> io::Result<TuiWriter> {
    // 1) Controlling terminal — works when stdout is piped/logged by a host.
    #[cfg(unix)]
    {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            Ok(f) => return Ok(TuiWriter::Tty(f)),
            Err(e) => {
                tracing::debug!(error = %e, "open /dev/tty failed, trying stdout");
            }
        }
    }

    // 2) Direct stdout when it is a terminal.
    let out = io::stdout();
    if out.is_terminal() {
        return Ok(TuiWriter::Stdout(out));
    }

    Err(io::Error::new(
        io::ErrorKind::NotConnected,
        "no interactive terminal (stdout is not a TTY and /dev/tty is unavailable)",
    ))
}

fn restore_terminal_on(out: &mut impl Write) {
    let _ = disable_raw_mode();
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}

enum TurnOutcome {
    Ok {
        text: String,
        agent: Agent,
        session: Session,
    },
    Err {
        error: String,
        agent: Agent,
        session: Session,
        cancelled: bool,
    },
}

/// Run the full-screen TUI until the user quits.
pub async fn run(opts: TuiRunOptions) -> anyhow::Result<()> {
    let tui_cfg = TuiAppConfig::from_core_config(&opts.config.tui);
    let mut app = TuiApp::new(tui_cfg);

    // OpenCode-style chrome
    app.provider_name = opts.provider.clone();
    app.model_name = opts.model.clone();
    app.agent_name = opts.agent_name.clone();
    app.project_dir = opts
        .project_dir
        .canonicalize()
        .unwrap_or_else(|_| opts.project_dir.clone());
    app.project_label = app
        .project_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| app.project_dir.display().to_string());
    app.refresh_git_branch();
    app.apply_context_window(
        &opts.provider,
        &opts.model,
        opts.config
            .configured_context_window(&opts.provider, &opts.model),
        opts.config.session.max_context_tokens as u64,
    );

    let mut config = opts.config.clone();
    config.load_command_files(&opts.project_dir);

    // Primary agents for Tab cycle
    app.primary_agents = config
        .agents
        .iter()
        .filter(|a| a.mode == AgentMode::Primary || a.mode == AgentMode::All)
        .map(|a| a.name.clone())
        .collect();
    if app.primary_agents.is_empty() {
        app.primary_agents = vec!["build".into(), "plan".into()];
    }
    if let Some(idx) = app
        .primary_agents
        .iter()
        .position(|n| n == &opts.agent_name)
    {
        app.agent_cycle_idx = idx;
    }

    let missing_key = opts.api_key.is_empty();
    app.status_message = if missing_key {
        format!(
            "agent={}  {}/{}  — no API key · /connect  ? help",
            opts.agent_name, opts.provider, opts.model
        )
    } else {
        format!(
            "agent={}  {}/{}  — Tab focus  Ctrl+T agent  Esc cancel  ? help",
            opts.agent_name, opts.provider, opts.model
        )
    };

    let agent_info = config
        .get_agent(&opts.agent_name)
        .cloned()
        .unwrap_or_else(|| whycode_core::types::AgentInfo {
            name: opts.agent_name.clone(),
            description: "Default".into(),
            mode: AgentMode::Primary,
            permission: whycode_core::types::PermissionSet {
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                ..whycode_core::types::PermissionSet::default()
            },
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        });

    let base = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&opts.agent_name));
    let system_prompt = Agent::with_agents_md(&base, &opts.project_dir);

    // Permission channel: agent blocks until TUI replies (shared across agent switches)
    let (perm_prompter, mut perm_rx) = ChannelPermissionPrompter::new();
    let perm_prompter: Arc<ChannelPermissionPrompter> = Arc::new(perm_prompter);

    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
        )
        .with_mcp(&config)
        .await;

    let mut session = Session::new(opts.project_dir.clone(), system_prompt);
    let mut history = SessionHistory::new();

    let mut provider = opts.provider.clone();
    let mut model = opts.model.clone();
    let mut api_key = opts.api_key.clone();
    let max_turns = opts.max_turns;
    let project_dir = opts.project_dir.clone();

    // On panic, leave alt-screen / raw mode so the shell is usable and the
    // crash report (written by whycode_core::logging) is readable.
    whycode_core::logging::set_panic_cleanup(|| {
        if let Ok(mut out) = open_tui_writer() {
            restore_terminal_on(&mut out);
        } else {
            let _ = disable_raw_mode();
        }
    });

    whycode_core::logging::emit(
        "whycode_tui",
        "info",
        "tui.starting",
        Some(serde_json::json!({
            "provider": provider,
            "model": model,
            "stdout_tty": io::stdout().is_terminal(),
            "stdin_tty": io::stdin().is_terminal(),
        })),
    );

    let mut tui_out = open_tui_writer().map_err(|e| {
        whycode_core::logging::emit(
            "whycode_tui",
            "error",
            "tui.open_writer_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!(
            "failed to open terminal for TUI ({e}). \
             Run inside a real terminal, or use `whycode --plain`."
        )
    })?;

    enable_raw_mode().map_err(|e| {
        whycode_core::logging::emit(
            "whycode_tui",
            "error",
            "tui.raw_mode_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!(
            "failed to enter raw mode ({e}). \
             Run inside a real terminal, or use `whycode --plain`."
        )
    })?;
    // Mouse capture: we own drag-select so clipboard text can be trimmed of
    // background pad spaces. Shift+drag is still native select in many hosts.
    // Bracketed paste: terminals deliver drag-dropped file paths as Event::Paste
    // (and multi-line pastes as one string instead of key spam).
    execute!(
        tui_out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .map_err(|e| {
        let _ = disable_raw_mode();
        whycode_core::logging::emit(
            "whycode_tui",
            "error",
            "tui.alt_screen_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!("failed to enter alternate screen ({e})")
    })?;
    // Lets terminals that support it (Kitty, WezTerm, Alacritty…) report
    // Shift+Enter distinctly, so multi-line input gets a portable binding.
    let keyboard_enhanced = matches!(supports_keyboard_enhancement(), Ok(true));
    if keyboard_enhanced {
        let _ = execute!(
            tui_out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let backend = CrosstermBackend::new(tui_out);
    let mut terminal = Terminal::new(backend).inspect_err(|e| {
        let _ = disable_raw_mode();
        whycode_core::logging::emit(
            "whycode_tui",
            "error",
            "tui.terminal_new_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
    })?;

    // Some hosts (piped stdout, odd PTYs) report 0×0 via TIOCGWINSZ. Drawing a
    // zero-area buffer is useless and has been linked to instant “flash and
    // quit” behaviour — force a sane fallback size.
    let (tw, th) = term_size().unwrap_or((0, 0));
    if tw == 0 || th == 0 {
        let fallback = Rect::new(0, 0, 80, 24);
        let _ = terminal.resize(fallback);
        whycode_core::logging::emit(
            "whycode_tui",
            "warn",
            "tui.size_fallback",
            Some(serde_json::json!({ "reported_w": tw, "reported_h": th, "using": "80x24" })),
        );
    }

    whycode_core::logging::emit(
        "whycode_tui",
        "info",
        "tui.ready",
        Some(serde_json::json!({ "term_w": tw, "term_h": th })),
    );

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<TurnEvent>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<TurnOutcome>();
    // Live context window from config provider's GET …/v1/models (only active model).
    // Channel payload is tiny: (provider, model, context_window) — never the full catalog.
    let (catalog_tx, mut catalog_rx) = mpsc::unbounded_channel::<(String, String, u32)>();
    spawn_model_context_fetch(&config, &provider, &model, &api_key, catalog_tx.clone());

    let mut agent_busy = false;
    let mut cancel_flag: Option<CancelFlag> = None;
    let mut spinner_frame: usize = 0;
    let mut pending_perm_reply: Option<tokio::sync::oneshot::Sender<bool>> = None;

    // Keep home empty so OpenCode-style logo shows (no system spam).
    // Key missing is communicated via footer "Get started /connect".
    if missing_key {
        app.status_message = "no API key · /connect".to_string();
    }

    if let Some(p) = opts.initial_prompt
        && !p.is_empty()
    {
        app.add_message(ChatRole::User, &p);
        app.pending_prompt = Some(p);
    }

    // Inert unless WHYCODE_BENCH is set; see crate::bench.
    let bench = crate::bench::config_from_env();

    let mut first_frame = true;
    let result = async {
        loop {
            // Expire toasts before drawing, so one never lingers a frame past
            // its time.
            app.toasts.prune(std::time::Instant::now());
            let completed = match terminal.draw(|f| render::render(f, &mut app)) {
                Ok(c) => c,
                Err(e) => {
                    whycode_core::logging::emit(
                        "whycode_tui",
                        "error",
                        "tui.draw_failed",
                        Some(serde_json::json!({ "error": e.to_string() })),
                    );
                    return Err(e.into());
                }
            };
            if first_frame {
                first_frame = false;
                whycode_core::logging::emit(
                    "whycode_tui",
                    "info",
                    "tui.first_frame",
                    Some(serde_json::json!({
                        "w": completed.area.width,
                        "h": completed.area.height,
                    })),
                );
            }
            app.screen_cells = snapshot_cells(completed.buffer);
            crate::bench::record_draw();

            if let Some(ref bench) = bench
                && crate::bench::should_stop(bench)
            {
                break;
            }

            // ── Stream events from agent ──────────────────────────────
            while let Ok(ev) = event_rx.try_recv() {
                apply_turn_event(&mut app, ev);
            }

            // Spinner while generating (generic status only)
            if agent_busy && !matches!(app.current_agent_state, AgentState::WaitingForPermission) {
                const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                spinner_frame = (spinner_frame + 1) % FRAMES.len();
                app.spinner_frame = spinner_frame;
                let generic = app.status_message.contains("Generating")
                    || app
                        .status_message
                        .chars()
                        .next()
                        .map(|c| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(c))
                        .unwrap_or(false);
                if generic {
                    // Spinner lives on the turn-status / footer glyphs — keep
                    // status text free of chrome so the turn strip doesn't
                    // repeat "Generating… Esc cancel".
                    app.status_message.clear();
                }
            }

            // ── Permission requests ───────────────────────────────────
            while let Ok(req) = perm_rx.try_recv() {
                pending_perm_reply = Some(req.reply);
                app.ask_permission(req.tool_name, req.detail);
            }

            // ── Turn finished ─────────────────────────────────────────
            if let Ok(outcome) = done_rx.try_recv() {
                agent_busy = false;
                cancel_flag = None;
                let elapsed_ms = app.complete_turn_timing();
                match outcome {
                    TurnOutcome::Ok {
                        text,
                        agent: a,
                        session: s,
                    } => {
                        agent = a;
                        session = s;
                        // Ensure final text is present
                        if !text.is_empty()
                            && let Some(last) = app.messages.last_mut()
                            && last.role == ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            last.content = text;
                        }
                        app.finish_open_thinking();
                        app.current_agent_state = AgentState::Idle;
                        // Agent may have switched branches; keep footer current.
                        app.refresh_git_branch();
                        app.status_message = format_turn_done_status(
                            &app,
                            agent.info.name.as_str(),
                            &provider,
                            &model,
                            elapsed_ms,
                            false,
                        );
                        // Persist after every completed turn (success path).
                        persist_session_best_effort(&session, "ok");
                    }
                    TurnOutcome::Err {
                        error,
                        agent: a,
                        session: s,
                        cancelled,
                    } => {
                        agent = a;
                        session = s;
                        app.finish_open_thinking();
                        if cancelled {
                            app.current_agent_state = AgentState::Idle;
                            app.status_message = format_turn_done_status(
                                &app,
                                agent.info.name.as_str(),
                                &provider,
                                &model,
                                elapsed_ms,
                                true,
                            );
                            app.add_message(ChatRole::System, "⏹ Generation cancelled (Esc).");
                            // Still flush so cancel mid-turn is not lost on crash.
                            persist_session_best_effort(&session, "cancelled");
                        } else {
                            app.current_agent_state = AgentState::Error(error.clone());
                            let dur = elapsed_ms
                                .map(|ms| format!("{} · ", format_elapsed_ms(ms)))
                                .unwrap_or_default();
                            app.status_message = format!("{dur}error — see chat");
                            app.add_message(ChatRole::System, format!("Error: {error}"));
                            app.toasts
                                .push(crate::toast::ToastKind::Error, truncate_toast(&error, 48));
                            whycode_core::logging::emit_sid(
                                "tui",
                                "error",
                                "turn.error",
                                Some(session.id.as_str()),
                                Some(serde_json::json!({
                                    "error": error,
                                    "elapsed_ms": elapsed_ms,
                                })),
                            );
                            persist_session_best_effort(&session, "error");
                        }
                    }
                }
            }

            // ── Apply model picker selection ──────────────────────────
            if let Some((p, m)) = app.pending_model.take() {
                let provider_changed = p != provider;
                provider = p.clone();
                model = m.clone();
                app.provider_name = p.clone();
                app.model_name = m.clone();
                if let Some(k) = config
                    .get_provider(&p)
                    .and_then(|pc| pc.api_key.clone())
                    .or_else(|| std::env::var(format!("{}_API_KEY", p.to_uppercase())).ok())
                {
                    api_key = k;
                }
                if provider_changed {
                    // Different base_url/key — drop stale window and re-fetch.
                    app.clear_api_context_window();
                    spawn_model_context_fetch(
                        &config,
                        &provider,
                        &model,
                        &api_key,
                        catalog_tx.clone(),
                    );
                } else {
                    // Same provider, new model id — re-fetch that model's window.
                    app.clear_api_context_window();
                    spawn_model_context_fetch(
                        &config,
                        &provider,
                        &model,
                        &api_key,
                        catalog_tx.clone(),
                    );
                }
                refresh_context_window(&mut app, &config, &p, &m);
                app.status_message = format!(
                    "Model → {p}/{m}  ·  window {}",
                    format_token_count(app.max_context_tokens),
                );
            }

            // ── Re-fetch when slash /models switches provider ──
            if app.pending_catalog_refresh {
                app.pending_catalog_refresh = false;
                app.clear_api_context_window();
                spawn_model_context_fetch(&config, &provider, &model, &api_key, catalog_tx.clone());
            }

            // ── Apply async single-model context_length from gateway ──
            while let Ok((for_provider, for_model, window)) = catalog_rx.try_recv() {
                if for_provider != provider || for_model != model {
                    continue; // stale in-flight result
                }
                app.set_api_context_window(
                    &for_provider,
                    &for_model,
                    window,
                    config.configured_context_window(&provider, &model),
                    config.session.max_context_tokens as u64,
                );
                whycode_core::logging::emit(
                    "whycode_tui",
                    "info",
                    "tui.context_window_applied",
                    Some(serde_json::json!({
                        "provider": for_provider,
                        "model": for_model,
                        "window": window,
                        "max": app.max_context_tokens,
                    })),
                );
            }

            // ── Start turn if needed ──────────────────────────────────
            if !agent_busy && let Some(prompt) = app.pending_prompt.take() {
                let submit_images = std::mem::take(&mut app.pending_submit_images);

                // Lazy-load API key from env/config when user first chats
                if api_key.is_empty() {
                    if let Ok(cfg) = Config::load()
                        && let Some(pc) = cfg.get_provider(&provider)
                        && let Some(k) = &pc.api_key
                        && !k.is_empty()
                    {
                        api_key = k.clone();
                    }
                    let env_name = format!("{}_API_KEY", provider.to_uppercase());
                    if api_key.is_empty()
                        && let Ok(k) = std::env::var(&env_name)
                        && !k.is_empty()
                    {
                        api_key = k;
                    }
                }
                if api_key.is_empty() {
                    let env_name = format!("{}_API_KEY", provider.to_uppercase());
                    app.add_message(
                        ChatRole::System,
                        format!(
                            "No API key for `{provider}`\n\
                                 → export {env_name}=…\n\
                                 → whycode provider add {provider} --api-key <key> · then /connect"
                        ),
                    );
                    app.status_message = "no API key · /connect".into();
                    app.toasts.push(
                        crate::toast::ToastKind::Warning,
                        format!("Missing {provider} API key"),
                    );
                    // Images already shown on the user bubble; don't re-queue.
                    let _ = submit_images;
                    continue;
                }

                agent_busy = true;
                let flag = new_cancel_flag();
                cancel_flag = Some(Arc::clone(&flag));
                app.mark_turn_started();
                app.current_agent_state = AgentState::Generating;
                app.status_message.clear();
                // Placeholder assistant bubble for streaming
                if app
                    .messages
                    .last()
                    .map(|m| m.role != ChatRole::Assistant)
                    .unwrap_or(true)
                {
                    app.add_message(ChatRole::Assistant, "");
                }

                let expanded = expand_at_files(&prompt, &project_dir);
                history.push_before_turn(&session.messages, &project_dir);
                if submit_images.is_empty() {
                    session.add_user_message(&expanded);
                } else {
                    match crate::images::build_user_blocks(&expanded, &submit_images) {
                        Ok(blocks) => session.add_user_message_blocks(blocks),
                        Err(e) => {
                            app.toasts.push(
                                crate::toast::ToastKind::Warning,
                                format!("Image attach failed: {e}"),
                            );
                            if expanded.trim().is_empty() {
                                session.add_user_message(&format!("(failed to load image: {e})"));
                            } else {
                                session.add_user_message(&expanded);
                            }
                        }
                    }
                }

                let provider2 = provider.clone();
                let model2 = model.clone();
                let api_key2 = api_key.clone();
                let event_tx2 = event_tx.clone();
                let done_tx2 = done_tx.clone();
                let cancel2 = Some(flag);

                // Move agent + session into background task
                let ag = std::mem::replace(
                    &mut agent,
                    // temporary placeholder; restored when turn completes
                    Agent::new(whycode_core::types::AgentInfo {
                        name: "_pending".into(),
                        description: String::new(),
                        mode: AgentMode::Primary,
                        permission: whycode_core::types::PermissionSet::default(),
                        model: None,
                        system_prompt: Some(String::new()),
                        temperature: None,
                        top_p: None,
                    }),
                );
                let sess = std::mem::replace(
                    &mut session,
                    Session::new(project_dir.clone(), String::new()),
                );

                tokio::spawn(async move {
                    let agent = ag;
                    let mut session = sess;
                    let result = agent
                        .run_turn_with_events(
                            &mut session,
                            &provider2,
                            &model2,
                            &api_key2,
                            max_turns,
                            Some(event_tx2),
                            cancel2,
                        )
                        .await;
                    match result {
                        Ok(text) => {
                            let _ = done_tx2.send(TurnOutcome::Ok {
                                text,
                                agent,
                                session,
                            });
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            let cancelled = msg.to_ascii_lowercase().contains("cancel");
                            let _ = done_tx2.send(TurnOutcome::Err {
                                error: msg,
                                agent,
                                session,
                                cancelled,
                            });
                        }
                    }
                });
            }

            // ── Input ─────────────────────────────────────────────────
            // How long to wait for a keystroke before looping again — which is
            // also how often the screen is repainted, because the loop draws
            // every iteration.
            //
            // A fixed 40ms meant ~21 repaints a second with nobody typing, all
            // of them painting an unchanged screen. Input latency is unaffected
            // by lengthening it: `poll` returns the moment a key arrives. What
            // the timeout does control is how quickly the loop notices things
            // that do not arrive as terminal events — streamed tokens on a
            // channel, the spinner, a toast reaching its expiry — so it stays
            // short while any of those are live.
            let idle = !agent_busy && app.toasts.is_empty();
            let poll_for = if idle {
                Duration::from_millis(500)
            } else {
                Duration::from_millis(40)
            };

            let has_ev = match event::poll(poll_for) {
                Ok(v) => v,
                Err(e) => {
                    whycode_core::logging::emit(
                        "whycode_tui",
                        "error",
                        "tui.poll_failed",
                        Some(serde_json::json!({ "error": e.to_string() })),
                    );
                    return Err(e.into());
                }
            };
            if has_ev {
                let ev = match event::read() {
                    Ok(ev) => ev,
                    Err(e) => {
                        whycode_core::logging::emit(
                            "whycode_tui",
                            "error",
                            "tui.read_failed",
                            Some(serde_json::json!({ "error": e.to_string() })),
                        );
                        return Err(e.into());
                    }
                };

                // Permission dialog keys handled specially
                if matches!(app.dialogs.active(), Some(DialogKind::Permission { .. }))
                    && let Event::Key(key) = &ev
                    && key.kind == KeyEventKind::Press
                {
                    match key.code {
                        KeyCode::Char('y')
                        | KeyCode::Char('Y')
                        | KeyCode::Char('a')
                        | KeyCode::Char('A')
                        | KeyCode::Enter => {
                            if let Some(reply) = pending_perm_reply.take() {
                                let _ = reply.send(true);
                            }
                            app.dialogs.pop();
                            app.mode = AppMode::Normal;
                            app.key_context = KeymapContext::Normal;
                            app.current_agent_state = AgentState::Generating;
                            app.status_message = "Allowed — continuing…".into();
                            continue;
                        }
                        KeyCode::Char('n')
                        | KeyCode::Char('N')
                        | KeyCode::Char('d')
                        | KeyCode::Char('D')
                        | KeyCode::Esc => {
                            if let Some(reply) = pending_perm_reply.take() {
                                let _ = reply.send(false);
                            }
                            app.dialogs.pop();
                            app.mode = AppMode::Normal;
                            app.key_context = KeymapContext::Normal;
                            app.current_agent_state = AgentState::Generating;
                            app.status_message = "Denied tool".into();
                            continue;
                        }
                        _ => {}
                    }
                }

                // Ctrl+T: cycle agents (when idle). Tab is focus toggle (Grok).
                if let Event::Key(key) = &ev
                    && key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Char('t')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                    && app.mode == AppMode::Normal
                    && !agent_busy
                {
                    cycle_agent(
                        &mut app,
                        &mut agent,
                        &mut session,
                        &config,
                        &project_dir,
                        Arc::clone(&perm_prompter),
                    )
                    .await;
                    continue;
                }

                // Slash commands on Enter
                if let Event::Key(key) = &ev
                    && key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Enter
                    && app.mode == AppMode::Normal
                    && !agent_busy
                {
                    let mut text = app.input_buffer.trim().to_string();
                    if app.slash_suggest.active
                        && let Some(cmd) = app.slash_suggest.current()
                    {
                        text = cmd.name.to_string();
                    }
                    if text.starts_with('/') {
                        app.input_buffer.clear();
                        app.input_cursor = 0;
                        app.slash_suggest.dismiss();
                        handle_slash(
                            &text,
                            &mut SlashContext {
                                app: &mut app,
                                session: &mut session,
                                history: &mut history,
                                agent: &mut agent,
                                config: &config,
                                project_dir: &project_dir,
                                provider: &mut provider,
                                model: &mut model,
                                api_key: &mut api_key,
                                perm_prompter: Arc::clone(&perm_prompter),
                            },
                        )
                        .await;
                        continue;
                    }
                }

                // While busy: Esc cancels (draft preserved — Grok). Typing, scroll,
                // and focus still work so the user can queue thoughts.
                if agent_busy
                    && let Event::Key(key) = &ev
                    && key.kind == KeyEventKind::Press
                {
                    match key.code {
                        KeyCode::Esc => {
                            if let Some(ref flag) = cancel_flag {
                                request_cancel(flag);
                                app.status_message = "Cancelling…".into();
                                app.esc_armed_at = None;
                            }
                            continue;
                        }
                        KeyCode::Char('q')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            if let Some(ref flag) = cancel_flag {
                                request_cancel(flag);
                            }
                            app.running = false;
                            continue;
                        }
                        KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            // Grok: Ctrl+C clears draft first; second cancels.
                            // We cancel the turn and leave the draft intact unless empty.
                            if app.input_buffer.is_empty() {
                                if let Some(ref flag) = cancel_flag {
                                    request_cancel(flag);
                                }
                                app.status_message = "Cancelling…".into();
                            } else {
                                app.clear_prompt_draft();
                                app.toasts.push(
                                    crate::toast::ToastKind::Info,
                                    "Draft cleared — Ctrl+C again to cancel",
                                );
                            }
                            continue;
                        }
                        KeyCode::Enter => {
                            // Block submit while generating; keep draft.
                            app.toasts.push(
                                crate::toast::ToastKind::Info,
                                "Wait for turn or Esc to cancel",
                            );
                            continue;
                        }
                        _ => {
                            // Typing, scroll, focus toggle — all allowed mid-turn.
                            let _ = input::handle_event(&mut app, ev);
                            continue;
                        }
                    }
                }

                if !input::handle_event(&mut app, ev) {
                    whycode_core::logging::emit(
                        "whycode_tui",
                        "info",
                        "tui.exit",
                        Some(serde_json::json!({ "reason": "handle_event=false" })),
                    );
                    break;
                }
            }

            if !app.running {
                whycode_core::logging::emit(
                    "whycode_tui",
                    "info",
                    "tui.exit",
                    Some(serde_json::json!({ "reason": "running=false" })),
                );
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    // Deny any hanging permission so agent task can finish
    if let Some(reply) = pending_perm_reply.take() {
        let _ = reply.send(false);
    }

    if let Err(ref e) = result {
        whycode_core::logging::emit(
            "whycode_tui",
            "error",
            "tui.loop_error",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
    }

    // Cleanup must not fail the process after a successful session — best-effort.
    let _ = disable_raw_mode();
    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    // Normal exit — panic hook no longer needs to touch the terminal.
    whycode_core::logging::clear_panic_cleanup();

    // After the terminal is restored, so a failed write cannot corrupt the
    // screen the user is left looking at.
    if let Some(ref bench) = bench {
        crate::bench::write_results(bench);
    }

    whycode_core::logging::emit(
        "whycode_tui",
        "info",
        "tui.stopped",
        Some(serde_json::json!({ "ok": result.is_ok() })),
    );

    result
}

/// Best-effort session flush (success, error, or cancel) + structured log.
fn persist_session_best_effort(session: &Session, reason: &str) {
    match open_db_quiet() {
        Some(db) => match session.save_to_db(&db) {
            Ok(()) => {
                whycode_core::logging::emit_sid(
                    "session",
                    "info",
                    "session.persist",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({
                        "reason": reason,
                        "messages": session.messages.len(),
                    })),
                );
            }
            Err(e) => {
                whycode_core::logging::emit_sid(
                    "session",
                    "warn",
                    "session.persist_failed",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({
                        "reason": reason,
                        "error": e.to_string(),
                    })),
                );
                tracing::warn!(error = %e, reason, "failed to persist session");
            }
        },
        None => {
            tracing::debug!(reason, "no database available for session persist");
        }
    }
}

/// Flatten the drawn buffer into `[row][col]` symbols for selection → clipboard.
fn snapshot_cells(buf: &Buffer) -> Vec<Vec<String>> {
    let a = buf.area();
    let mut rows = Vec::with_capacity(a.height as usize);
    for y in a.y..a.y.saturating_add(a.height) {
        let mut row = Vec::with_capacity(a.width as usize);
        for x in a.x..a.x.saturating_add(a.width) {
            row.push(buf[(x, y)].symbol().to_string());
        }
        rows.push(row);
    }
    rows
}

/// Status line after a turn ends: `4.2s · 1.2k in / 340 out · agent=build  xai/model`.
fn format_turn_done_status(
    app: &TuiApp,
    agent_name: &str,
    provider: &str,
    model: &str,
    elapsed_ms: Option<u128>,
    cancelled: bool,
) -> String {
    let mut parts = Vec::new();
    if cancelled {
        parts.push("Cancelled".to_string());
    }
    if let Some(ms) = elapsed_ms {
        parts.push(format_elapsed_ms(ms));
    }
    if let Some(ref usage) = app.turn_usage {
        parts.push(format_usage_short(usage));
    }
    if !cancelled {
        parts.push(format!("agent={agent_name}  {provider}/{model}"));
    } else if parts.len() == 1 {
        // "Cancelled" alone — keep a period for the old short form.
        return "Cancelled.".into();
    }
    parts.join(" · ")
}

fn apply_turn_event(app: &mut TuiApp, ev: TurnEvent) {
    match ev {
        TurnEvent::TextDelta(t) => {
            app.finish_open_thinking();
            app.current_agent_state = AgentState::Generating;
            app.append_to_last(&t);
        }
        TurnEvent::ThinkingDelta(t) => {
            app.current_agent_state = AgentState::Thinking;
            app.append_thinking(&t);
        }
        TurnEvent::ToolStart { id, name, input } => {
            app.finish_open_thinking();
            app.status_message = format!("tool: {name}");
            app.add_tool_call(id, name, input);
        }
        TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        } => {
            app.add_tool_result(&id, content, is_error);
        }
        TurnEvent::Status(s) => {
            app.status_message = s;
        }
        TurnEvent::Usage(usage) => {
            app.turn_usage = Some(usage.clone());
            // Per-step input size ≈ context window fill (Grok-style meter).
            app.set_context_from_usage(&usage);
            let mut parts = Vec::new();
            if let Some(ms) = app.turn_elapsed_ms() {
                parts.push(format_elapsed_ms(ms));
            }
            parts.push(format_usage_short(&usage));
            app.status_message = parts.join(" · ");
        }
        TurnEvent::Cancelled => {
            app.finish_open_thinking();
            app.status_message = "Cancelled.".into();
            app.current_agent_state = AgentState::Idle;
        }
    }
}

async fn cycle_agent(
    app: &mut TuiApp,
    agent: &mut Agent,
    session: &mut Session,
    config: &Config,
    project_dir: &std::path::Path,
    perm_prompter: Arc<ChannelPermissionPrompter>,
) {
    if app.primary_agents.is_empty() {
        return;
    }
    app.agent_cycle_idx = (app.agent_cycle_idx + 1) % app.primary_agents.len();
    let name = app.primary_agents[app.agent_cycle_idx].clone();
    // Always update agent_name so colors/header reflect the switch
    app.agent_name = name.clone();
    app.status_message = format!("Agent → {name}");
    if let Some(info) = config.get_agent(&name).cloned() {
        let base = info
            .system_prompt
            .clone()
            .unwrap_or_else(|| Agent::system_prompt_for(&name));
        let prompt = Agent::with_agents_md(&base, project_dir);
        *agent = Agent::new(info)
            .with_config(config)
            .with_permission_prompter(
                Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
            );
        session.set_system_prompt(&prompt);
    }
}

fn expand_at_files(input: &str, project_dir: &std::path::Path) -> String {
    let mut result = String::new();
    let mut rest = input;
    while let Some(at) = rest.find('@') {
        result.push_str(&rest[..at]);
        let after = &rest[at + 1..];
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
                result.push_str(&format!(
                    "\n\n--- file: {path_str} ---\n{content}\n--- end file ---\n\n"
                ));
            }
            Err(_) => {
                result.push('@');
                result.push_str(path_str);
            }
        }
        rest = &after[end..];
    }
    result.push_str(rest);
    result
}

fn open_db_quiet() -> Option<whycode_storage::db::Database> {
    let data_dir = whycode_core::config::Config::data_dir().ok()?;
    std::fs::create_dir_all(&data_dir).ok()?;
    let db_path = data_dir.join("whycode.db");
    whycode_storage::db::Database::open(&db_path.to_string_lossy()).ok()
}

fn share_server_up(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/api/health");
    // Sync quick check via TCP connect (no reqwest dependency in tui path)
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(80),
    )
    .is_ok()
        && {
            // optional: ignore unused url
            let _ = url;
            true
        }
}

fn unshare_session(project_dir: &std::path::Path, id: &str) -> usize {
    let mut n = 0usize;
    let candidates = [
        project_dir.join(".whycode").join("shares"),
        whycode_core::config::Config::data_dir()
            .map(|d| d.join("shares"))
            .unwrap_or_default(),
    ];
    for dir in candidates {
        for ext in ["json", "md"] {
            let p = dir.join(format!("{id}.{ext}"));
            if p.exists() && std::fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
    }
    n
}

async fn handle_slash(text: &str, ctx: &mut SlashContext<'_>) {
    let (cmd, rest) = match text.find(char::is_whitespace) {
        Some(i) => (&text[..i], text[i..].trim()),
        None => (text, ""),
    };

    // Custom commands from config
    if let Some(name) = cmd.strip_prefix('/')
        && let Some(custom) = ctx.config.commands.get(name)
    {
        let rendered = custom.render(rest);
        ctx.app.add_message(ChatRole::User, &rendered);
        ctx.app.pending_prompt = Some(rendered);
        return;
    }

    match cmd {
        "/exit" | "/quit" | "/q" => {
            ctx.app.running = false;
        }
        "/help" | "/h" => {
            ctx.app.mode = AppMode::Help;
            ctx.app.key_context = KeymapContext::Help;
        }
        "/new" | "/clear" => {
            *ctx.history = SessionHistory::new();
            *ctx.session = Session::new(
                ctx.project_dir.to_path_buf(),
                Agent::with_agents_md(&ctx.agent.system_prompt(), ctx.project_dir),
            );
            ctx.app.messages.clear();
            ctx.app.context_used = ctx.session.token_count() as u64;
            ctx.app.turn_usage = None;
            ctx.app
                .toasts
                .push(crate::toast::ToastKind::Success, "New session");
        }
        "/undo" => {
            if let Some(msgs) = ctx.history.undo(&ctx.session.messages, ctx.project_dir) {
                ctx.session.set_messages(msgs);
                ctx.app.messages.clear();
                ctx.app.status_message = "Undid last turn".into();
            } else if ctx.session.undo_last_turn() > 0 {
                ctx.app.status_message = "Undid last turn".into();
            } else {
                ctx.app.status_message = "Nothing to undo".into();
            }
        }
        "/redo" => {
            if let Some(msgs) = ctx.history.redo(&ctx.session.messages, ctx.project_dir) {
                ctx.session.set_messages(msgs);
                ctx.app.status_message = "Redid turn".into();
            } else {
                ctx.app.status_message = "Nothing to redo".into();
            }
        }
        "/compact" | "/summarize" => {
            let before = ctx.session.messages.len();
            ctx.session.compact(ctx.config.session.compaction_threshold);
            // Provider usage is stale after trim — fall back to the char heuristic.
            ctx.app.context_used = ctx.session.token_count() as u64;
            ctx.app.status_message = format!("Compacted {before} → {}", ctx.session.messages.len());
        }
        "/share" | "/export" => match ctx.session.export_share() {
            Ok(p) => {
                let md = p.replace(".json", ".md");
                let id = ctx.session.id.clone();
                let port = std::env::var("WHYCODE_SHARE_PORT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(3030u16);
                let url = format!("http://127.0.0.1:{port}/s/{id}");
                let live = share_server_up(port);
                ctx.app.status_message = if live {
                    format!("Share: {url}")
                } else {
                    format!("Exported — run `whycode serve` then open {url}")
                };
                ctx.app.add_message(
                    ChatRole::System,
                    format!(
                        "Session shared locally:\n\
                         - {p}\n\
                         - {md}\n\
                         - View URL: {url}\n\
                         {}\n\
                         /unshare removes local share files.",
                        if live {
                            "(server is up)"
                        } else {
                            "Start server: whycode serve 3030"
                        }
                    ),
                );
            }
            Err(e) => ctx.app.toasts.push(
                crate::toast::ToastKind::Error,
                format!("Export failed: {e}"),
            ),
        },
        "/unshare" => {
            let id = ctx.session.id.clone();
            let removed = unshare_session(ctx.project_dir, &id);
            ctx.app.status_message = if removed > 0 {
                format!("Unshared ({removed} files)")
            } else {
                "No share files found".into()
            };
        }
        "/connect" => {
            // Reload key from env / config
            if let Ok(cfg) = Config::load()
                && let Some(pc) = cfg.get_provider(ctx.provider)
                && let Some(k) = &pc.api_key
                && !k.is_empty()
            {
                *ctx.api_key = k.clone();
            }
            let env_name = format!("{}_API_KEY", ctx.provider.to_uppercase());
            if ctx.api_key.is_empty()
                && let Ok(k) = std::env::var(&env_name)
                && !k.is_empty()
            {
                *ctx.api_key = k;
            }
            if ctx.api_key.is_empty() {
                ctx.app.status_message = format!("no API key · set {env_name}");
                ctx.app.add_message(
                    ChatRole::System,
                    format!(
                        "No API key for `{}`\n\
                         → export {env_name}=…\n\
                         → whycode provider add {} --api-key <key> · then /connect",
                        ctx.provider, ctx.provider
                    ),
                );
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    format!("Still no key for {}", ctx.provider),
                );
            } else {
                ctx.app.status_message = format!("API key loaded · {}", ctx.provider);
                ctx.app.add_message(
                    ChatRole::System,
                    format!("✓ API key ready for `{}` / `{}`", ctx.provider, ctx.model),
                );
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!("Connected · {}", ctx.provider),
                );
            }
        }
        "/agent" => {
            if rest.is_empty() {
                ctx.app.status_message = format!(
                    "Agent: {} — Tab cycles {:?}",
                    ctx.agent.info.name, ctx.app.primary_agents
                );
            } else if let Some(info) = ctx.config.get_agent(rest).cloned() {
                let base = info
                    .system_prompt
                    .clone()
                    .unwrap_or_else(|| Agent::system_prompt_for(rest));
                let prompt = Agent::with_agents_md(&base, ctx.project_dir);
                *ctx.agent = Agent::new(info)
                    .with_config(ctx.config)
                    .with_permission_prompter(Arc::clone(&ctx.perm_prompter)
                        as Arc<dyn whycode_agent::PermissionPrompter>);
                ctx.session.set_system_prompt(&prompt);
                if let Some(idx) = ctx.app.primary_agents.iter().position(|n| n == rest) {
                    ctx.app.agent_cycle_idx = idx;
                }
                ctx.app.agent_name = rest.to_string();
                ctx.app.status_message = format!("Switched to agent '{rest}'");
            } else {
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    format!("Unknown agent '{rest}'"),
                );
            }
        }
        "/sessions" | "/resume" | "/continue" => {
            // Previously only existed in the --plain REPL.
            ctx.app.session_list.sessions = load_session_entries();
            ctx.app.session_list.selected = 0;
            crate::input::open_dialog(ctx.app, DialogKind::SessionList);
        }
        "/models" if rest.is_empty() => {
            ctx.app.model_selection.models = configured_models(ctx.config);
            ctx.app.model_selection.selected = ctx
                .app
                .model_selection
                .models
                .iter()
                .position(|(p, m)| p == ctx.provider && m == ctx.model)
                .unwrap_or(0);
            crate::input::open_dialog(ctx.app, DialogKind::Model);
        }
        "/models" => {
            if rest.is_empty() {
                let src = if ctx
                    .app
                    .api_context_for
                    .as_ref()
                    .is_some_and(|(p, m)| p == ctx.provider.as_str() && m == ctx.model.as_str())
                {
                    "api"
                } else {
                    "local"
                };
                ctx.app.status_message = format!(
                    "Model: {}/{}  ·  ctx {} / {} ({src})",
                    ctx.provider,
                    ctx.model,
                    crate::app::format_token_count(ctx.app.context_used),
                    crate::app::format_token_count(ctx.app.max_context_tokens),
                );
            } else if let Some((p, m)) = rest.split_once('/') {
                let provider_changed = p != ctx.provider.as_str();
                *ctx.provider = p.to_string();
                *ctx.model = m.to_string();
                ctx.app.provider_name = p.to_string();
                ctx.app.model_name = m.to_string();
                if let Some(k) = ctx
                    .config
                    .get_provider(p)
                    .and_then(|pc| pc.api_key.clone())
                    .or_else(|| std::env::var(format!("{}_API_KEY", p.to_uppercase())).ok())
                {
                    *ctx.api_key = k;
                }
                if provider_changed {
                    ctx.app.clear_api_context_window();
                    ctx.app.pending_catalog_refresh = true;
                } else {
                    // Model-only change: still need a fresh window for the new id.
                    ctx.app.clear_api_context_window();
                    ctx.app.pending_catalog_refresh = true;
                }
                refresh_context_window(ctx.app, ctx.config, p, m);
                ctx.app.status_message = format!(
                    "Model → {}/{}  ·  window {}",
                    ctx.provider,
                    ctx.model,
                    crate::app::format_token_count(ctx.app.max_context_tokens),
                );
            } else {
                *ctx.model = rest.to_string();
                ctx.app.model_name = rest.to_string();
                refresh_context_window(ctx.app, ctx.config, ctx.provider, rest);
                ctx.app.status_message = format!(
                    "Model → {}  ·  window {}",
                    ctx.model,
                    crate::app::format_token_count(ctx.app.max_context_tokens),
                );
            }
        }
        "/tools" => {
            let tools =
                whycode_tools::ToolExecutor::new().get_definitions(&ctx.agent.info.permission);
            ctx.app.status_message = format!("{} tools", tools.len());
            ctx.app.add_message(
                ChatRole::System,
                tools
                    .iter()
                    .map(|t| format!("• {} — {}", t.name, t.description))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        "/info" | "/details" => {
            ctx.app.add_message(
                ChatRole::System,
                session_details(ctx.session, &ctx.agent.info.name, ctx.app),
            );
        }
        "/init" => {
            ctx.app.add_message(
                ChatRole::User,
                "Create or update AGENTS.md for this project with build/test conventions and architecture notes. Write the file.",
            );
            ctx.app.pending_prompt = Some(
                "Analyze this project and write a complete AGENTS.md at the project root with build/test commands, conventions, and architecture. Use the write tool.".into(),
            );
        }
        other => {
            ctx.app.toasts.push(
                crate::toast::ToastKind::Warning,
                format!("Unknown command {other} — try /help"),
            );
        }
    }
}

fn truncate_toast(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or(s).trim();
    let n = first.chars().count();
    if n <= max {
        first.to_string()
    } else {
        format!(
            "{}…",
            first
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    }
}

/// Recompute footer context max for the active provider/model.
fn refresh_context_window(app: &mut TuiApp, config: &Config, provider: &str, model: &str) {
    app.apply_context_window(
        provider,
        model,
        config.configured_context_window(provider, model),
        config.session.max_context_tokens as u64,
    );
}

/// Background `GET {config.base_url}/models` — extract **one** model's window.
///
/// No-op without `base_url`/`api_base`. Failures are logged only; meter keeps
/// config/built-in fallback. Never stores the full gateway list in the TUI.
fn spawn_model_context_fetch(
    config: &Config,
    provider: &str,
    model: &str,
    runtime_api_key: &str,
    tx: mpsc::UnboundedSender<(String, String, u32)>,
) {
    // Opt-out for debugging hang/crash suspicions: WHYCODE_NO_MODEL_CATALOG=1
    if std::env::var_os("WHYCODE_NO_MODEL_CATALOG").is_some() {
        tracing::debug!("WHYCODE_NO_MODEL_CATALOG set — skip /v1/models");
        return;
    }

    let Some(req) = whycode_llm::catalog_request_from_config(
        config,
        provider,
        if runtime_api_key.is_empty() {
            None
        } else {
            Some(runtime_api_key)
        },
    ) else {
        tracing::debug!(
            provider,
            "no base_url/api_base in config — skip /v1/models fetch"
        );
        return;
    };

    let provider_name = req.provider_name.clone();
    let model = model.to_string();
    let url = whycode_llm::normalize_models_url(&req.base_url);
    tokio::spawn(async move {
        match whycode_llm::fetch_model_context_window(&req, &model).await {
            Ok(Some(window)) => {
                tracing::info!(
                    provider = %provider_name,
                    model = %model,
                    %url,
                    window,
                    "GET /v1/models context_length ok"
                );
                let _ = tx.send((provider_name, model, window));
            }
            Ok(None) => {
                tracing::debug!(
                    provider = %provider_name,
                    model = %model,
                    %url,
                    "model not in /v1/models list — using local fallback"
                );
            }
            Err(e) => {
                tracing::warn!(
                    provider = %provider_name,
                    model = %model,
                    %url,
                    error = %e,
                    "GET /v1/models failed (using local context fallback)"
                );
            }
        }
    });
}

/// Every provider/model pair the config knows about, for the model picker.
fn configured_models(config: &Config) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = config
        .providers
        .values()
        .flat_map(|p| p.models.iter().map(move |m| (p.name.clone(), m.clone())))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Stored sessions, newest first, for the session picker.
///
/// A database that will not open is not worth interrupting the user for here —
/// the picker shows its empty state, and `whycode session list` reports the
/// actual error.
fn load_session_entries() -> Vec<crate::app::SessionEntry> {
    let Ok(data_dir) = Config::data_dir() else {
        return Vec::new();
    };
    let path = data_dir.join("whycode.db");
    let Ok(db) = whycode_storage::db::Database::open(&path.to_string_lossy()) else {
        return Vec::new();
    };
    db.list_sessions()
        .unwrap_or_default()
        .into_iter()
        .map(|s| crate::app::SessionEntry {
            messages: db.message_count(&s.id).unwrap_or(0),
            id: s.id,
            title: s.title,
        })
        .collect()
}

/// Session details for `/info`.
///
/// Reports the provider's own token counts when it gave any. The character
/// heuristic is shown only when it did not, and labelled as an estimate — the
/// two are not the same measurement and presenting them identically would
/// suggest they are.
fn session_details(session: &Session, agent: &str, app: &TuiApp) -> String {
    let usage = &session.usage;
    let mut out = format!(
        "Session\n  agent:     {agent}\n  messages:  {}\n  model:     {}/{}\n  context:   {} / {} ({}%)\n",
        session.messages.len(),
        app.provider_name,
        app.model_name,
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent(),
    );

    if usage.is_empty() {
        out.push_str(&format!(
            "  tokens:    ~{} (estimated; the provider has not reported usage yet)\n",
            session.token_count()
        ));
        return out;
    }

    out.push_str(&format!(
        "  input:     {}\n  output:    {}\n",
        usage.input_tokens, usage.output_tokens
    ));
    if let Some(created) = usage.cache_creation_input_tokens {
        out.push_str(&format!("  cache write: {created}\n"));
    }
    if let Some(read) = usage.cache_read_input_tokens {
        out.push_str(&format!("  cache read:  {read}\n"));
    }
    out.push_str(&format!("  total:     {}\n", usage.total()));
    out
}
