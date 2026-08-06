//! TUI event loop — streaming agent + permission dialogs (OpenCode-style).

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use whycode_agent::{
    CancelFlag, ChannelQuestionPrompter, QuestionError, QuestionPrompter, QuestionRequest,
    TurnEvent, new_cancel_flag, request_cancel,
};
use whycode_config::Config;
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
    pub question_prompter: Arc<ChannelQuestionPrompter>,
}

fn bind_agent_prompters(
    agent: Agent,
    perm: &Arc<ChannelPermissionPrompter>,
    question: &Arc<ChannelQuestionPrompter>,
) -> Agent {
    agent
        .with_permission_prompter(Arc::clone(perm) as Arc<dyn whycode_agent::PermissionPrompter>)
        .with_question_prompter(Arc::clone(question) as Arc<dyn QuestionPrompter>)
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
    /// When set, load this session (or the latest if `"__latest__"`) before first paint.
    ///
    /// Used by CLI `--continue` / `--resume <id>`. In-session resume uses
    /// `pending_session_id` on the app instead.
    pub resume_session_id: Option<String>,
}

/// Sentinel for `TuiRunOptions::resume_session_id`: most recently updated session.
pub const RESUME_LATEST: &str = "__latest__";

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
        /// Wall time for `run_turn` only (excludes post-turn title refine).
        work_ms: u128,
    },
    Err {
        error: String,
        agent: Agent,
        session: Session,
        cancelled: bool,
        work_ms: u128,
    },
}

/// Run the full-screen TUI until the user quits.
pub async fn run(opts: TuiRunOptions) -> anyhow::Result<()> {
    // Wall clock for the Cline-style exit summary (process open → quit).
    let session_started = Instant::now();

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

    // Primary agents for Ctrl+T cycle
    app.primary_agents = config
        .agents
        .iter()
        .filter(|a| a.mode == AgentMode::Primary || a.mode == AgentMode::All)
        .map(|a| a.name.clone())
        .collect();
    if app.primary_agents.is_empty() {
        app.primary_agents = vec!["build".into(), "plan".into(), "ask".into()];
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
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base, &opts.project_dir),
        &opts.project_dir,
        &config,
        None,
    );

    // Permission channel: agent blocks until TUI replies (shared across agent switches)
    let (perm_prompter, mut perm_rx) = ChannelPermissionPrompter::new();
    let perm_prompter: Arc<ChannelPermissionPrompter> = Arc::new(perm_prompter);

    // Questionnaire channel (Grok-style `question` tool)
    let q_timeout = if config.tools.question.timeout_enabled {
        Some(Duration::from_secs(config.tools.question.timeout_secs.max(1)))
    } else {
        None
    };
    let (question_prompter, mut question_rx) = ChannelQuestionPrompter::new(q_timeout);
    let question_prompter: Arc<ChannelQuestionPrompter> = Arc::new(question_prompter);

    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
        )
        .with_question_prompter(
            Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>
        )
        .with_mcp(&config)
        .await;

    let mut session = Session::new(opts.project_dir.clone(), system_prompt.clone());
    app.session_title = session.title.clone();
    // Code RAG: index once if empty (skips when chunks already exist).
    maybe_session_auto_index(&opts.project_dir, &config, &mut app);
    let mut history = SessionHistory::new();

    // CLI `--continue` / `--resume`: hydrate before first paint when possible.
    if let Some(ref want) = opts.resume_session_id {
        match try_load_session(want) {
            Ok(Some(loaded)) => {
                let n = loaded.messages.len();
                session = loaded;
                // system_prompt is not persisted yet — keep the live agent prompt.
                session.system_prompt = system_prompt.clone();
                if opts.config.session.auto_title && session.maybe_upgrade_title_from_history() {
                    persist_session_best_effort(&session, "title_backfill");
                }
                let title = session.title.clone();
                app.load_messages_from_session(&session);
                app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!("Resumed · {title} ({n} msgs)"),
                );
            }
            Ok(None) => {
                app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    if want == RESUME_LATEST {
                        "No saved sessions to continue".into()
                    } else {
                        format!("Session not found: {want}")
                    },
                );
            }
            Err(e) => {
                app.toasts.push(
                    crate::toast::ToastKind::Error,
                    format!("Resume failed: {e}"),
                );
            }
        }
    }

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
    // Async session titles (small-model refine) — never blocks agent_busy.
    // Payload: (session_id, title) so a late refine cannot touch another session.
    let (title_tx, mut title_rx) = mpsc::unbounded_channel::<(String, String)>();
    // Live context window from config provider's GET …/v1/models (only active model).
    // Channel payload is tiny: (provider, model, context_window) — never the full catalog.
    //
    // Do **not** spawn at TUI open: a slow/hanging catalog races the first chat
    // on the same gateway host and can serialize the turn (wall ≫ server Duration).
    // Queue a fetch after the first turn finishes, or on model switch when idle.
    let (catalog_tx, mut catalog_rx) = mpsc::unbounded_channel::<(String, String, u32)>();
    let mut catalog_fetch_pending = false;

    let mut agent_busy = false;
    let mut cancel_flag: Option<CancelFlag> = None;
    let mut spinner_frame: usize = 0;
    // Permission queue: multiple tool asks can complete without serializing
    // the whole parallel tool batch on a single oneshot slot.
    let mut pending_question_queue: std::collections::VecDeque<QuestionRequest> =
        std::collections::VecDeque::new();
    let mut pending_perm_queue: std::collections::VecDeque<whycode_agent::PermissionRequest> =
        std::collections::VecDeque::new();
    // Title may arrive before TurnOutcome restores the real session; hold it.
    let mut pending_async_title: Option<(String, String)> = None;

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
            if app.toasts.prune(std::time::Instant::now()) {
                app.mark_dirty();
            }

            // Animation paths that do not arrive as terminal events still need
            // periodic paints (spinner, toast stack). Idle with a clean flag
            // skips the draw entirely → 0 idle redraws/s.
            let animate = agent_busy || !app.toasts.is_empty();
            if app.needs_redraw || animate || first_frame {
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
                // Cell snapshot is only for mouse text selection → clipboard.
                // Skip the ~4k String allocs/frame when nothing is selected.
                if app.mouse_sel.is_some() {
                    app.screen_cells = snapshot_cells(completed.buffer);
                } else if !app.screen_cells.is_empty() {
                    app.screen_cells.clear();
                }
                crate::bench::record_draw();
                // Stay dirty while animation is live; otherwise clear so the
                // next idle poll does not repaint an unchanged screen.
                app.needs_redraw = animate;
            }

            if let Some(ref bench) = bench
                && crate::bench::should_stop(bench)
            {
                break;
            }

            // ── Stream events from agent (coalesce text/thinking deltas) ──
            if drain_turn_events(&mut app, &mut event_rx) {
                app.mark_dirty();
            }

            // Spinner while generating (generic status only)
            if agent_busy
                && !matches!(
                    app.current_agent_state,
                    AgentState::WaitingForPermission | AgentState::WaitingForQuestion
                )
            {
                const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                spinner_frame = (spinner_frame + 1) % FRAMES.len();
                app.spinner_frame = spinner_frame;
                app.mark_dirty();
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

            // ── Permission requests (queued; show next when idle) ─────
            while let Ok(req) = perm_rx.try_recv() {
                pending_perm_queue.push_back(req);
            }
            if !matches!(
                app.current_agent_state,
                AgentState::WaitingForPermission | AgentState::WaitingForQuestion
            ) && let Some(front) = pending_perm_queue.front()
            {
                app.ask_permission(front.tool_name.clone(), front.detail.clone());
                app.mark_dirty();
            }

            // ── Question requests (queued; one questionnaire at a time) ─
            while let Ok(req) = question_rx.try_recv() {
                pending_question_queue.push_back(req);
            }
            if !matches!(
                app.current_agent_state,
                AgentState::WaitingForPermission | AgentState::WaitingForQuestion
            ) && let Some(front) = pending_question_queue.front()
            {
                app.ask_question(front.questions.clone());
                app.mark_dirty();
            }

            // ── Async title refine (does not hold agent_busy) ─────────
            while let Ok((sid, title)) = title_rx.try_recv() {
                if session.id == sid {
                    if session.apply_generated_title(&title) {
                        app.session_title = session.title.clone();
                        persist_session_best_effort(&session, "title_async");
                        app.mark_dirty();
                    }
                } else {
                    // Turn still restoring session, or user switched sessions.
                    pending_async_title = Some((sid, title));
                }
            }

            // ── Turn finished ─────────────────────────────────────────
            if let Ok(outcome) = done_rx.try_recv() {
                agent_busy = false;
                cancel_flag = None;
                // One deferred catalog pass if we still lack a live window.
                // Avoid re-fetching after every turn (would race fast follow-ups).
                if app.api_context_window.is_none() {
                    catalog_fetch_pending = true;
                }
                // Stamp "Worked for Xs" from work_ms only (title refine is async
                // and never included — see spawn_title_refine below).
                let work_ms = match &outcome {
                    TurnOutcome::Ok { work_ms, .. } | TurnOutcome::Err { work_ms, .. } => *work_ms,
                };
                let elapsed_ms = Some(app.complete_turn_timing_ms(work_ms));
                app.mark_dirty();
                match outcome {
                    TurnOutcome::Ok {
                        text,
                        agent: a,
                        session: s,
                        work_ms: _,
                    } => {
                        agent = a;
                        session = s;
                        // Apply a title that raced ahead of this restore.
                        if let Some((sid, title)) = pending_async_title.take()
                            && session.id == sid
                        {
                            let _ = session.apply_generated_title(&title);
                        }
                        app.session_title = session.title.clone();
                        // Ensure final text is present
                        if !text.is_empty()
                            && let Some(last) = app.messages.last_mut()
                            && last.role == ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            last.content = text.clone();
                        }
                        // Auto-retain runs inside Agent::run_turn (heuristic + LLM).
                        // Surface agent Status events as toasts already via drain path.
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
                        work_ms: _,
                    } => {
                        agent = a;
                        session = s;
                        if let Some((sid, title)) = pending_async_title.take()
                            && session.id == sid
                        {
                            let _ = session.apply_generated_title(&title);
                        }
                        app.session_title = session.title.clone();
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
                            // Clean user-facing copy; full wire body stays in logs.
                            let display = whycode_llm::format_turn_error(
                                &whycode_core::Error::Llm(error.clone()),
                            );
                            app.current_agent_state = AgentState::Error(display.clone());
                            let dur = elapsed_ms
                                .map(|ms| format!("{} · ", format_elapsed_ms(ms)))
                                .unwrap_or_default();
                            app.status_message = format!("{dur}error — see chat");
                            app.add_message(ChatRole::System, format!("Error: {display}"));
                            app.toasts.push(
                                crate::toast::ToastKind::Error,
                                truncate_toast(&display, 48),
                            );
                            whycode_core::logging::emit_sid(
                                "tui",
                                "error",
                                "turn.error",
                                Some(session.id.as_str()),
                                Some(serde_json::json!({
                                    "error": error,
                                    "display": display,
                                    "elapsed_ms": elapsed_ms,
                                })),
                            );
                            persist_session_best_effort(&session, "error");
                        }
                    }
                }
            }

            // ── Apply session picker / /resume selection ──────────────
            if let Some(id) = app.pending_session_id.take() {
                // Don't switch mid-turn — re-queue and wait.
                if agent_busy {
                    app.pending_session_id = Some(id);
                } else {
                    match try_load_session(&id) {
                        Ok(Some(loaded)) => {
                            // Persist the current session first so nothing is lost
                            // when switching away from an unsaved turn.
                            if !session.messages.is_empty() {
                                persist_session_best_effort(&session, "switch");
                            }
                            let n = loaded.messages.len();
                            history = SessionHistory::new();
                            session = loaded;
                            session.system_prompt = with_project_memory(
                                &Agent::with_agents_md(&agent.system_prompt(), &project_dir),
                                &project_dir,
                                &config,
                                None,
                            );
                            if config.session.auto_title
                                && session.maybe_upgrade_title_from_history()
                            {
                                persist_session_best_effort(&session, "title_backfill");
                            }
                            let title = session.title.clone();
                            app.load_messages_from_session(&session);
                            app.toasts.push(
                                crate::toast::ToastKind::Success,
                                format!("Resumed · {title} ({n} msgs)"),
                            );
                            app.status_message =
                                format!("Resumed session {}", short_session_id(&session.id));
                        }
                        Ok(None) => {
                            app.toasts.push(
                                crate::toast::ToastKind::Warning,
                                format!("Session not found: {}", short_session_id(&id)),
                            );
                        }
                        Err(e) => {
                            app.toasts.push(
                                crate::toast::ToastKind::Error,
                                format!("Resume failed: {e}"),
                            );
                        }
                    }
                }
            }

            // ── Apply model picker selection ──────────────────────────
            if let Some((p, m)) = app.pending_model.take() {
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
                // Drop stale window; re-fetch when idle so we never contend with a turn.
                app.clear_api_context_window();
                if agent_busy {
                    catalog_fetch_pending = true;
                } else {
                    catalog_fetch_pending = false;
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
                if agent_busy {
                    // Don't contend with the in-flight turn; retry when idle.
                    catalog_fetch_pending = true;
                } else {
                    catalog_fetch_pending = false;
                    spawn_model_context_fetch(
                        &config,
                        &provider,
                        &model,
                        &api_key,
                        catalog_tx.clone(),
                    );
                }
            }

            // Deferred / idle catalog: never race the first (or any) user turn.
            if catalog_fetch_pending
                && !agent_busy
                && app.pending_prompt.is_none()
                && !missing_key
            {
                catalog_fetch_pending = false;
                spawn_model_context_fetch(
                    &config,
                    &provider,
                    &model,
                    &api_key,
                    catalog_tx.clone(),
                );
            }

            // ── Apply async single-model context_length from gateway ──
            while let Ok((for_provider, for_model, window)) = catalog_rx.try_recv() {
                if for_provider != provider || for_model != model {
                    continue; // stale in-flight result
                }
                app.mark_dirty();
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

            // Mouse `[stop]` on the turn strip (or other UI) → cancel.
            if agent_busy && app.pending_cancel {
                app.pending_cancel = false;
                if let Some(ref flag) = cancel_flag {
                    request_cancel(flag);
                    app.status_message = "Cancelling…".into();
                }
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
                // Auto-recall memories relevant to this user turn (Grok/jcode style).
                refresh_session_memory(
                    &mut session,
                    &agent,
                    &project_dir,
                    &config,
                    Some(&expanded),
                );
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

                // Instant offline title (no API cost). Prefer the transcript's
                // first user message so resumed legacy placeholders name from
                // the original topic, not the latest follow-up.
                if config.session.auto_title {
                    let seed = session
                        .first_user_text()
                        .unwrap_or_else(|| expanded.clone());
                    if session.apply_heuristic_title(&seed) {
                        app.session_title = session.title.clone();
                    }
                }

                // Fast model for trivial chat (selam/hi) — sibling or config.
                let (route_provider, route_model) = whycode_agent::resolve_turn_model(
                    &provider,
                    &model,
                    &expanded,
                    agent.model_fast().or(config.session.model_fast.as_deref()),
                );
                if route_model != model || route_provider != provider {
                    tracing::info!(
                        from = %format!("{provider}/{model}"),
                        to = %format!("{route_provider}/{route_model}"),
                        "routed trivial turn to fast model"
                    );
                    whycode_core::logging::emit_sid(
                        "tui",
                        "info",
                        "turn.route_fast",
                        Some(session.id.as_str()),
                        Some(serde_json::json!({
                            "from": format!("{provider}/{model}"),
                            "to": format!("{route_provider}/{route_model}"),
                        })),
                    );
                }

                let provider2 = route_provider;
                let model2 = route_model;
                let api_key2 = api_key.clone();
                let event_tx2 = event_tx.clone();
                let done_tx2 = done_tx.clone();
                let cancel2 = Some(flag);
                let auto_title = config.session.auto_title;
                let title_model = config.session.title_model.clone();
                let title_tx2 = title_tx.clone();
                // Title refine still uses session provider/model (or title_model).
                let title_provider = provider.clone();
                let title_session_model = model.clone();

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
                    // Time only the agent loop. Title refine runs async *after*
                    // we release agent_busy so the user can type immediately.
                    let work_t0 = std::time::Instant::now();
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
                    let work_ms = work_t0.elapsed().as_millis();
                    // Kick off small-model title refine without awaiting — the
                    // main loop applies the title when title_tx delivers.
                    if auto_title && result.is_ok() {
                        let _ = agent.spawn_title_refine(
                            &session,
                            &title_provider,
                            &title_session_model,
                            &api_key2,
                            title_model.as_deref(),
                            title_tx2,
                        );
                    }
                    match result {
                        Ok(text) => {
                            let _ = done_tx2.send(TurnOutcome::Ok {
                                text,
                                agent,
                                session,
                                work_ms,
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
                                work_ms,
                            });
                        }
                    }
                });
            }

            // ── Input ─────────────────────────────────────────────────
            // How long to wait for a keystroke before looping again.
            //
            // Paint is gated by `needs_redraw` / animation — poll timeout no
            // longer implies a full redraw. Short timeout while the agent is
            // busy or toasts are live so spinner / stream / expiry stay snappy;
            // long timeout when idle so we do not spin the CPU.
            let idle = !agent_busy && app.toasts.is_empty() && !app.needs_redraw;
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
                app.mark_dirty();
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
                            if let Some(req) = pending_perm_queue.pop_front() {
                                let _ = req.reply.send(true);
                            }
                            app.dialogs.pop();
                            app.mode = AppMode::Normal;
                            app.key_context = KeymapContext::Normal;
                            if let Some(next) = pending_perm_queue.front() {
                                app.ask_permission(next.tool_name.clone(), next.detail.clone());
                                app.status_message = format!(
                                    "Allowed — {} more permission(s)…",
                                    pending_perm_queue.len()
                                );
                            } else {
                                app.current_agent_state = AgentState::Generating;
                                app.status_message = "Allowed — continuing…".into();
                            }
                            continue;
                        }
                        KeyCode::Char('n')
                        | KeyCode::Char('N')
                        | KeyCode::Char('d')
                        | KeyCode::Char('D')
                        | KeyCode::Esc => {
                            if let Some(req) = pending_perm_queue.pop_front() {
                                let _ = req.reply.send(false);
                            }
                            app.dialogs.pop();
                            app.mode = AppMode::Normal;
                            app.key_context = KeymapContext::Normal;
                            if let Some(next) = pending_perm_queue.front() {
                                app.ask_permission(next.tool_name.clone(), next.detail.clone());
                                app.status_message = format!(
                                    "Denied — {} more permission(s)…",
                                    pending_perm_queue.len()
                                );
                            } else {
                                app.current_agent_state = AgentState::Generating;
                                app.status_message = "Denied tool".into();
                            }
                            continue;
                        }
                        _ => {}
                    }
                }

                // Questionnaire dialog (Grok-style `question` tool)
                if matches!(app.dialogs.active(), Some(DialogKind::Question(_)))
                    && let Event::Key(key) = &ev
                    && key.kind == KeyEventKind::Press
                {
                    let handled = handle_question_key(
                        &mut app,
                        key.code,
                        &mut pending_question_queue,
                        &mut pending_perm_queue,
                    );
                    if handled {
                        app.mark_dirty();
                        continue;
                    }
                }

                // Mouse single-select may finish the questionnaire from input.rs
                if let Some(answers) = app.pending_question_answers.take() {
                    if let Some(req) = pending_question_queue.pop_front() {
                        let _ = req.reply.send(Ok(answers));
                    }
                    if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
                        app.mode = AppMode::Normal;
                        app.key_context = KeymapContext::Normal;
                        resume_after_question(
                            &mut app,
                            &pending_question_queue,
                            &pending_perm_queue,
                        );
                    }
                }

                // [✗] / Esc may dismiss Question via input.rs — complete oneshot
                if app.question_dismissed {
                    app.question_dismissed = false;
                    if let Some(req) = pending_question_queue.pop_front() {
                        let _ = req.reply.send(Err(QuestionError::Cancelled));
                    }
                    if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
                        resume_after_question(
                            &mut app,
                            &pending_question_queue,
                            &pending_perm_queue,
                        );
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
                        Arc::clone(&question_prompter),
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
                        app.pending_pastes.clear();
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
                                question_prompter: Arc::clone(&question_prompter),
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

    // Deny any hanging permissions / questionnaires so agent tasks can finish
    while let Some(req) = pending_perm_queue.pop_front() {
        let _ = req.reply.send(false);
    }
    while let Some(req) = pending_question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
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

    // Final flush + Cline-style summary on the normal terminal (scrollback).
    persist_session_best_effort(&session, "exit");
    let model_label = format!("{provider}/{model}");
    let summary = session.format_exit_summary(session_started.elapsed(), &model_label, "whycode");
    print_session_summary(&summary);

    whycode_core::logging::emit(
        "whycode_tui",
        "info",
        "tui.stopped",
        Some(serde_json::json!({
            "ok": result.is_ok(),
            "session_id": session.id,
            "messages": session.messages.len(),
            "duration_s": session_started.elapsed().as_secs(),
        })),
    );

    result
}

/// Print the exit summary where the user will see it after alt-screen leave.
///
/// The TUI draws on `/dev/tty`; after restore, prefer that same device so the
/// lines land in the real terminal scrollback even when stdout is captured.
/// Fall back to stdout, then stderr.
fn print_session_summary(summary: &str) {
    #[cfg(unix)]
    {
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            if writeln!(tty, "{summary}").is_ok() && tty.flush().is_ok() {
                return;
            }
        }
    }
    let mut out = io::stdout();
    if writeln!(out, "{summary}").is_ok() && out.flush().is_ok() {
        return;
    }
    let mut err = io::stderr();
    let _ = writeln!(err, "{summary}");
    let _ = err.flush();
}

/// Best-effort session flush (success, error, or cancel) + structured log.
fn persist_session_best_effort(session: &Session, reason: &str) {
    let outcome = with_session_db(|db| session.save_to_db(db));
    match outcome {
        Some(Ok(())) => {
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
        Some(Err(e)) => {
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
        None => {
            tracing::debug!(reason, "no database available for session persist");
        }
    }
}

/// Process-lifetime SQLite handle for the TUI (avoids re-running migrations
/// and reopening the file on every turn persist).
fn with_session_db<T>(f: impl FnOnce(&whycode_storage::db::Database) -> T) -> Option<T> {
    use std::sync::{Mutex, OnceLock};
    static DB: OnceLock<Mutex<Option<whycode_storage::db::Database>>> = OnceLock::new();
    let lock = DB.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().ok()?;
    if guard.is_none() {
        *guard = open_db_quiet();
    }
    guard.as_ref().map(f)
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

/// Status line after a turn ends (header chrome).
///
/// Grok-like: `Worked for 4.2s · 1.2k in / 340 out` — no `agent=` noise.
/// Cancelled: `Turn cancelled in 4.2s`.
fn format_turn_done_status(
    app: &TuiApp,
    _agent_name: &str,
    _provider: &str,
    _model: &str,
    elapsed_ms: Option<u128>,
    cancelled: bool,
) -> String {
    if cancelled {
        return match elapsed_ms {
            Some(ms) => format!("Turn cancelled in {}", format_elapsed_ms(ms)),
            None => "Turn cancelled.".into(),
        };
    }
    let mut parts = Vec::new();
    if let Some(ms) = elapsed_ms {
        parts.push(format!("Worked for {}", format_elapsed_ms(ms)));
    } else {
        parts.push("Done".into());
    }
    if let Some(ref usage) = app.turn_usage {
        parts.push(format_usage_short(usage));
    }
    parts.join(" · ")
}

/// Drain the agent event channel, coalescing consecutive text/thinking deltas
/// into one UI append each. Returns whether any event was applied.
fn drain_turn_events(app: &mut TuiApp, event_rx: &mut mpsc::UnboundedReceiver<TurnEvent>) -> bool {
    let mut any = false;
    let mut text_buf = String::new();
    let mut think_buf = String::new();

    let flush_text = |app: &mut TuiApp, buf: &mut String| {
        if buf.is_empty() {
            return;
        }
        app.finish_open_thinking();
        app.current_agent_state = AgentState::Generating;
        app.append_to_last(buf);
        buf.clear();
    };
    let flush_think = |app: &mut TuiApp, buf: &mut String| {
        if buf.is_empty() {
            return;
        }
        app.current_agent_state = AgentState::Thinking;
        app.append_thinking(buf);
        buf.clear();
    };

    while let Ok(ev) = event_rx.try_recv() {
        any = true;
        match ev {
            TurnEvent::TextDelta(t) => {
                flush_think(app, &mut think_buf);
                text_buf.push_str(&t);
            }
            TurnEvent::ThinkingDelta(t) => {
                flush_text(app, &mut text_buf);
                think_buf.push_str(&t);
            }
            other => {
                flush_text(app, &mut text_buf);
                flush_think(app, &mut think_buf);
                apply_turn_event(app, other);
            }
        }
    }
    flush_text(app, &mut text_buf);
    flush_think(app, &mut think_buf);
    any
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
            // Post-turn niceties (e.g. async memory retain) may arrive after
            // Idle — surface as a quiet toast so we don't clobber "Worked for…".
            if !app.is_busy() && s.starts_with("Remembered ") {
                app.toasts
                    .push(crate::toast::ToastKind::Info, truncate_toast(&s, 48));
            } else {
                app.status_message = s;
            }
            app.mark_dirty();
        }
        TurnEvent::Intent {
            kind,
            confidence: _,
            badge,
            notice_kind,
            notice,
        } => {
            app.intent_kind = Some(kind);
            app.intent_badge = if badge.is_empty() {
                None
            } else {
                Some(badge)
            };
            if !notice.is_empty() {
                let toast_kind = match notice_kind.as_str() {
                    "warning" => crate::toast::ToastKind::Warning,
                    _ => crate::toast::ToastKind::Info,
                };
                // Warnings: full message (mode mismatch). Info: compact.
                let msg = if matches!(toast_kind, crate::toast::ToastKind::Warning) {
                    truncate_toast(&notice, 96)
                } else {
                    truncate_toast(&notice, 56)
                };
                app.toasts.push(toast_kind, msg);
            }
            app.mark_dirty();
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
            app.mark_dirty();
        }
        TurnEvent::Cancelled => {
            app.finish_open_thinking();
            app.status_message = "Cancelled.".into();
            app.current_agent_state = AgentState::Idle;
            app.mark_dirty();
        }
        TurnEvent::FileConflict {
            path,
            claimant,
            owner,
        } => {
            // Conflict notify: short warning toast so concurrent writers are visible.
            let short_path = path.rsplit('/').next().unwrap_or(&path);
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                truncate_toast(
                    &format!("File conflict: {short_path} ({claimant} vs {owner})"),
                    72,
                ),
            );
            app.status_message = format!("conflict: {short_path}");
            app.mark_dirty();
        }
        TurnEvent::SwarmStatus {
            active: _,
            total,
            message,
        } => {
            app.status_message = if message.is_empty() {
                format!("swarm {total}…")
            } else {
                message
            };
            app.mark_dirty();
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
    question_prompter: Arc<ChannelQuestionPrompter>,
) {
    if app.primary_agents.is_empty() {
        return;
    }
    app.agent_cycle_idx = (app.agent_cycle_idx + 1) % app.primary_agents.len();
    let name = app.primary_agents[app.agent_cycle_idx].clone();
    // Always update agent_name so colors/header reflect the switch
    app.agent_name = name.clone();
    app.intent_badge = None;
    app.intent_kind = None;
    app.status_message = format!("Agent → {name}");
    app.toasts.push(
        crate::toast::ToastKind::Info,
        format!("Agent → {name}  (Ctrl+T)"),
    );
    if let Some(info) = config.get_agent(&name).cloned() {
        let base = info
            .system_prompt
            .clone()
            .unwrap_or_else(|| Agent::system_prompt_for(&name));
        let prompt = with_project_memory(
            &Agent::with_agents_md(&base, project_dir),
            project_dir,
            config,
            None,
        );
        *agent = Agent::new(info)
            .with_config(config)
            .with_permission_prompter(
                Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
            )
            .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);
        session.set_system_prompt(&prompt);
    }
}

/// Handle keys while a questionnaire panel is open. Returns true if consumed.
fn handle_question_key(
    app: &mut TuiApp,
    code: KeyCode,
    pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &std::collections::VecDeque<whycode_agent::PermissionRequest>,
) -> bool {
    let Some(DialogKind::Question(mut state)) = app.dialogs.pop() else {
        return false;
    };

    let finish_cancel = |app: &mut TuiApp,
                         pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
                         pending_perm_queue: &std::collections::VecDeque<
        whycode_agent::PermissionRequest,
    >| {
        if let Some(req) = pending_question_queue.pop_front() {
            let _ = req.reply.send(Err(QuestionError::Cancelled));
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        resume_after_question(app, pending_question_queue, pending_perm_queue);
    };

    let finish_ok = |app: &mut TuiApp,
                     answers: Vec<whycode_tools::question::QuestionAnswer>,
                     pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
                     pending_perm_queue: &std::collections::VecDeque<
        whycode_agent::PermissionRequest,
    >| {
        if let Some(req) = pending_question_queue.pop_front() {
            let _ = req.reply.send(Ok(answers));
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        resume_after_question(app, pending_question_queue, pending_perm_queue);
    };

    match code {
        KeyCode::Esc => {
            if state.free_text_focus && !state.free_text.is_empty() {
                state.free_text_focus = false;
                app.dialogs.push(DialogKind::Question(state));
                return true;
            }
            if state.free_text_focus {
                state.free_text_focus = false;
                app.dialogs.push(DialogKind::Question(state));
                return true;
            }
            finish_cancel(app, pending_question_queue, pending_perm_queue);
            return true;
        }
        KeyCode::Up | KeyCode::Char('k') if !state.free_text_focus => {
            state.move_cursor(-1);
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Down | KeyCode::Char('j') if !state.free_text_focus => {
            state.move_cursor(1);
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        // Multi-question navigate (Grok-style ←/→ between questions)
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') if !state.free_text_focus => {
            let _ = state.go_prev_question();
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') if !state.free_text_focus => {
            let _ = state.go_next_question();
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        // Copy full questionnaire to clipboard
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('c') | KeyCode::Char('C')
            if !state.free_text_focus =>
        {
            let text = state.clipboard_text();
            if crate::clipboard::copy_text(&text) {
                app.toasts.push(
                    crate::toast::ToastKind::Info,
                    format!("Copied question ({} chars)", text.chars().count()),
                );
            } else {
                app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    "Copy failed — no clipboard",
                );
            }
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Char(' ') if !state.free_text_focus => {
            if state.current().map(|q| q.multi_select).unwrap_or(false) {
                state.toggle_multi_at_cursor();
            } else if state.is_other_index(state.cursor) {
                state.free_text_focus = true;
            }
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Char('o') | KeyCode::Char('O') if !state.free_text_focus => {
            // Jump to Other…
            let other = state.option_count().saturating_sub(1);
            state.cursor = other;
            state.free_text_focus = true;
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Enter => {
            if let Some(answers) = state.confirm_current() {
                finish_ok(app, answers, pending_question_queue, pending_perm_queue);
            } else {
                // Still on this question (e.g. empty Other → focus free text)
                app.dialogs.push(DialogKind::Question(state));
            }
            return true;
        }
        KeyCode::Backspace if state.free_text_focus => {
            state.free_text.pop();
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Char(c) if state.free_text_focus && !c.is_control() => {
            state.free_text.push(c);
            app.dialogs.push(DialogKind::Question(state));
            return true;
        }
        KeyCode::Char(c) if !state.free_text_focus && !c.is_control() => {
            // Digit shortcut 1..n for single-select
            if let Some(d) = c.to_digit(10) {
                let idx = (d as usize).saturating_sub(1);
                if idx < state.option_count() {
                    state.cursor = idx;
                    if state.is_other_index(idx) {
                        state.free_text_focus = true;
                        app.dialogs.push(DialogKind::Question(state));
                    } else if state.current().map(|q| q.multi_select).unwrap_or(false) {
                        state.multi_selected.insert(idx);
                        app.dialogs.push(DialogKind::Question(state));
                    } else if let Some(answers) = state.confirm_current() {
                        finish_ok(app, answers, pending_question_queue, pending_perm_queue);
                    } else {
                        app.dialogs.push(DialogKind::Question(state));
                    }
                    return true;
                }
            }
            app.dialogs.push(DialogKind::Question(state));
            return false;
        }
        _ => {
            app.dialogs.push(DialogKind::Question(state));
            false
        }
    }
}

fn resume_after_question(
    app: &mut TuiApp,
    pending_question_queue: &std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &std::collections::VecDeque<whycode_agent::PermissionRequest>,
) {
    if let Some(next) = pending_question_queue.front() {
        app.ask_question(next.questions.clone());
        app.status_message = format!(
            "Answered — {} more question set(s)…",
            pending_question_queue.len()
        );
    } else if let Some(next) = pending_perm_queue.front() {
        app.ask_permission(next.tool_name.clone(), next.detail.clone());
    } else {
        app.current_agent_state = AgentState::Generating;
        app.status_message = "Answered — continuing…".into();
    }
}

/// Max chars inlined per `@file` (speculative context without blowing prefill).
const AT_FILE_MAX_CHARS: usize = 24_000;

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
    let data_dir = whycode_config::Config::data_dir().ok()?;
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
        whycode_config::Config::data_dir()
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
                with_project_memory(
                    &Agent::with_agents_md(&ctx.agent.system_prompt(), ctx.project_dir),
                    ctx.project_dir,
                    ctx.config,
                    None,
                ),
            );
            ctx.app.session_title = ctx.session.title.clone();
            ctx.app.messages.clear();
            ctx.app.context_used = ctx.session.token_count() as u64;
            ctx.app.turn_usage = None;
            ctx.app
                .toasts
                .push(crate::toast::ToastKind::Success, "New session");
        }
        "/rename" => {
            let name = rest.trim();
            if name.is_empty() {
                ctx.app.status_message =
                    format!("Title: {} — usage: /rename <name>", ctx.session.title);
            } else {
                ctx.session.set_title_manual(name);
                ctx.app.session_title = ctx.session.title.clone();
                // Persist immediately so the session picker sees the rename.
                persist_session_best_effort(ctx.session, "rename");
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!("Renamed → {}", ctx.session.title),
                );
            }
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
        "/remember" => {
            let text = rest.trim();
            if text.is_empty() {
                ctx.app.status_message = "Usage: /remember <text>".into();
            } else {
                match memory_service(ctx.project_dir, ctx.config) {
                    Ok(svc) => match svc.remember(text, Some(&ctx.session.id)) {
                        Ok(id) => {
                            ctx.app.toasts.push(
                                crate::toast::ToastKind::Success,
                                format!("Remembered {}", &id[..8.min(id.len())]),
                            );
                            ctx.app.status_message = format!("Saved memory: {text}");
                        }
                        Err(e) => {
                            ctx.app
                                .toasts
                                .push(crate::toast::ToastKind::Error, format!("Memory: {e}"));
                        }
                    },
                    Err(e) => {
                        ctx.app
                            .toasts
                            .push(crate::toast::ToastKind::Error, format!("Memory: {e}"));
                    }
                }
            }
        }
        "/memory" => {
            match memory_service(ctx.project_dir, ctx.config) {
                Ok(svc) => {
                    let n = svc.list(1000).map(|r| r.len()).unwrap_or(0);
                    let path = svc.memory_md_path();
                    let mut msg = format!(
                        "Memory enabled={} · {} entries · {}\nproject_key={}",
                        ctx.config.memory.enabled,
                        n,
                        path.display(),
                        svc.project_key
                    );
                    if let Ok(rows) = svc.list(8) {
                        for r in rows {
                            msg.push_str(&format!(
                                "\n· {}  {}",
                                &r.id[..8.min(r.id.len())],
                                r.text
                            ));
                        }
                    }
                    ctx.app.add_message(ChatRole::System, msg);
                    ctx.app.status_message = format!("Memory · {n} entries");
                }
                Err(e) => {
                    ctx.app
                        .toasts
                        .push(crate::toast::ToastKind::Error, format!("Memory: {e}"));
                }
            }
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
                let prompt = with_project_memory(
                    &Agent::with_agents_md(&base, ctx.project_dir),
                    ctx.project_dir,
                    ctx.config,
                    None,
                );
                *ctx.agent = bind_agent_prompters(
                    Agent::new(info).with_config(ctx.config),
                    &ctx.perm_prompter,
                    &ctx.question_prompter,
                );
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
        "/sessions" => {
            ctx.app.session_list.sessions = load_session_entries();
            ctx.app.session_list.selected = 0;
            crate::input::open_dialog(ctx.app, DialogKind::SessionList);
        }
        "/resume" | "/continue" => {
            // With an id (or prefix): resume immediately. Bare: open picker.
            // `/continue` with no id resumes the most recently updated session
            // (same semantics as CLI `--continue`).
            if !rest.is_empty() {
                ctx.app.pending_session_id = Some(rest.to_string());
            } else if cmd == "/continue" {
                ctx.app.pending_session_id = Some(RESUME_LATEST.to_string());
            } else {
                ctx.app.session_list.sessions = load_session_entries();
                ctx.app.session_list.selected = 0;
                crate::input::open_dialog(ctx.app, DialogKind::SessionList);
            }
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
            // List what the model actually sees (core profile by default).
            let profile = whycode_tools::ToolProfile::parse(&ctx.config.session.tool_profile);
            let tools = whycode_tools::ToolExecutor::new()
                .get_definitions_profile(&ctx.agent.info.permission, profile);
            let full_n = whycode_tools::ToolExecutor::new()
                .get_definitions(&ctx.agent.info.permission)
                .len();
            ctx.app.status_message =
                format!("{} tools (profile: {})", tools.len(), profile.as_str());
            let header = format!(
                "Tool profile: **{}** — {} advertised to the model ({} registered in binary).\n\
                 Config: `session.tool_profile = \"core\"|\"full\"`\n\n",
                profile.as_str(),
                tools.len(),
                full_n
            );
            ctx.app.add_message(
                ChatRole::System,
                header
                    + &tools
                        .iter()
                        .map(|t| format!("• {} — {}", t.name, t.description))
                        .collect::<Vec<_>>()
                        .join("\n"),
            );
        }
        "/info" | "/details" => {
            ctx.app.add_message(
                ChatRole::System,
                session_details(ctx.session, &ctx.agent.info.name, ctx.app, ctx.config),
            );
        }
        "/theme" | "/themes" => {
            use crate::theme::ThemeName;
            if rest.is_empty() {
                // Open picker; select current theme.
                ctx.app.theme_selected = ThemeName::ALL
                    .iter()
                    .position(|t| *t == ctx.app.theme)
                    .unwrap_or(0);
                crate::input::open_dialog(ctx.app, DialogKind::Theme);
            } else if let Ok(t) = rest.parse::<ThemeName>() {
                ctx.app.theme = t;
                ctx.app.config.theme = t;
                ctx.app.config.theme_override = None;
                ctx.app.theme_selected = ThemeName::ALL
                    .iter()
                    .position(|x| *x == t)
                    .unwrap_or(0);
                ctx.app.status_message = format!("Theme → {}", t.name());
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!("Theme · {}", t.name()),
                );
            } else {
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    format!("Unknown theme '{rest}' — try /theme"),
                );
            }
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
///
/// While building the list, backfill placeholder titles (`New session - …`,
/// `project-ab`) from the first user message so the picker stays useful for
/// sessions created before auto-title or never refined.
fn load_session_entries() -> Vec<crate::app::SessionEntry> {
    let Some(db) = with_session_db(|d| {
        // Clone rows we need while the lock is held; do backfill with a second
        // borrow after we drop the map borrow (same connection).
        let rows = d.list_sessions().unwrap_or_default();
        let counts = d.message_counts_by_session().unwrap_or_default();
        (rows, counts)
    }) else {
        return Vec::new();
    };
    let (rows, counts) = db;
    let mut out = Vec::with_capacity(rows.len());
    for s in rows {
        let messages = counts.get(&s.id).copied().unwrap_or(0);
        let mut title = s.title;
        if messages > 0
            && whycode_session::title::looks_like_default_title(
                &title,
                std::path::Path::new(&s.project_path),
            )
        {
            // Backfill under the shared handle so we do not re-open the DB.
            let upgraded = with_session_db(|d| {
                if let Ok(Some(mut loaded)) = Session::load_from_db(d, &s.id)
                    && loaded.maybe_upgrade_title_from_history()
                {
                    if let Err(err) = loaded.save_to_db(d) {
                        tracing::warn!(error = %err, "failed to persist backfilled session title");
                    }
                    Some(loaded.title)
                } else {
                    None
                }
            })
            .flatten();
            if let Some(t) = upgraded {
                title = t;
            }
        }
        out.push(crate::app::SessionEntry {
            messages,
            id: s.id,
            title,
        });
    }
    out
}

fn memory_settings(config: &Config) -> whycode_memory::MemorySettings {
    memory_settings_for(config, None)
}

fn memory_settings_for(
    config: &Config,
    agent_bank: Option<String>,
) -> whycode_memory::MemorySettings {
    let mut s = whycode_agent::memory_settings_from_config(config);
    s.agent_bank = agent_bank;
    s
}

/// Best-effort code index when the TUI session starts (skips if already indexed).
fn maybe_session_auto_index(project_dir: &std::path::Path, config: &Config, app: &mut TuiApp) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(n) =
        whycode_memory::maybe_auto_index(project_dir, &data_dir, &memory_settings(config))
    {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            format!("Indexed {n} code chunks"),
        );
    }
}

fn with_project_memory(
    system_prompt: &str,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) -> String {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    whycode_memory::apply_memory_prompt(
        system_prompt,
        project_dir,
        &data_dir,
        &memory_settings(config),
        query,
    )
}

fn refresh_session_memory(
    session: &mut Session,
    agent: &Agent,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) {
    let base = Agent::with_agents_md(&agent.system_prompt(), project_dir);
    session.set_system_prompt(&with_project_memory(&base, project_dir, config, query));
}

fn memory_service(
    project_dir: &std::path::Path,
    config: &Config,
) -> anyhow::Result<whycode_memory::MemoryService> {
    let data_dir = Config::data_dir()?;
    whycode_memory::MemoryService::open(project_dir, data_dir, memory_settings(config))
}

/// Shorten a UUID-style session id for status lines (`a1b2c3d4…`).
fn short_session_id(id: &str) -> String {
    let take = id.chars().take(8).collect::<String>();
    if id.chars().count() > 8 {
        format!("{take}…")
    } else {
        take
    }
}

/// Load a session by exact id, unique prefix, or [`RESUME_LATEST`].
fn try_load_session(want: &str) -> anyhow::Result<Option<Session>> {
    match with_session_db(|db| resolve_and_load_session(db, want)) {
        Some(r) => r,
        None => anyhow::bail!("database unavailable"),
    }
}

/// Resolve `want` against the session table and load the full transcript.
///
/// - [`RESUME_LATEST`] → first row of `list_sessions` (ORDER BY updated_at DESC)
/// - exact id match
/// - otherwise unique prefix (case-insensitive); ambiguous prefix → error
pub fn resolve_and_load_session(
    db: &whycode_storage::db::Database,
    want: &str,
) -> anyhow::Result<Option<Session>> {
    if want == RESUME_LATEST || want.eq_ignore_ascii_case("latest") {
        let list = db.list_sessions()?;
        let Some(row) = list.into_iter().next() else {
            return Ok(None);
        };
        return Session::load_from_db(db, &row.id);
    }

    if let Some(s) = Session::load_from_db(db, want)? {
        return Ok(Some(s));
    }

    // Prefix match (handy for typing the first 8 chars from `/sessions`).
    let want_l = want.to_ascii_lowercase();
    let list = db.list_sessions()?;
    let matches: Vec<_> = list
        .into_iter()
        .filter(|s| s.id.to_ascii_lowercase().starts_with(&want_l))
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Session::load_from_db(db, &matches[0].id),
        n => anyhow::bail!("ambiguous session id prefix '{want}' ({n} matches); use a longer id"),
    }
}

/// Session details for `/info`.
///
/// Reports the provider's own token counts when it gave any. The character
/// heuristic is shown only when it did not, and labelled as an estimate — the
/// two are not the same measurement and presenting them identically would
/// suggest they are.
fn session_details(
    session: &Session,
    agent: &str,
    app: &TuiApp,
    config: &Config,
) -> String {
    let usage = &session.usage;
    let profile = whycode_tools::ToolProfile::parse(&config.session.tool_profile);
    let mut out = format!(
        "Session\n  title:     {}\n  source:    {:?}\n  id:        {}\n  agent:     {agent}\n  messages:  {}\n  model:     {}/{}\n  context:   {} / {} ({}%)\n  tools:     profile={}\n  prompt_cache: {}\n",
        session.title,
        session.title_source,
        session.id,
        session.messages.len(),
        app.provider_name,
        app.model_name,
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent(),
        profile.as_str(),
        config.session.prompt_cache,
    );
    if let Some(ref fast) = config.session.model_fast {
        out.push_str(&format!("  model_fast: {fast}\n"));
    } else {
        out.push_str("  model_fast: (auto small sibling on trivial chat)\n");
    }
    out.push_str(&format!(
        "  swarm:     enabled={} max_agents={} worktrees={}\n",
        config.swarm.enabled, config.swarm.max_agents, config.swarm.worktrees
    ));

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
