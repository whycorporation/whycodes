//! TUI event loop — streaming agent + permission dialogs.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
    AgentState, AppMode, ChatRole, DialogKind, FocusPane, TuiApp, format_elapsed_ms,
    format_token_count, format_usage_short,
};
use crate::config::TuiAppConfig;
use crate::input;
use crate::keymap::KeymapContext;
use crate::session_runtime::SessionRuntime;
use crate::ui::render;

/// After Esc / [stop], wait this long for cooperative cancel, then abort the
/// turn task hard. Must be short enough that "Cancelling…" never feels stuck.
const CANCEL_FORCE_AFTER: Duration = Duration::from_millis(1200);

/// Cap on concurrently live sessions (each holds a full transcript + agent).
const MAX_LIVE_SESSIONS: usize = 8;

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
    pub auth_tx: mpsc::UnboundedSender<AuthFlowEvent>,
}

/// Messages from an in-flight OAuth login task (`/connect` with no stored
/// credential starts one) back to the TUI event loop.
pub enum AuthFlowEvent {
    /// Progress text from the flow (URL to open, device code, waiting…).
    Note(String),
    /// Anthropic paste flow: the flow needs the user to paste `code#state`.
    /// The next submitted input line is forwarded through this sender.
    NeedCode(tokio::sync::oneshot::Sender<String>),
    /// Flow finished: Ok(label) on success, Err(message) on failure.
    Done {
        provider: String,
        result: Result<String, String>,
    },
}

/// Drives `whycode_auth::providers::LoginUi` from the TUI: notes land in
/// the chat transcript, the pasted code is collected via the prompt box.
struct TuiLoginUi {
    tx: mpsc::UnboundedSender<AuthFlowEvent>,
}

impl TuiLoginUi {
    /// Best-effort delivery to the TUI event loop: a send only fails when the
    /// loop is gone (shutdown), and then the note has nowhere to land anyway.
    fn send(&self, event: AuthFlowEvent) {
        if self.tx.send(event).is_err() {
            tracing::debug!("auth-flow event dropped: TUI event loop closed");
        }
    }
}

impl whycode_auth::providers::LoginUi for TuiLoginUi {
    fn show_sign_in(&mut self, label: &str, url: &str, browser_opened: bool) {
        let browser = if browser_opened {
            "Browser opened — complete the sign-in there."
        } else {
            "Open the URL above manually."
        };
        self.send(AuthFlowEvent::Note(format!(
            "Sign in with {label}:\n  {url}\n{browser}"
        )));
    }

    fn note(&mut self, text: &str) {
        self.send(AuthFlowEvent::Note(text.to_string()));
    }

    fn show_device_code(&mut self, user_code: &str, verification_uri: &str, browser_opened: bool) {
        let browser = if browser_opened {
            "Browser opened — enter the code there."
        } else {
            "Open the URL manually."
        };
        self.send(AuthFlowEvent::Note(format!(
            "GitHub Copilot login:\n  1. Visit  {verification_uri}\n  2. Enter code:  {user_code}\n{browser}"
        )));
    }

    fn prompt_pasted_code(
        &mut self,
    ) -> impl std::future::Future<Output = whycode_auth::error::Result<String>> + Send {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(AuthFlowEvent::NeedCode(tx));
        async move {
            rx.await.map_err(|_| {
                whycode_auth::AuthError::FlowCancelled("sign-in dismissed".to_string())
            })
        }
    }
}

/// Spawn the OAuth subscription login flow for `provider`, reporting progress
/// back to the event loop via `tx`. Shared by `/connect` (current provider,
/// no key) and `/login` (explicit picker choice).
fn spawn_oauth_login(
    app: &mut TuiApp,
    tx: &mpsc::UnboundedSender<AuthFlowEvent>,
    dir: std::path::PathBuf,
    provider: &str,
) {
    let p = provider.to_string();
    let tx = tx.clone();
    app.add_message(
        ChatRole::System,
        format!("Starting `{p}` subscription sign-in… (Esc cancels)"),
    );
    tokio::spawn(async move {
        let store = whycode_auth::TokenStore::new(&dir);
        let mut ui = TuiLoginUi { tx: tx.clone() };
        let result = whycode_auth::providers::login_with_ui(&p, &store, true, &mut ui)
            .await
            .map(|_| p.clone())
            .map_err(|e| e.to_string());
        if tx
            .send(AuthFlowEvent::Done {
                provider: p,
                result,
            })
            .is_err()
        {
            tracing::debug!("auth-flow Done dropped: TUI event loop closed");
        }
    });
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
    /// When set, turns go to `whycode serve` over HTTP instead of an in-process agent.
    pub remote: Option<crate::remote::RemoteAttach>,
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

pub enum TurnOutcome {
    Ok {
        text: String,
        agent: Agent,
        session: Session,
        /// Wall time for `run_turn` only (excludes post-turn title refine).
        work_ms: u128,
    },
    /// Remote `whycode serve` turn finished; local agent/session stay in place.
    Remote {
        text: String,
        error: Option<String>,
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

    // Workspace file index: background scan of the project + allowed external
    // dirs; powers the `@file` picker (Ctrl+Space) and, once warm, the file
    // tools' glob/grep/list fast paths.
    let file_index = whycode_index::WorkspaceIndex::start(
        whycode_index::WorkspaceIndex::project_roots(&opts.project_dir),
    );
    app.set_file_index(file_index.clone());

    // Session chrome labels
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
    let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
    let perm_prompter: Arc<ChannelPermissionPrompter> = Arc::new(perm_prompter);

    // Questionnaire channel (Grok-style `question` tool)
    let q_timeout = if config.tools.question.timeout_enabled {
        Some(Duration::from_secs(
            config.tools.question.timeout_secs.max(1),
        ))
    } else {
        None
    };
    let (question_prompter, question_rx) = ChannelQuestionPrompter::new(q_timeout);
    let question_prompter: Arc<ChannelQuestionPrompter> = Arc::new(question_prompter);

    config.general.project_path = Some(opts.project_dir.clone());
    // Plugins only at boot — MCP connect can block on slow servers. Defer MCP
    // (+ memory auto-index) until after the first frame so the TUI paints ASAP.
    let session_claims = whycode_core::FileClaimRegistry::new();
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index.clone())
        .with_session_claims(session_claims.clone())
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
        )
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>)
        .with_plugins(Some(opts.project_dir.as_path()));

    let remote = opts.remote.clone();
    let mut session = Session::new(opts.project_dir.clone(), system_prompt.clone());
    app.session_title = session.title.clone();
    // Welcome /resume list (Grok home) — cheap SQLite read, newest first.
    if app.session_list.sessions.is_empty() {
        app.session_list.sessions = load_session_entries();
    }
    // Code RAG auto-index deferred past first paint (see loop below).
    let history = SessionHistory::new();

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

    if let Some(ref rem) = remote {
        match crate::remote::fetch_messages(&rem.base_url, &rem.session_id).await {
            Ok((title, msgs)) => {
                session.id = rem.session_id.clone();
                if !title.is_empty() {
                    session.title = title;
                }
                if !msgs.is_empty() {
                    session.messages = msgs;
                    app.load_messages_from_session(&session);
                }
                app.session_title = session.title.clone();
            }
            Err(e) => {
                session.id = rem.session_id.clone();
                app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    format!("Remote hydrate: {e}"),
                );
            }
        }
        app.toasts.push(
            crate::toast::ToastKind::Info,
            format!("Attached · {} (auto-approves tools)", rem.base_url),
        );
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

    let (event_tx, event_rx) = mpsc::unbounded_channel::<TurnEvent>();
    let (done_tx, done_rx) = mpsc::unbounded_channel::<TurnOutcome>();
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
    // A7 idle prompt suggestions (default off).
    let (suggest_tx, mut suggest_rx) = mpsc::unbounded_channel::<String>();
    // In-TUI OAuth login (`/connect`): flow progress → event loop.
    let (auth_tx, mut auth_rx) = mpsc::unbounded_channel::<AuthFlowEvent>();

    // Background jobs / schedule enqueue use the same long-lived event channel.
    agent.wire_event_sink(event_tx.clone());

    let mut rt = SessionRuntime::new(
        agent,
        session,
        history,
        event_tx,
        event_rx,
        done_tx,
        done_rx,
        perm_prompter,
        question_prompter,
        perm_rx,
        question_rx,
    );

    // S2: background sessions. `rt` is always the ACTIVE session — the loop
    // body below is unchanged from the single-session design. Switching
    // swaps `rt` with `runtimes[idx]` (plus the TuiApp view snapshot), so
    // background turns keep running on their own channels and are drained
    // into their own view snapshots each iteration.
    let mut runtimes: Vec<SessionRuntime> = Vec::new();
    let mut mru: Vec<usize> = Vec::new();

    // When the user first hit Esc / [stop]. After CANCEL_FORCE_AFTER we
    // abort the join handle so "Cancelling…" can never stick forever.
    let mut cancel_requested_at: Option<Instant> = None;
    let mut spinner_frame: usize = 0;
    // Title may arrive before TurnOutcome restores the real rt.session; hold it.
    let mut pending_async_title: Option<(String, String)> = None;

    // Keep home empty so the brand logo shows (no system spam).
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
        'main: loop {
            // ── Drain background sessions into their own view snapshots ──
            // Events on inactive runtimes never touch `app`; they update the
            // runtime's snapshot + state and set `unread` so the dashboard
            // and cycle keys can surface activity.
            for bg in runtimes.iter_mut() {
                drain_background_runtime(bg);
            }
            // Dashboard open: keep rows live as background state changes.
            if matches!(app.dialogs.active(), Some(DialogKind::Sessions)) {
                let cursor = app.sessions_cursor;
                refresh_sessions_rows(&mut app, &rt, &runtimes);
                app.sessions_cursor = cursor.min(app.sessions_rows.len().saturating_sub(1));
                app.mark_dirty();
            }
            // Session picker open: keep the live section at the top fresh.
            if matches!(app.dialogs.active(), Some(DialogKind::SessionList)) {
                refresh_picker_live_section(&mut app, &rt, &runtimes);
            }

            // Expire toasts before drawing, so one never lingers a frame past
            // its time.
            if app.toasts.prune(std::time::Instant::now()) {
                app.mark_dirty();
            }

            // `@file` picker: adopt matcher results published by the index
            // worker threads (async fuzzy — keystrokes never block).
            if app.file_suggest.poll_matches() {
                app.mark_dirty();
            }

            // Animation paths that do not arrive as terminal events still need
            // periodic paints (spinner, toast stack). Idle with a clean flag
            // skips the draw entirely → 0 idle redraws/s.
            let animate = rt.agent_busy || !app.toasts.is_empty();
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
                    // After first paint: MCP connect + code RAG auto-index.
                    // Both can block; doing them here keeps startup feel snappy.
                    rt.agent.load_mcp(&config).await;
                    maybe_session_auto_index(&project_dir, &config, &mut app);
                    refresh_sidebar(&mut app, &config, &file_index);
                    if !app.toasts.is_empty() {
                        app.mark_dirty();
                    }
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

            // ── Stream events from rt.agent (coalesce text/thinking deltas) ──
            if drain_turn_events(&mut app, &mut rt.event_rx) {
                if app.sidebar.visible {
                    refresh_sidebar(&mut app, &config, &file_index);
                }
                app.mark_dirty();
            }

            // Spinner while generating (generic status only)
            if rt.agent_busy
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
            while let Ok(req) = rt.perm_rx.try_recv() {
                rt.pending_perm_queue.push_back(req);
            }
            if !matches!(
                app.current_agent_state,
                AgentState::WaitingForPermission | AgentState::WaitingForQuestion
            ) && let Some(front) = rt.pending_perm_queue.front()
            {
                app.ask_permission(front.tool_name.clone(), front.detail.clone());
                app.mark_dirty();
            }

            // ── Question requests (queued; one questionnaire at a time) ─
            while let Ok(req) = rt.question_rx.try_recv() {
                rt.pending_question_queue.push_back(req);
            }
            if !matches!(
                app.current_agent_state,
                AgentState::WaitingForPermission | AgentState::WaitingForQuestion
            ) && let Some(front) = rt.pending_question_queue.front()
            {
                app.ask_question(front.questions.clone());
                app.mark_dirty();
            }

            // ── Async title refine (does not hold rt.agent_busy) ─────────
            while let Ok((sid, title)) = title_rx.try_recv() {
                if rt.session.id == sid {
                    if rt.session.apply_generated_title(&title) {
                        app.session_title = rt.session.title.clone();
                        rt.persist("title_async");
                        app.mark_dirty();
                    }
                } else if let Some(bg) = runtimes.iter_mut().find(|b| b.session.id == sid) {
                    // Title belongs to a parked session — apply + persist there.
                    if bg.session.apply_generated_title(&title) {
                        bg.view.session_title = bg.session.title.clone();
                        bg.unread = true;
                        bg.persist("title_async");
                    }
                } else {
                    // Turn still restoring rt.session, or user switched sessions.
                    pending_async_title = Some((sid, title));
                }
            }

            // ── Force-stop if cancel is ignored too long ──────────────
            // Cooperative cancel covers stream/tools via select!. This is the
            // hard backstop for spawn_blocking shells / wedged HTTP that never
            // yield: abort the join handle and restore rt.agent/rt.session.
            if rt.agent_busy
                && let Some(since) = cancel_requested_at
            {
                let force = since.elapsed() >= CANCEL_FORCE_AFTER || app.pending_cancel; // second [stop] while cancelling
                if force {
                    app.pending_cancel = false;
                    force_stop_turn(
                        &mut app,
                        &mut rt.agent,
                        &mut rt.session,
                        &mut rt.agent_busy,
                        &mut rt.cancel_flag,
                        &mut cancel_requested_at,
                        &mut rt.turn_join,
                        &mut rt.session_backup,
                        &mut rt.pending_question_queue,
                        &mut rt.pending_perm_queue,
                        &mut rt.done_rx,
                        &config,
                        &project_dir,
                        &provider,
                        &model,
                        rt.event_tx.clone(),
                        Arc::clone(&rt.perm_prompter),
                        Arc::clone(&rt.question_prompter),
                        &file_index,
                    );
                }
            }

            // ── Turn finished ─────────────────────────────────────────
            if let Ok(outcome) = rt.done_rx.try_recv() {
                rt.agent_busy = false;
                rt.cancel_flag = None;
                cancel_requested_at = None;
                rt.turn_join = None;
                rt.session_backup = None;
                // One deferred catalog pass if we still lack a live window.
                // Avoid re-fetching after every turn (would race fast follow-ups).
                if app.api_context_window.is_none() {
                    catalog_fetch_pending = true;
                }
                // Stamp "Worked for Xs" from work_ms only (title refine is async
                // and never included — see spawn_title_refine below).
                let work_ms = match &outcome {
                    TurnOutcome::Ok { work_ms, .. }
                    | TurnOutcome::Err { work_ms, .. }
                    | TurnOutcome::Remote { work_ms, .. } => *work_ms,
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
                        rt.agent = a;
                        rt.session = s;
                        // Apply a title that raced ahead of this restore.
                        if let Some((sid, title)) = pending_async_title.take()
                            && rt.session.id == sid
                        {
                            let _ = rt.session.apply_generated_title(&title);
                        }
                        app.session_title = rt.session.title.clone();
                        // Ensure final text is present
                        if !text.is_empty()
                            && let Some(last) = app.messages.last_mut()
                            && last.role == ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            last.content = text.clone();
                        }
                        // Auto-retain runs inside Agent::run_turn (heuristic + LLM).
                        // Surface rt.agent Status events as toasts already via drain path.
                        app.finish_open_thinking();
                        app.current_agent_state = AgentState::Idle;
                        // Silent providers leave turn_usage empty — estimate fill
                        // from the transcript so the footer is not stuck at 0.
                        if app.turn_usage.is_none() {
                            app.sync_context_estimate(&rt.session);
                        }
                        // Agent may have switched branches; keep footer current.
                        app.refresh_git_branch();
                        app.status_message = format_turn_done_status(
                            &app,
                            rt.agent.info.name.as_str(),
                            &provider,
                            &model,
                            elapsed_ms,
                            false,
                        );
                        // Persist after every completed turn (success path).
                        rt.persist("ok");
                        // A7: optional idle follow-up suggestion (default off).
                        maybe_spawn_prompt_suggestion(
                            &config,
                            &rt.session,
                            &provider,
                            &model,
                            &api_key,
                            &mut app,
                            suggest_tx.clone(),
                        );
                    }
                    TurnOutcome::Remote {
                        text,
                        error,
                        work_ms: _,
                    } => {
                        app.finish_open_thinking();
                        if let Some(err) = error {
                            app.current_agent_state = AgentState::Idle;
                            app.add_message(ChatRole::System, format!("Remote error: {err}"));
                            app.status_message = "remote error".into();
                        } else {
                            if !text.is_empty()
                                && let Some(last) = app.messages.last_mut()
                                && last.role == ChatRole::Assistant
                                && last.content.is_empty()
                            {
                                last.content = text.clone();
                            }
                            app.current_agent_state = AgentState::Idle;
                            app.status_message = format_turn_done_status(
                                &app,
                                rt.agent.info.name.as_str(),
                                &provider,
                                &model,
                                elapsed_ms,
                                false,
                            );
                        }
                    }
                    TurnOutcome::Err {
                        error,
                        agent: a,
                        session: s,
                        cancelled,
                        work_ms: _,
                    } => {
                        rt.agent = a;
                        rt.session = s;
                        if let Some((sid, title)) = pending_async_title.take()
                            && rt.session.id == sid
                        {
                            let _ = rt.session.apply_generated_title(&title);
                        }
                        app.session_title = rt.session.title.clone();
                        app.finish_open_thinking();
                        // Transcript may have partial assistant text; refresh estimate
                        // when no provider usage landed this turn.
                        if app.turn_usage.is_none() {
                            app.sync_context_estimate(&rt.session);
                        }
                        if cancelled {
                            app.current_agent_state = AgentState::Idle;
                            app.status_message = format_turn_done_status(
                                &app,
                                rt.agent.info.name.as_str(),
                                &provider,
                                &model,
                                elapsed_ms,
                                true,
                            );
                            app.add_message(ChatRole::System, "⏹ Generation cancelled (Esc).");
                            // Still flush so cancel mid-turn is not lost on crash.
                            rt.persist("cancelled");
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
                            app.toasts
                                .push(crate::toast::ToastKind::Error, truncate_toast(&display, 48));
                            whycode_core::logging::emit_sid(
                                "tui",
                                "error",
                                "turn.error",
                                Some(rt.session.id.as_str()),
                                Some(serde_json::json!({
                                    "error": error,
                                    "display": display,
                                    "elapsed_ms": elapsed_ms,
                                })),
                            );
                            rt.persist("error");
                        }
                    }
                }
            }

            // ── Apply rt.session picker / /resume selection ──────────────
            // ── Picker close selection (Ctrl+W on a live row) ────────
            if let Some(close_idx) = app.session_list.pending_close.take() {
                if close_idx == usize::MAX {
                    // Closing the ACTIVE session: only when idle and others
                    // exist — park nothing, switch to the most recent parked.
                    if rt.agent_busy {
                        app.toasts.push(
                            crate::toast::ToastKind::Warning,
                            "Turn in flight — Esc first, then close",
                        );
                    } else if runtimes.is_empty() {
                        app.toasts.push(
                            crate::toast::ToastKind::Info,
                            "Last live session stays open",
                        );
                    } else {
                        rt.persist("close");
                        let idx = mru.pop().unwrap_or(runtimes.len() - 1);
                        let idx = idx.min(runtimes.len() - 1);
                        let mut closed = std::mem::replace(&mut rt, runtimes.remove(idx));
                        // The closed runtime's turn guard is idle; drop it.
                        closed.turn_join.take();
                        mru.retain(|&i| i != idx);
                        for i in mru.iter_mut() {
                            if *i > idx {
                                *i -= 1;
                            }
                        }
                        rt.unread = false;
                        app.restore_view(&rt.view);
                        app.focus = FocusPane::Prompt;
                        app.toasts.push(
                            crate::toast::ToastKind::Info,
                            format!(
                                "Closed · now {} ({} live)",
                                rt.session.title,
                                runtimes.len() + 1
                            ),
                        );
                    }
                } else if close_idx < runtimes.len() {
                    // Closing a PARKED session: deny waiters, abort its turn,
                    // persist, drop the runtime.
                    let mut bg = runtimes.remove(close_idx);
                    while let Some(req) = bg.pending_perm_queue.pop_front() {
                        let _ = req.reply.send(false);
                    }
                    while let Some(req) = bg.pending_question_queue.pop_front() {
                        let _ = req.reply.send(Err(QuestionError::Cancelled));
                    }
                    if let Some(h) = bg.turn_join.take() {
                        h.abort();
                    }
                    bg.agent.background_registry().kill_all();
                    bg.persist("close");
                    mru.retain(|&i| i != close_idx);
                    for i in mru.iter_mut() {
                        if *i > close_idx {
                            *i -= 1;
                        }
                    }
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        format!(
                            "Closed · {} ({} live)",
                            bg.session.title,
                            runtimes.len() + 1
                        ),
                    );
                }
            }

            // ── Dashboard switch selection ──────────────────────────
            if let Some(target) = app.pending_session_switch.take()
                && target != usize::MAX
                && target < runtimes.len()
            {
                switch_to_runtime(&mut app, &mut rt, &mut runtimes, target);
                mru.retain(|&i| i != target);
                mru.push(target);
                app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!(
                        "Session · {} ({} live)",
                        rt.session.title,
                        runtimes.len() + 1
                    ),
                );
            }

            if let Some(id) = app.pending_session_id.take() {
                // Don't switch mid-turn — re-queue and wait.
                if rt.agent_busy {
                    app.pending_session_id = Some(id);
                } else if let Some(idx) = runtimes.iter().position(|b| b.session.id == id) {
                    // Already live in a parked session — just switch to it.
                    switch_to_runtime(&mut app, &mut rt, &mut runtimes, idx);
                    mru.retain(|&i| i != idx);
                    mru.push(idx);
                    app.toasts.push(
                        crate::toast::ToastKind::Success,
                        format!("Switched to live session · {}", rt.session.title),
                    );
                } else {
                    match try_load_session(&id) {
                        Ok(Some(loaded)) => {
                            // Persist the current rt.session first so nothing is lost
                            // when switching away from an unsaved turn.
                            if !rt.session.messages.is_empty() {
                                rt.persist("switch");
                            }
                            let n = loaded.messages.len();
                            rt.history = SessionHistory::new();
                            rt.session = loaded;
                            rt.session.system_prompt = with_project_memory(
                                &Agent::with_agents_md(&rt.agent.system_prompt(), &project_dir),
                                &project_dir,
                                &config,
                                None,
                            );
                            if config.session.auto_title
                                && rt.session.maybe_upgrade_title_from_history()
                            {
                                rt.persist("title_backfill");
                            }
                            let title = rt.session.title.clone();
                            app.load_messages_from_session(&rt.session);
                            app.toasts.push(
                                crate::toast::ToastKind::Success,
                                format!("Resumed · {title} ({n} msgs)"),
                            );
                            app.status_message =
                                format!("Resumed rt.session {}", short_session_id(&rt.session.id));
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
                    whycode_llm::oauth_refresh::unregister(&p);
                } else if whycode_auth::providers::supports_oauth(&p)
                    && let Ok(dir) = Config::data_dir()
                    && let Some(tok) = whycode_auth::providers::access_token(&p, &dir).await
                {
                    // OAuth subscription login (`whycode auth login <p>`).
                    whycode_llm::oauth_refresh::register(&p, dir);
                    api_key = tok;
                }
                // Drop stale window; re-fetch when idle so we never contend with a turn.
                app.clear_api_context_window();
                if rt.agent_busy {
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

            // ── `/login` picker selection → start OAuth sign-in ──
            if let Some(p) = app.pending_login_provider.take()
                && let Ok(dir) = Config::data_dir()
            {
                spawn_oauth_login(&mut app, &auth_tx, dir, &p);
            }

            // ── Re-fetch when slash /models switches provider ──
            if app.pending_catalog_refresh {
                app.pending_catalog_refresh = false;
                app.clear_api_context_window();
                if rt.agent_busy {
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
                && !rt.agent_busy
                && app.pending_prompt.is_none()
                && !missing_key
            {
                catalog_fetch_pending = false;
                spawn_model_context_fetch(&config, &provider, &model, &api_key, catalog_tx.clone());
            }

            // A7 suggestion results
            while let Ok(suggestion) = suggest_rx.try_recv() {
                if !suggestion.trim().is_empty() && !rt.agent_busy {
                    app.pending_suggestion = Some(suggestion.clone());
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        truncate_toast(&format!("suggest · Tab · {suggestion}"), 64),
                    );
                    app.mark_dirty();
                }
            }

            // OAuth login flow progress (`/connect` spawned task).
            while let Ok(ev) = auth_rx.try_recv() {
                match ev {
                    AuthFlowEvent::Note(text) => {
                        app.add_message(ChatRole::System, &text);
                        app.status_message = text.lines().next().unwrap_or("").to_string();
                    }
                    AuthFlowEvent::NeedCode(sink) => {
                        app.auth_code_sink = Some(sink);
                        app.status_message =
                            "Paste the sign-in code, then Enter · Esc cancels".into();
                        app.focus_prompt();
                    }
                    AuthFlowEvent::Done {
                        provider: p,
                        result,
                    } => match result {
                        Ok(_) => {
                            // Load the fresh credential for the active provider.
                            if p == provider
                                && let Ok(dir) = Config::data_dir()
                                && let Some(tok) =
                                    whycode_auth::providers::access_token(&p, &dir).await
                            {
                                whycode_llm::oauth_refresh::register(&p, dir);
                                api_key = tok;
                            }
                            let hint = if p == provider {
                                String::new()
                            } else {
                                format!(" — switch with /models {p}/<model>")
                            };
                            app.add_message(
                                ChatRole::System,
                                format!("✓ Signed in to `{p}` (subscription){hint}"),
                            );
                            app.status_message = format!("Signed in · {p}");
                            app.toasts
                                .push(crate::toast::ToastKind::Success, format!("Connected · {p}"));
                        }
                        Err(msg) => {
                            app.add_message(
                                ChatRole::System,
                                format!("Sign-in to `{p}` failed: {msg}"),
                            );
                            app.status_message = format!("sign-in failed · {p}");
                            app.toasts.push(
                                crate::toast::ToastKind::Error,
                                truncate_toast(&format!("sign-in failed: {msg}"), 64),
                            );
                        }
                    },
                }
                app.mark_dirty();
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
            // Second click while already cancelling → immediate force-stop.
            if rt.agent_busy && app.pending_cancel {
                app.pending_cancel = false;
                if cancel_requested_at.is_some() {
                    force_stop_turn(
                        &mut app,
                        &mut rt.agent,
                        &mut rt.session,
                        &mut rt.agent_busy,
                        &mut rt.cancel_flag,
                        &mut cancel_requested_at,
                        &mut rt.turn_join,
                        &mut rt.session_backup,
                        &mut rt.pending_question_queue,
                        &mut rt.pending_perm_queue,
                        &mut rt.done_rx,
                        &config,
                        &project_dir,
                        &provider,
                        &model,
                        rt.event_tx.clone(),
                        Arc::clone(&rt.perm_prompter),
                        Arc::clone(&rt.question_prompter),
                        &file_index,
                    );
                } else {
                    begin_cancel(
                        &mut app,
                        &rt.cancel_flag,
                        &mut cancel_requested_at,
                        &mut rt.pending_question_queue,
                        &mut rt.pending_perm_queue,
                    );
                }
            }

            // Drain scheduled /loop prompts when idle (no pending manual submit).
            if !rt.agent_busy
                && app.pending_prompt.is_none()
                && let Some(next) = app.pending_auto_prompts.pop_front()
            {
                app.pending_prompt = Some(next);
            }

            // ── Start turn if needed ──────────────────────────────────
            if !rt.agent_busy
                && let Some(prompt) = app.pending_prompt.take()
            {
                let submit_images = std::mem::take(&mut app.pending_submit_images);

                if let Some(ref rem) = remote {
                    drop(submit_images);
                    let expanded = expand_at_files(&prompt, &project_dir);
                    rt.session.add_user_message(&expanded);
                    rt.agent_busy = true;
                    let flag = new_cancel_flag();
                    rt.cancel_flag = Some(Arc::clone(&flag));
                    cancel_requested_at = None;
                    app.mark_turn_started();
                    app.current_agent_state = AgentState::Generating;
                    app.status_message = "remote…".into();
                    if app
                        .messages
                        .last()
                        .map(|m| m.role != ChatRole::Assistant)
                        .unwrap_or(true)
                    {
                        app.add_message(ChatRole::Assistant, "");
                    }
                    let rem = rem.clone();
                    let event_tx2 = rt.event_tx.clone();
                    let done_tx2 = rt.done_tx.clone();
                    rt.turn_join = Some(tokio::spawn(async move {
                        let t0 = std::time::Instant::now();
                        let result =
                            crate::remote::stream_chat(&rem, &expanded, event_tx2, Some(flag))
                                .await;
                        let work_ms = t0.elapsed().as_millis();
                        match result {
                            Ok(text) => {
                                if let Err(e) = done_tx2.send(TurnOutcome::Remote {
                                    text,
                                    error: None,
                                    work_ms,
                                }) {
                                    tracing::debug!(error = %e, "remote turn done dropped");
                                }
                            }
                            Err(e) => {
                                if let Err(send_err) = done_tx2.send(TurnOutcome::Remote {
                                    text: String::new(),
                                    error: Some(e.to_string()),
                                    work_ms,
                                }) {
                                    tracing::debug!(error = %send_err, "remote turn err dropped");
                                }
                            }
                        }
                    }));
                    continue;
                }

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

                rt.agent_busy = true;
                let flag = new_cancel_flag();
                rt.cancel_flag = Some(Arc::clone(&flag));
                cancel_requested_at = None;
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
                rt.history
                    .push_before_turn(&rt.session.messages, &project_dir);
                // Auto-recall memories relevant to this user turn (Grok/jcode style).
                refresh_session_memory(
                    &mut rt.session,
                    &rt.agent,
                    &project_dir,
                    &config,
                    Some(&expanded),
                );
                if submit_images.is_empty() {
                    rt.session.add_user_message(&expanded);
                } else {
                    match crate::images::build_user_blocks(&expanded, &submit_images) {
                        Ok(blocks) => rt.session.add_user_message_blocks(blocks),
                        Err(e) => {
                            app.toasts.push(
                                crate::toast::ToastKind::Warning,
                                format!("Image attach failed: {e}"),
                            );
                            if expanded.trim().is_empty() {
                                rt.session
                                    .add_user_message(&format!("(failed to load image: {e})"));
                            } else {
                                rt.session.add_user_message(&expanded);
                            }
                        }
                    }
                }

                // Instant offline title (no API cost). Prefer the transcript's
                // first user message so resumed legacy placeholders name from
                // the original topic, not the latest follow-up.
                if config.session.auto_title {
                    let seed = rt
                        .session
                        .first_user_text()
                        .unwrap_or_else(|| expanded.clone());
                    if rt.session.apply_heuristic_title(&seed) {
                        app.session_title = rt.session.title.clone();
                    }
                }

                // Fast model for trivial chat (selam/hi) — sibling or config.
                let (route_provider, route_model) = whycode_agent::resolve_turn_model(
                    &provider,
                    &model,
                    &expanded,
                    rt.agent
                        .model_fast()
                        .or(config.session.model_fast.as_deref()),
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
                        Some(rt.session.id.as_str()),
                        Some(serde_json::json!({
                            "from": format!("{provider}/{model}"),
                            "to": format!("{route_provider}/{route_model}"),
                        })),
                    );
                }

                let provider2 = route_provider;
                let model2 = route_model;
                let api_key2 = api_key.clone();
                let event_tx2 = rt.event_tx.clone();
                let done_tx2 = rt.done_tx.clone();
                let cancel2 = Some(flag);
                let auto_title = config.session.auto_title;
                let title_model = config.session.title_model.clone();
                let title_tx2 = title_tx.clone();
                // Title refine still uses rt.session provider/model (or title_model).
                let title_provider = provider.clone();
                let title_session_model = model.clone();

                // Move rt.agent + rt.session into background task
                let ag = std::mem::replace(
                    &mut rt.agent,
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
                // Snapshot for force-abort recovery (task owns the live rt.session).
                rt.session_backup = Some(rt.session.clone());
                let sess = std::mem::replace(
                    &mut rt.session,
                    Session::new(project_dir.clone(), String::new()),
                );

                rt.turn_join = Some(tokio::spawn(async move {
                    let agent = ag;
                    let mut session = sess;
                    // Time only the agent loop. Title refine runs async *after*
                    // we release rt.agent_busy so the user can type immediately.
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
                }));
            }

            // ── Input ─────────────────────────────────────────────────
            // How long to wait for a keystroke before looping again.
            //
            // Paint is gated by `needs_redraw` / animation — poll timeout no
            // longer implies a full redraw. Short timeout while the rt.agent is
            // busy or toasts are live so spinner / stream / expiry stay snappy;
            // long timeout when idle so we do not spin the CPU.
            let awaiting_matches = app.file_suggest.awaiting_matches();
            let idle =
                !rt.agent_busy && app.toasts.is_empty() && !app.needs_redraw && !awaiting_matches;
            let poll_for = if awaiting_matches {
                // Fuzzy workers are mid-rematch: stay near frame cadence so
                // results land within ~1 frame of being published.
                Duration::from_millis(16)
            } else if idle {
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
                // Drain the whole pending queue before the next paint. A
                // trackpad flick is dozens of wheel events; handling one per
                // draw made the chat look frozen (each frame re-laid the
                // transcript). Moves alone do not force a redraw — hover
                // chrome still calls mark_dirty when the hit set changes.
                let batch = match read_event_batch() {
                    Ok(b) => b,
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
                let mut batch = batch;
                input::coalesce_chat_wheels(&mut app, &mut batch);
                if batch.iter().any(event_forces_redraw) {
                    app.mark_dirty();
                }

                for ev in batch {
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
                                if let Some(req) = rt.pending_perm_queue.pop_front() {
                                    let _ = req.reply.send(true);
                                }
                                app.dialogs.pop();
                                app.mode = AppMode::Normal;
                                app.key_context = KeymapContext::Normal;
                                if let Some(next) = rt.pending_perm_queue.front() {
                                    app.ask_permission(next.tool_name.clone(), next.detail.clone());
                                    app.status_message = format!(
                                        "Allowed — {} more permission(s)…",
                                        rt.pending_perm_queue.len()
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
                                if let Some(req) = rt.pending_perm_queue.pop_front() {
                                    let _ = req.reply.send(false);
                                }
                                app.dialogs.pop();
                                app.mode = AppMode::Normal;
                                app.key_context = KeymapContext::Normal;
                                if let Some(next) = rt.pending_perm_queue.front() {
                                    app.ask_permission(next.tool_name.clone(), next.detail.clone());
                                    app.status_message = format!(
                                        "Denied — {} more permission(s)…",
                                        rt.pending_perm_queue.len()
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
                            &mut rt.pending_question_queue,
                            &rt.pending_perm_queue,
                        );
                        if handled {
                            app.mark_dirty();
                            continue;
                        }
                    }

                    // Mouse single-select may finish the questionnaire from input.rs
                    if let Some(answers) = app.pending_question_answers.take() {
                        if let Some(req) = rt.pending_question_queue.pop_front() {
                            let _ = req.reply.send(Ok(answers));
                        }
                        if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
                            app.mode = AppMode::Normal;
                            app.key_context = KeymapContext::Normal;
                            resume_after_question(
                                &mut app,
                                &rt.pending_question_queue,
                                &rt.pending_perm_queue,
                            );
                        }
                    }

                    // [✗] / Esc may dismiss Question via input.rs — complete oneshot
                    if app.question_dismissed {
                        app.question_dismissed = false;
                        if let Some(req) = rt.pending_question_queue.pop_front() {
                            let _ = req.reply.send(Err(QuestionError::Cancelled));
                        }
                        if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
                            resume_after_question(
                                &mut app,
                                &rt.pending_question_queue,
                                &rt.pending_perm_queue,
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
                        && !rt.agent_busy
                    {
                        cycle_agent(
                            &mut app,
                            &mut rt.agent,
                            &mut rt.session,
                            &config,
                            &project_dir,
                            Arc::clone(&rt.perm_prompter),
                            Arc::clone(&rt.question_prompter),
                            &rt.event_tx,
                        )
                        .await;
                        continue;
                    }

                    // ── S2: multi-session keys ────────────────────────────
                    // Ctrl+N: park the active session and open a fresh one.
                    if let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Char('n')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                        && app.mode == AppMode::Normal
                    {
                        if runtimes.len() + 1 >= MAX_LIVE_SESSIONS {
                            app.toasts.push(
                                crate::toast::ToastKind::Warning,
                                format!("Session limit ({MAX_LIVE_SESSIONS}) — close one first"),
                            );
                            continue;
                        }
                        app.save_view(&mut rt.view);
                        let parked = std::mem::replace(
                            &mut rt,
                            spawn_new_session_runtime(
                                &app.agent_name,
                                &config,
                                &project_dir,
                                &file_index,
                                session_claims.clone(),
                            )
                            .await,
                        );
                        runtimes.push(parked);
                        mru.push(runtimes.len() - 1);
                        app.restore_view(&rt.view);
                        app.focus = FocusPane::Prompt;
                        app.toasts.push(
                            crate::toast::ToastKind::Info,
                            format!("New session ({} live)", runtimes.len() + 1),
                        );
                        continue;
                    }

                    // Ctrl+PageDown/PageUp: cycle sessions in creation order.
                    if let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                        && matches!(key.code, KeyCode::PageDown | KeyCode::PageUp)
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                        && app.mode == AppMode::Normal
                        && !runtimes.is_empty()
                    {
                        let idx = if key.code == KeyCode::PageDown {
                            0
                        } else {
                            runtimes.len() - 1
                        };
                        switch_to_runtime(&mut app, &mut rt, &mut runtimes, idx);
                        mru.retain(|&i| i != idx);
                        mru.push(idx);
                        app.toasts.push(
                            crate::toast::ToastKind::Info,
                            format!(
                                "Session · {} ({} live)",
                                rt.session.title,
                                runtimes.len() + 1
                            ),
                        );
                        continue;
                    }

                    // Ctrl+O: live-session dashboard (grouped, peek, attach).
                    if let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Char('o')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                        && app.mode == AppMode::Normal
                    {
                        open_sessions_dashboard(&mut app, &rt, &runtimes);
                        continue;
                    }

                    // Ctrl+Tab: MRU switch to the most recently parked session.
                    if let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Tab
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                        && app.mode == AppMode::Normal
                        && !runtimes.is_empty()
                    {
                        let idx = mru.pop().unwrap_or(runtimes.len() - 1);
                        let idx = idx.min(runtimes.len() - 1);
                        switch_to_runtime(&mut app, &mut rt, &mut runtimes, idx);
                        mru.retain(|&i| i != idx);
                        mru.push(idx);
                        app.toasts.push(
                            crate::toast::ToastKind::Info,
                            format!(
                                "Session · {} ({} live)",
                                rt.session.title,
                                runtimes.len() + 1
                            ),
                        );
                        continue;
                    }

                    // Slash commands on Enter
                    if let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Enter
                        && app.mode == AppMode::Normal
                        && !rt.agent_busy
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
                                    session: &mut rt.session,
                                    history: &mut rt.history,
                                    agent: &mut rt.agent,
                                    config: &config,
                                    project_dir: &project_dir,
                                    provider: &mut provider,
                                    model: &mut model,
                                    api_key: &mut api_key,
                                    perm_prompter: Arc::clone(&rt.perm_prompter),
                                    question_prompter: Arc::clone(&rt.question_prompter),
                                    auth_tx: auth_tx.clone(),
                                },
                            )
                            .await;
                            continue;
                        }
                    }

                    // While busy: Esc cancels (draft preserved — Grok). Typing, scroll,
                    // and focus still work so the user can queue thoughts.
                    if rt.agent_busy
                        && let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                    {
                        match key.code {
                            KeyCode::Esc => {
                                // First Esc: cooperative cancel. Second: force-stop now.
                                if cancel_requested_at.is_some() {
                                    force_stop_turn(
                                        &mut app,
                                        &mut rt.agent,
                                        &mut rt.session,
                                        &mut rt.agent_busy,
                                        &mut rt.cancel_flag,
                                        &mut cancel_requested_at,
                                        &mut rt.turn_join,
                                        &mut rt.session_backup,
                                        &mut rt.pending_question_queue,
                                        &mut rt.pending_perm_queue,
                                        &mut rt.done_rx,
                                        &config,
                                        &project_dir,
                                        &provider,
                                        &model,
                                        rt.event_tx.clone(),
                                        Arc::clone(&rt.perm_prompter),
                                        Arc::clone(&rt.question_prompter),
                                        &file_index,
                                    );
                                } else {
                                    begin_cancel(
                                        &mut app,
                                        &rt.cancel_flag,
                                        &mut cancel_requested_at,
                                        &mut rt.pending_question_queue,
                                        &mut rt.pending_perm_queue,
                                    );
                                }
                                app.esc_armed_at = None;
                                continue;
                            }
                            KeyCode::Char('q')
                                if key
                                    .modifiers
                                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
                            {
                                // Quit: always force-stop so we never hang on exit.
                                if rt.agent_busy {
                                    force_stop_turn(
                                        &mut app,
                                        &mut rt.agent,
                                        &mut rt.session,
                                        &mut rt.agent_busy,
                                        &mut rt.cancel_flag,
                                        &mut cancel_requested_at,
                                        &mut rt.turn_join,
                                        &mut rt.session_backup,
                                        &mut rt.pending_question_queue,
                                        &mut rt.pending_perm_queue,
                                        &mut rt.done_rx,
                                        &config,
                                        &project_dir,
                                        &provider,
                                        &model,
                                        rt.event_tx.clone(),
                                        Arc::clone(&rt.perm_prompter),
                                        Arc::clone(&rt.question_prompter),
                                        &file_index,
                                    );
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
                                // Empty draft + already cancelling → force-stop.
                                if app.input_buffer.is_empty() {
                                    if cancel_requested_at.is_some() {
                                        force_stop_turn(
                                            &mut app,
                                            &mut rt.agent,
                                            &mut rt.session,
                                            &mut rt.agent_busy,
                                            &mut rt.cancel_flag,
                                            &mut cancel_requested_at,
                                            &mut rt.turn_join,
                                            &mut rt.session_backup,
                                            &mut rt.pending_question_queue,
                                            &mut rt.pending_perm_queue,
                                            &mut rt.done_rx,
                                            &config,
                                            &project_dir,
                                            &provider,
                                            &model,
                                            rt.event_tx.clone(),
                                            Arc::clone(&rt.perm_prompter),
                                            Arc::clone(&rt.question_prompter),
                                            &file_index,
                                        );
                                    } else {
                                        begin_cancel(
                                            &mut app,
                                            &rt.cancel_flag,
                                            &mut cancel_requested_at,
                                            &mut rt.pending_question_queue,
                                            &mut rt.pending_perm_queue,
                                        );
                                    }
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
                        break 'main;
                    }
                } // for ev in batch
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

    // Deny any hanging permissions / questionnaires so rt.agent tasks can finish
    while let Some(req) = rt.pending_perm_queue.pop_front() {
        let _ = req.reply.send(false);
    }
    while let Some(req) = rt.pending_question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
    }
    // Best-effort: stop background shells so we don't leave orphans.
    rt.agent.background_registry().kill_all();

    // Parked sessions: deny their waiters, abort their turns, persist.
    for mut bg in runtimes.drain(..) {
        while let Some(req) = bg.pending_perm_queue.pop_front() {
            let _ = req.reply.send(false);
        }
        while let Some(req) = bg.pending_question_queue.pop_front() {
            let _ = req.reply.send(Err(QuestionError::Cancelled));
        }
        if let Some(h) = bg.turn_join.take() {
            h.abort();
        }
        bg.agent.background_registry().kill_all();
        bg.persist("shutdown");
    }

    if let Err(ref e) = result {
        whycode_core::logging::emit(
            "whycode_tui",
            "error",
            "tui.loop_error",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
    }

    // Cleanup must not fail the process after a successful rt.session — best-effort.
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
    rt.persist("exit");
    let model_label = format!("{provider}/{model}");
    let summary =
        rt.session
            .format_exit_summary(session_started.elapsed(), &model_label, "whycode");
    print_session_summary(&summary);

    whycode_core::logging::emit(
        "whycode_tui",
        "info",
        "tui.stopped",
        Some(serde_json::json!({
            "ok": result.is_ok(),
            "session_id": rt.session.id,
            "messages": rt.session.messages.len(),
            "duration_s": session_started.elapsed().as_secs(),
        })),
    );

    result
}

/// Read the event that woke `poll`, then drain anything already queued.
///
/// Cap the batch so a stuck input flood cannot grow without bound before
/// the next paint / turn-event drain.
fn read_event_batch() -> io::Result<Vec<Event>> {
    const MAX_BATCH: usize = 256;
    let mut batch = Vec::with_capacity(8);
    batch.push(event::read()?);
    while batch.len() < MAX_BATCH {
        match event::poll(Duration::ZERO) {
            Ok(true) => batch.push(event::read()?),
            Ok(false) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(batch)
}

/// Mouse motion is tracked for hover; it must not by itself schedule a
/// full chat paint (handle_mouse marks dirty only when chrome hover changes).
fn event_forces_redraw(ev: &Event) -> bool {
    !matches!(
        ev,
        Event::Mouse(m) if m.kind == MouseEventKind::Moved
    )
}

/// Print the exit summary where the user will see it after alt-screen leave.
///
/// The TUI draws on `/dev/tty`; after restore, prefer that same device so the
/// lines land in the real terminal scrollback even when stdout is captured.
/// Fall back to stdout, then stderr.
fn print_session_summary(summary: &str) {
    #[cfg(unix)]
    {
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty")
            && writeln!(tty, "{summary}").is_ok()
            && tty.flush().is_ok()
        {
            return;
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

/// Arm cooperative cancel: set the flag, unblock permission/question waits,
/// and start the force-stop timer.
fn begin_cancel(
    app: &mut TuiApp,
    cancel_flag: &Option<CancelFlag>,
    cancel_requested_at: &mut Option<Instant>,
    pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &mut std::collections::VecDeque<whycode_agent::PermissionRequest>,
) {
    if let Some(flag) = cancel_flag.as_ref() {
        request_cancel(flag);
    }
    // Unblock any interactive wait so the agent can observe cancel promptly.
    while let Some(req) = pending_question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
    }
    while let Some(req) = pending_perm_queue.pop_front() {
        // Deny — tool layer treats false as "user refused".
        let _ = req.reply.send(false);
    }
    if cancel_requested_at.is_none() {
        *cancel_requested_at = Some(Instant::now());
    }
    app.status_message = "Cancelling…".into();
    app.current_agent_state = AgentState::Generating;
    app.finish_open_thinking();
    app.mark_dirty();
}

/// Hard-stop: abort the turn task, restore agent/session, free the UI.
///
/// Called after [`CANCEL_FORCE_AFTER`] or on a second Esc/[stop] while already
/// cancelling. Guarantees `rt.agent_busy` becomes false.
#[allow(clippy::too_many_arguments)]
fn force_stop_turn(
    app: &mut TuiApp,
    agent: &mut Agent,
    session: &mut Session,
    agent_busy: &mut bool,
    cancel_flag: &mut Option<CancelFlag>,
    cancel_requested_at: &mut Option<Instant>,
    turn_join: &mut Option<tokio::task::JoinHandle<()>>,
    session_backup: &mut Option<Session>,
    pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &mut std::collections::VecDeque<whycode_agent::PermissionRequest>,
    done_rx: &mut mpsc::UnboundedReceiver<TurnOutcome>,
    config: &Config,
    project_dir: &std::path::Path,
    provider: &str,
    model: &str,
    event_tx: mpsc::UnboundedSender<TurnEvent>,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    file_index: &Arc<whycode_index::WorkspaceIndex>,
) {
    // Always re-signal cancel in case the task is still cooperative.
    if let Some(flag) = cancel_flag.as_ref() {
        request_cancel(flag);
    }
    while let Some(req) = pending_question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
    }
    while let Some(req) = pending_perm_queue.pop_front() {
        let _ = req.reply.send(false);
    }

    if let Some(h) = turn_join.take() {
        h.abort();
    }

    // If the task finished in the race window, prefer its restored agent/session.
    let mut got_outcome = false;
    while let Ok(outcome) = done_rx.try_recv() {
        got_outcome = true;
        match outcome {
            TurnOutcome::Ok {
                agent: a,
                session: s,
                ..
            }
            | TurnOutcome::Err {
                agent: a,
                session: s,
                ..
            } => {
                *agent = a;
                *session = s;
            }
            TurnOutcome::Remote { .. } => {}
        }
    }

    if !got_outcome {
        // Task dropped without returning — rebuild agent; restore session snapshot.
        if let Some(backup) = session_backup.take() {
            *session = backup;
        }
        let preferred = if app.agent_name.is_empty() {
            agent.info.name.clone()
        } else {
            app.agent_name.clone()
        };
        rebuild_agent_after_force_stop(
            agent,
            session,
            config,
            project_dir,
            &preferred,
            event_tx,
            perm_prompter,
            question_prompter,
            file_index,
        );
    } else {
        session_backup.take();
    }

    *agent_busy = false;
    *cancel_flag = None;
    *cancel_requested_at = None;

    app.finish_open_thinking();
    app.current_agent_state = AgentState::Idle;
    app.status_message = format_turn_done_status(
        app,
        agent.info.name.as_str(),
        provider,
        model,
        app.turn_elapsed_ms(),
        true,
    );
    // Avoid duplicate system lines if cooperative cancel already announced.
    let already = app
        .messages
        .last()
        .map(|m| m.role == ChatRole::System && m.content.contains("cancelled"))
        .unwrap_or(false);
    if !already {
        app.add_message(ChatRole::System, "⏹ Stopped.");
    }
    persist_session_best_effort(session, "force_cancelled");
    app.mark_dirty();
}

#[allow(clippy::too_many_arguments)]
fn rebuild_agent_after_force_stop(
    agent: &mut Agent,
    session: &mut Session,
    config: &Config,
    project_dir: &std::path::Path,
    preferred_name: &str,
    event_tx: mpsc::UnboundedSender<TurnEvent>,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    file_index: &Arc<whycode_index::WorkspaceIndex>,
) {
    let name = if preferred_name.is_empty() || preferred_name == "_pending" {
        if config.default_agent.is_empty() {
            "build".into()
        } else {
            config.default_agent.clone()
        }
    } else {
        preferred_name.to_string()
    };
    let info = config
        .get_agent(&name)
        .cloned()
        .unwrap_or_else(|| whycode_core::types::AgentInfo {
            name: name.clone(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycode_core::types::PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        });
    let base = info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&info.name));
    let prompt = with_project_memory(
        &Agent::with_agents_md(&base, project_dir),
        project_dir,
        config,
        None,
    );
    let bg = agent.background_registry().clone();
    let claims = agent.session_claims();
    let mut next = Agent::new(info)
        .with_config(config)
        .with_background_registry(bg)
        .with_file_index(file_index.clone())
        .with_permission_prompter(perm_prompter as Arc<dyn whycode_agent::PermissionPrompter>)
        .with_question_prompter(question_prompter as Arc<dyn QuestionPrompter>);
    if let Some(c) = claims {
        next = next.with_session_claims(c);
    }
    *agent = next;
    agent.wire_event_sink(event_tx);
    // Keep existing system prompt on session if any; else set rebuilt one.
    if session.system_prompt.is_empty() {
        session.set_system_prompt(&prompt);
    }
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

/// Build a fresh runtime for a new empty session (Ctrl+N). Owns its
/// prompter pair and channels; the agent shares no state with any other
/// runtime except the process-wide background registry pattern.
async fn spawn_new_session_runtime(
    agent_name: &str,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycode_index::WorkspaceIndex>,
    session_claims: whycode_core::FileClaimRegistry,
) -> SessionRuntime {
    let agent_info =
        config
            .get_agent(agent_name)
            .cloned()
            .unwrap_or_else(|| whycode_core::types::AgentInfo {
                name: agent_name.to_string(),
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
        .unwrap_or_else(|| Agent::system_prompt_for(agent_name));
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base, project_dir),
        project_dir,
        config,
        None,
    );

    let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
    let perm_prompter: Arc<ChannelPermissionPrompter> = Arc::new(perm_prompter);
    let q_timeout = if config.tools.question.timeout_enabled {
        Some(Duration::from_secs(
            config.tools.question.timeout_secs.max(1),
        ))
    } else {
        None
    };
    let (question_prompter, question_rx) = ChannelQuestionPrompter::new(q_timeout);
    let question_prompter: Arc<ChannelQuestionPrompter> = Arc::new(question_prompter);

    let agent = Agent::new(agent_info)
        .with_config(config)
        .with_file_index(file_index.clone())
        .with_session_claims(session_claims)
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
        )
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>)
        .with_mcp(config)
        .await;

    let session = Session::new(project_dir.to_path_buf(), system_prompt);
    let history = SessionHistory::new();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<TurnEvent>();
    let (done_tx, done_rx) = mpsc::unbounded_channel::<TurnOutcome>();

    SessionRuntime::new(
        agent,
        session,
        history,
        event_tx,
        event_rx,
        done_tx,
        done_rx,
        perm_prompter,
        question_prompter,
        perm_rx,
        question_rx,
    )
}

/// Refresh the dashboard row snapshot from the live runtimes and open the
/// dashboard dialog. Rows are grouped: needs-input → working → idle.
fn open_sessions_dashboard(app: &mut TuiApp, rt: &SessionRuntime, runtimes: &[SessionRuntime]) {
    refresh_sessions_rows(app, rt, runtimes);
    if !matches!(app.dialogs.active(), Some(DialogKind::Sessions)) {
        app.sessions_cursor = 0;
        app.dialogs.push(DialogKind::Sessions);
        app.mode = AppMode::Command;
        app.key_context = KeymapContext::Dialog;
    }
    app.mark_dirty();
}

/// Rebuild the grouped row snapshot in place (live refresh while open).
fn refresh_sessions_rows(app: &mut TuiApp, rt: &SessionRuntime, runtimes: &[SessionRuntime]) {
    let mut rows: Vec<crate::app::SessionDashboardRow> = Vec::new();
    let active_state = rt.state();
    rows.push(crate::app::SessionDashboardRow {
        parked_idx: None,
        title: format!("{} (current)", rt.session.title),
        glyph: active_state.glyph().to_string(),
        state_label: active_state.label().to_string(),
        preview: if rt.agent_busy {
            app.status_message.clone()
        } else {
            rt.preview()
        },
        unread: false,
    });
    for (i, bg) in runtimes.iter().enumerate() {
        let st = bg.state();
        rows.push(crate::app::SessionDashboardRow {
            parked_idx: Some(i),
            title: bg.session.title.clone(),
            glyph: st.glyph().to_string(),
            state_label: st.label().to_string(),
            preview: bg.preview(),
            unread: bg.unread,
        });
    }
    // Group: needs input (rank 0) → working (1) → idle/error (2); stable
    // within a group so creation order is preserved.
    rows.sort_by_key(|r| {
        let rank = match r.parked_idx {
            None => active_state.group_rank(),
            Some(i) => runtimes[i].state().group_rank(),
        };
        (rank, r.parked_idx.unwrap_or(usize::MAX))
    });
    app.sessions_rows = rows;
}

/// Rewrite the picker's live section in place: live rows (active + parked
/// runtimes) sit at the top, persisted-only rows below. Persisted rows keep
/// their relative order; the cursor stays on the same entry when possible.
fn refresh_picker_live_section(app: &mut TuiApp, rt: &SessionRuntime, runtimes: &[SessionRuntime]) {
    let selected_id = app
        .session_list
        .sessions
        .get(app.session_list.selected)
        .map(|e| e.id.clone());

    let mut live_rows: Vec<crate::app::SessionEntry> = Vec::new();
    live_rows.push(crate::app::SessionEntry {
        id: rt.session.id.clone(),
        title: format!("{} (current)", rt.session.title),
        messages: rt.session.messages.len(),
        updated_at: Some(rt.session.updated_at),
        live: Some(usize::MAX),
    });
    for (i, bg) in runtimes.iter().enumerate() {
        let st = bg.state();
        live_rows.push(crate::app::SessionEntry {
            id: bg.session.id.clone(),
            title: format!("{} {} {}", st.glyph(), bg.session.title, st.label()),
            messages: bg.session.messages.len(),
            updated_at: Some(bg.session.updated_at),
            live: Some(i),
        });
    }
    let live_ids: std::collections::HashSet<&str> =
        live_rows.iter().map(|e| e.id.as_str()).collect();
    let mut persisted: Vec<crate::app::SessionEntry> = app
        .session_list
        .sessions
        .iter()
        .filter(|e| e.live.is_none() && !live_ids.contains(e.id.as_str()))
        .cloned()
        .collect();
    // First open: sessions list is empty until the slash command fills it —
    // merge DB rows then.
    if persisted.is_empty() && app.session_list.sessions.iter().all(|e| e.live.is_some()) {
        persisted = load_session_entries()
            .into_iter()
            .filter(|e| !live_ids.contains(e.id.as_str()))
            .collect();
    }
    let mut merged = live_rows;
    merged.extend(persisted);
    app.session_list.sessions = merged;
    if let Some(id) = selected_id
        && let Some(pos) = app.session_list.sessions.iter().position(|e| e.id == id)
    {
        app.session_list.selected = pos;
    }
}

/// Swap the active runtime with `runtimes[idx]`, preserving both sessions'
/// view state. The outgoing active session's `TuiApp` view is saved into
/// its snapshot; the incoming one's snapshot is restored into `app`.
fn switch_to_runtime(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    idx: usize,
) {
    // Outgoing: save the visible view into the runtime being parked.
    app.save_view(&mut rt.view);
    // Incoming: swap runtimes, restore its snapshot into the visible app.
    std::mem::swap(rt, &mut runtimes[idx]);
    rt.unread = false;
    app.restore_view(&rt.view);
    app.focus = FocusPane::Prompt;
}

/// Drain a background (inactive) runtime: prompter requests into its queues,
/// turn events into its view snapshot, completion into agent/session restore.
/// Never touches the visible `app`; sets `unread` on any activity so the
/// dashboard and cycle keys can surface it.
fn drain_background_runtime(rt: &mut SessionRuntime) {
    while let Ok(req) = rt.perm_rx.try_recv() {
        rt.pending_perm_queue.push_back(req);
        rt.unread = true;
    }
    while let Ok(req) = rt.question_rx.try_recv() {
        rt.pending_question_queue.push_back(req);
        rt.unread = true;
    }

    // Replay turn events through a scratch TuiApp holding this runtime's
    // snapshot — reuses the exact active-path rendering logic.
    let mut scratch = TuiApp::new(crate::config::TuiAppConfig::default());
    scratch.restore_view(&rt.view);
    if drain_turn_events(&mut scratch, &mut rt.event_rx) {
        rt.unread = true;
        scratch.save_view(&mut rt.view);
    }

    if let Ok(outcome) = rt.done_rx.try_recv() {
        rt.agent_busy = false;
        rt.cancel_flag = None;
        rt.turn_join = None;
        rt.session_backup = None;
        rt.unread = true;
        match outcome {
            TurnOutcome::Ok {
                text,
                agent: a,
                session: s,
                ..
            } => {
                rt.agent = a;
                rt.session = s;
                rt.last_error = false;
                if !text.is_empty() {
                    let mut scratch = TuiApp::new(crate::config::TuiAppConfig::default());
                    scratch.restore_view(&rt.view);
                    if let Some(last) = scratch.messages.last_mut()
                        && last.role == ChatRole::Assistant
                        && last.content.is_empty()
                    {
                        last.content = text;
                    }
                    scratch.save_view(&mut rt.view);
                }
            }
            TurnOutcome::Remote { text, error, .. } => {
                rt.last_error = error.is_some();
                if let Some(err) = error {
                    let mut scratch = TuiApp::new(crate::config::TuiAppConfig::default());
                    scratch.restore_view(&rt.view);
                    scratch.add_message(ChatRole::System, format!("Remote error: {err}"));
                    scratch.save_view(&mut rt.view);
                } else if !text.is_empty() {
                    let mut scratch = TuiApp::new(crate::config::TuiAppConfig::default());
                    scratch.restore_view(&rt.view);
                    if let Some(last) = scratch.messages.last_mut()
                        && last.role == ChatRole::Assistant
                        && last.content.is_empty()
                    {
                        last.content = text;
                    }
                    scratch.save_view(&mut rt.view);
                }
            }
            TurnOutcome::Err {
                agent: a,
                session: s,
                cancelled,
                error,
                ..
            } => {
                rt.agent = a;
                rt.session = s;
                rt.last_error = !cancelled;
                let mut scratch = TuiApp::new(crate::config::TuiAppConfig::default());
                scratch.restore_view(&rt.view);
                if cancelled {
                    scratch.add_message(ChatRole::System, "⏹ Generation cancelled (Esc).");
                } else {
                    let display =
                        whycode_llm::format_turn_error(&whycode_core::Error::Llm(error.clone()));
                    scratch.add_message(ChatRole::System, format!("Error: {display}"));
                }
                scratch.save_view(&mut rt.view);
            }
        }
        rt.persist("background");
    }
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
            // Grok-style labels in the busy strip (`bash` → `run`).
            let shown = match name.as_str() {
                "bash" | "shell" | "run_terminal_command" => "run",
                "read_file" => "read",
                "search_code" | "rg" => "grep",
                other => other,
            };
            app.status_message = format!("tool: {shown}");
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
            app.intent_badge = if badge.is_empty() { None } else { Some(badge) };
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
        TurnEvent::Background {
            id,
            status,
            summary,
        } => {
            match status.as_str() {
                "running" => {
                    app.bg_running_count = app.bg_running_count.saturating_add(1);
                    app.status_message = format!("bg {id} started");
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        truncate_toast(&format!("bg {id}: {summary}"), 56),
                    );
                }
                "done" => {
                    app.bg_running_count = app.bg_running_count.saturating_sub(1);
                    app.toasts.push(
                        crate::toast::ToastKind::Success,
                        truncate_toast(&format!("bg {id} done · {summary}"), 56),
                    );
                }
                "failed" => {
                    app.bg_running_count = app.bg_running_count.saturating_sub(1);
                    app.toasts.push(
                        crate::toast::ToastKind::Warning,
                        truncate_toast(&format!("bg {id} failed · {summary}"), 64),
                    );
                }
                "killed" => {
                    app.bg_running_count = app.bg_running_count.saturating_sub(1);
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        truncate_toast(&format!("bg {id} killed"), 40),
                    );
                }
                _ => {
                    app.status_message = format!("bg {id} {status}");
                }
            }
            app.mark_dirty();
        }
        TurnEvent::EnqueuePrompt { text } => {
            if !text.trim().is_empty() {
                app.pending_auto_prompts.push_back(text);
                app.toasts.push(
                    crate::toast::ToastKind::Info,
                    truncate_toast(
                        &format!("queued · {} left", app.pending_auto_prompts.len()),
                        40,
                    ),
                );
                app.mark_dirty();
            }
        }
        TurnEvent::Panel(update) => {
            apply_panel_update(app, update);
        }
        TurnEvent::SwarmMessage { from, to, text } => {
            app.toasts.push(
                crate::toast::ToastKind::Info,
                truncate_toast(&format!("swarm {from}→{to}: {text}"), 72),
            );
            app.mark_dirty();
        }
        TurnEvent::PermissionAsk { .. } => {}
        TurnEvent::QuestionAsk { .. } => {}
        TurnEvent::FileStale {
            path,
            reader,
            writer,
        } => {
            let short = path.rsplit('/').next().unwrap_or(&path);
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                truncate_toast(&format!("stale read: {short} ({reader} vs {writer})"), 72),
            );
            app.mark_dirty();
        }
    }
}

pub(crate) fn apply_panel_update(app: &mut TuiApp, update: whycode_core::PanelUpdate) {
    use whycode_core::PanelUpdate;
    app.sidebar.preview = match update {
        PanelUpdate::Clear => crate::app::SidebarPreview::None,
        PanelUpdate::File { path, text } => crate::app::SidebarPreview::File { path, text },
        PanelUpdate::Diff { path, unified } => crate::app::SidebarPreview::Diff { path, unified },
        PanelUpdate::Mermaid { source } => crate::app::SidebarPreview::Mermaid { source },
    };
    app.sidebar.visible = true;
    app.sidebar.active_tab = crate::app::SidebarTab::Preview;
    let label = match &app.sidebar.preview {
        crate::app::SidebarPreview::None => "panel cleared",
        crate::app::SidebarPreview::File { path, .. } => path.as_str(),
        crate::app::SidebarPreview::Diff { path, .. } => path.as_str(),
        crate::app::SidebarPreview::Mermaid { .. } => "mermaid",
    };
    app.toasts.push(
        crate::toast::ToastKind::Info,
        truncate_toast(&format!("panel · {label}"), 48),
    );
    app.mark_dirty();
}

/// Refresh sidebar lists from the workspace index, config, and todos.json.
fn refresh_sidebar(
    app: &mut TuiApp,
    config: &whycode_config::Config,
    file_index: &std::sync::Arc<whycode_index::WorkspaceIndex>,
) {
    const FILE_CAP: usize = 80;
    let mut files: Vec<String> = file_index
        .entries()
        .into_iter()
        .map(|e| {
            if e.is_dir {
                format!("{}/", e.rel)
            } else {
                e.rel.to_string()
            }
        })
        .collect();
    files.sort();
    files.truncate(FILE_CAP);
    app.sidebar.file_tree = files;

    let mut mcp: Vec<String> = config
        .mcp_servers
        .keys()
        .map(|name| format!(" {name}"))
        .collect();
    mcp.sort();
    app.sidebar.mcp_status = mcp;

    app.sidebar.todos = load_sidebar_todos(&app.project_dir);
}

fn load_sidebar_todos(project_dir: &std::path::Path) -> Vec<String> {
    let path = project_dir.join(".whycode").join("todos.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let Some(items) = parsed.get("todos").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?;
            let status = item
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("pending");
            let mark = match status {
                "completed" => "☑",
                "cancelled" => "✗",
                "in_progress" => "…",
                _ => "☐",
            };
            Some(format!("{mark} {content}"))
        })
        .take(40)
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn cycle_agent(
    app: &mut TuiApp,
    agent: &mut Agent,
    session: &mut Session,
    config: &Config,
    project_dir: &std::path::Path,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    event_tx: &tokio::sync::mpsc::UnboundedSender<TurnEvent>,
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
        let bg = agent.background_registry().clone();
        let claims = agent.session_claims();
        let mut next = Agent::new(info)
            .with_config(config)
            .with_background_registry(bg)
            .with_permission_prompter(
                Arc::clone(&perm_prompter) as Arc<dyn whycode_agent::PermissionPrompter>
            )
            .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);
        if let Some(c) = claims {
            next = next.with_session_claims(c);
        }
        *agent = next;
        agent.wire_event_sink(event_tx.clone());
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
                         pending_question_queue: &mut std::collections::VecDeque<
        QuestionRequest,
    >,
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
            true
        }
        KeyCode::Up | KeyCode::Char('k') if !state.free_text_focus => {
            state.move_cursor(-1);
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Down | KeyCode::Char('j') if !state.free_text_focus => {
            state.move_cursor(1);
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        // Multi-question navigate (Grok-style ←/→ between questions)
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') if !state.free_text_focus => {
            let _ = state.go_prev_question();
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') if !state.free_text_focus => {
            let _ = state.go_next_question();
            app.dialogs.push(DialogKind::Question(state));
            true
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
            true
        }
        KeyCode::Char(' ') if !state.free_text_focus => {
            if state.current().map(|q| q.multi_select).unwrap_or(false) {
                state.toggle_multi_at_cursor();
            } else if state.is_other_index(state.cursor) {
                state.free_text_focus = true;
            }
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Char('o') | KeyCode::Char('O') if !state.free_text_focus => {
            // Jump to Other…
            let other = state.option_count().saturating_sub(1);
            state.cursor = other;
            state.free_text_focus = true;
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Enter => {
            if let Some(answers) = state.confirm_current() {
                finish_ok(app, answers, pending_question_queue, pending_perm_queue);
            } else {
                // Still on this question (e.g. empty Other → focus free text)
                app.dialogs.push(DialogKind::Question(state));
            }
            true
        }
        KeyCode::Backspace if state.free_text_focus => {
            state.free_text.pop();
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Char(c) if state.free_text_focus && !c.is_control() => {
            state.free_text.push(c);
            app.dialogs.push(DialogKind::Question(state));
            true
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
            false
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
            ctx.app.sync_context_estimate(ctx.session);
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
                ctx.app.load_messages_from_session(ctx.session);
                ctx.app.status_message = "Undid last turn".into();
            } else if ctx.session.undo_last_turn() > 0 {
                ctx.app.load_messages_from_session(ctx.session);
                ctx.app.status_message = "Undid last turn".into();
            } else {
                ctx.app.status_message = "Nothing to undo".into();
            }
        }
        "/redo" => {
            if let Some(msgs) = ctx.history.redo(&ctx.session.messages, ctx.project_dir) {
                ctx.session.set_messages(msgs);
                ctx.app.load_messages_from_session(ctx.session);
                ctx.app.status_message = "Redid turn".into();
            } else {
                ctx.app.status_message = "Nothing to redo".into();
            }
        }
        "/compact" | "/summarize" => {
            let before = ctx.session.messages.len();
            ctx.session.compact(ctx.config.session.compaction_threshold);
            // Provider usage is stale after trim — fall back to the char heuristic.
            ctx.app.sync_context_estimate(ctx.session);
            ctx.app.turn_usage = None;
            ctx.app.status_message = format!("Compacted {before} → {}", ctx.session.messages.len());
        }
        "/bg" => {
            let rest = rest.trim();
            if rest.is_empty() || rest == "list" {
                let jobs = ctx.agent.background_registry().list();
                if jobs.is_empty() {
                    ctx.app
                        .toasts
                        .push(crate::toast::ToastKind::Info, "No background jobs");
                } else {
                    let mut lines = vec![format!(
                        "Background jobs ({} running)",
                        ctx.agent.background_registry().running_count()
                    )];
                    for j in jobs {
                        lines.push(format!(
                            "{} [{}] {:.0}s  {}",
                            j.id,
                            j.status.as_str(),
                            j.elapsed.as_secs_f64(),
                            j.label
                        ));
                    }
                    lines.push("Hint: /bg kill bg-N".into());
                    ctx.app.add_message(ChatRole::System, lines.join("\n"));
                }
            } else if let Some(id) = rest.strip_prefix("kill ").map(str::trim) {
                match ctx.agent.background_registry().kill(id) {
                    Ok(msg) => ctx.app.toasts.push(crate::toast::ToastKind::Info, msg),
                    Err(e) => ctx.app.toasts.push(crate::toast::ToastKind::Warning, e),
                }
            } else {
                ctx.app.status_message = "Usage: /bg | /bg kill <id>".into();
            }
        }
        "/loop" => {
            let rest = rest.trim();
            if rest == "stop" || rest == "clear" {
                let n = ctx.app.pending_auto_prompts.len();
                ctx.app.pending_auto_prompts.clear();
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Info,
                    format!("Cleared {n} queued loop prompt(s)"),
                );
            } else {
                // /loop N prompt…  or  /loop prompt… (N=3)
                let mut parts = rest.splitn(2, char::is_whitespace);
                let first = parts.next().unwrap_or("").trim();
                let rest_prompt = parts.next().unwrap_or("").trim();
                let (n, prompt) = if let Ok(count) = first.parse::<usize>() {
                    (count, rest_prompt.to_string())
                } else if !rest.is_empty() {
                    (3usize, rest.to_string())
                } else {
                    ctx.app.status_message = "Usage: /loop N prompt…  |  /loop stop".into();
                    return;
                };
                if prompt.is_empty() {
                    ctx.app.status_message = "Usage: /loop N prompt…  |  /loop stop".into();
                    return;
                }
                let n = n.clamp(1, 20);
                // First runs now; remaining N-1 queued.
                ctx.app.add_message(ChatRole::User, &prompt);
                ctx.app.pending_prompt = Some(prompt.clone());
                for _ in 1..n {
                    ctx.app.pending_auto_prompts.push_back(prompt.clone());
                }
                ctx.app
                    .toasts
                    .push(crate::toast::ToastKind::Info, format!("Loop ×{n} queued"));
            }
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
        "/memory" => match memory_service(ctx.project_dir, ctx.config) {
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
                        msg.push_str(&format!("\n· {}  {}", &r.id[..8.min(r.id.len())], r.text));
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
        },
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
            // OAuth subscription login (`whycode auth login <provider>`).
            if ctx.api_key.is_empty()
                && whycode_auth::providers::supports_oauth(ctx.provider)
                && let Ok(dir) = Config::data_dir()
                && let Some(tok) = whycode_auth::providers::access_token(ctx.provider, &dir).await
            {
                whycode_llm::oauth_refresh::register(ctx.provider, dir);
                *ctx.api_key = tok;
            }
            if ctx.api_key.is_empty() {
                ctx.app.status_message = format!("no API key · set {env_name}");
                let oauth_supported = whycode_auth::providers::supports_oauth(ctx.provider);
                ctx.app.add_message(
                    ChatRole::System,
                    format!(
                        "No API key for `{}`\n\
                         → export {env_name}=…\n\
                         → whycode provider add {} --api-key <key> · then /connect",
                        ctx.provider, ctx.provider
                    ),
                );
                // OAuth-supported provider: offer the login flow right here
                // instead of only printing help (plan-oauth `/connect`).
                if oauth_supported && let Ok(dir) = Config::data_dir() {
                    spawn_oauth_login(ctx.app, &ctx.auth_tx, dir, ctx.provider.as_str());
                } else {
                    ctx.app.toasts.push(
                        crate::toast::ToastKind::Warning,
                        format!("Still no key for {}", ctx.provider),
                    );
                }
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
        "/login" => {
            let arg = rest.trim();
            if arg.is_empty() {
                // No argument: open the provider picker, one row per OAuth
                // provider, annotated with the stored-credential status.
                let mut rows = Vec::new();
                if let Ok(dir) = Config::data_dir() {
                    let store = whycode_auth::TokenStore::new(&dir);
                    for name in whycode_auth::OAUTH_PROVIDERS {
                        let label = whycode_auth::providers::spec_for(name)
                            .map(|s| s.label)
                            .unwrap_or(name);
                        let connected = store.get(name).ok().flatten().is_some();
                        rows.push(crate::app::LoginProviderRow {
                            provider: (*name).to_string(),
                            label: label.to_string(),
                            connected,
                        });
                    }
                }
                ctx.app.login_dialog = crate::app::LoginDialogState { selected: 0, rows };
                crate::input::open_dialog(ctx.app, DialogKind::Login);
            } else if whycode_auth::providers::supports_oauth(arg) {
                if let Ok(dir) = Config::data_dir() {
                    spawn_oauth_login(ctx.app, &ctx.auth_tx, dir, arg);
                }
            } else {
                ctx.app.status_message = format!(
                    "OAuth login not available for `{arg}` ({})",
                    whycode_auth::OAUTH_PROVIDERS.join(", ")
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
        "/timestamps" => {
            ctx.app.show_timestamps = !ctx.app.show_timestamps;
            ctx.app.config.show_timestamps = ctx.app.show_timestamps;
            for msg in &mut ctx.app.messages {
                msg.invalidate_layout();
            }
            ctx.app.status_message = if ctx.app.show_timestamps {
                "Timestamps on".into()
            } else {
                "Timestamps off".into()
            };
            ctx.app.mark_dirty();
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
        "/doctor" => {
            ctx.app.add_message(
                ChatRole::System,
                doctor_report(ctx.session, ctx.app, ctx.config, ctx.agent, ctx.project_dir),
            );
        }
        "/diff" => {
            ctx.app
                .add_message(ChatRole::System, project_diff_report(ctx.project_dir));
        }
        "/context" => {
            ctx.app.add_message(
                ChatRole::System,
                context_report(ctx.session, ctx.app, ctx.config, ctx.agent),
            );
        }
        "/cost" | "/usage" => {
            ctx.app
                .add_message(ChatRole::System, cost_report(ctx.session, ctx.app));
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
                ctx.app.theme_selected = ThemeName::ALL.iter().position(|x| *x == t).unwrap_or(0);
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

/// Spawn a tiny follow-up suggestion when `tui.prompt_suggestions = "idle"`.
fn maybe_spawn_prompt_suggestion(
    config: &Config,
    session: &Session,
    provider: &str,
    model: &str,
    api_key: &str,
    app: &mut TuiApp,
    suggest_tx: mpsc::UnboundedSender<String>,
) {
    let mode = config.tui.prompt_suggestions.trim().to_ascii_lowercase();
    if mode != "idle" && mode != "on" && mode != "true" && mode != "1" {
        return;
    }
    if api_key.is_empty() {
        return;
    }
    app.pending_suggestion = None;
    let last_user = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == whycode_core::types::Role::User)
        .and_then(|m| m.content.as_text().map(|s| s.to_string()))
        .unwrap_or_default();
    if last_user.trim().is_empty() {
        return;
    }
    let last_asst = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == whycode_core::types::Role::Assistant)
        .and_then(|m| m.content.as_text().map(|s| s.to_string()))
        .unwrap_or_default();
    let provider = provider.to_string();
    let model = model.to_string();
    let api_key = api_key.to_string();
    let model_fast = config.session.model_fast.clone();
    let mut reg = whycode_llm::provider::ProviderRegistry::default();
    reg.register_from_config(config);
    tokio::spawn(async move {
        let (p, m) = whycode_agent::resolve_title_model(&provider, &model, model_fast.as_deref());
        let Some(prov) = reg.get(&p) else {
            return;
        };
        use whycode_core::types::{LlmRequest, Message, MessageContent, Role};
        let body = format!(
            "User last said:\n{}\n\nAssistant replied (excerpt):\n{}\n\n\
             Suggest ONE short next user message (≤12 words) to continue the coding task. \
             Reply with only that message, no quotes.",
            last_user.chars().take(500).collect::<String>(),
            last_asst.chars().take(400).collect::<String>()
        );
        let request = LlmRequest {
            system: "You propose a single follow-up user prompt for a coding agent.".into(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text(body),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: vec![],
            max_tokens: Some(40),
            temperature: Some(0.4),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        };
        let transport = whycode_llm::LlmTransport {
            complete_timeout: Some(std::time::Duration::from_secs(8)),
            retry: whycode_llm::RetryPolicy {
                max_retries: 0,
                initial_backoff: std::time::Duration::from_millis(100),
                max_backoff: std::time::Duration::from_secs(1),
                max_elapsed: std::time::Duration::from_secs(8),
                full_jitter: true,
            },
        };
        if let Ok(resp) = transport.complete(prov, &request, &api_key, &m).await {
            let text = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    whycode_core::types::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .trim_matches('"')
                .to_string();
            if !text.is_empty() {
                let _ = suggest_tx.send(text);
            }
        }
    });
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
///
/// OAuth subscription logins (`/login`) bypass config, so their providers
/// would never appear here: merge in the suggested models for any provider
/// with a credential in the token store.
fn configured_models(config: &Config) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = config
        .providers
        .values()
        .flat_map(|p| p.models.iter().map(move |m| (p.name.clone(), m.clone())))
        .collect();
    if let Ok(dir) = Config::data_dir() {
        let store = whycode_auth::TokenStore::new(&dir);
        for name in whycode_auth::OAUTH_PROVIDERS {
            if store.get(name).ok().flatten().is_some() {
                out.extend(
                    whycode_auth::providers::suggested_models(name)
                        .iter()
                        .map(|m| ((*name).to_string(), (*m).to_string())),
                );
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn parse_session_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
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
            updated_at: parse_session_rfc3339(&s.updated_at),
            live: None,
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
/// Context window breakdown (`/context` — Claude Code spirit).
fn context_report(
    session: &Session,
    app: &TuiApp,
    config: &Config,
    agent: &whycode_agent::Agent,
) -> String {
    use whycode_core::types::{MessageContent, Role};

    let mut lines = vec!["Context".to_string()];
    lines.push(format!(
        "  budget:    {} / {} ({}%)",
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent()
    ));
    lines.push(format!(
        "  estimate:  ~{} tok (char heuristic)",
        session.token_count()
    ));
    lines.push(format!(
        "  compact:   threshold={} llm={}",
        config.session.compaction_threshold, config.session.compaction_llm
    ));

    let mut by_role = std::collections::BTreeMap::<&str, usize>::new();
    let mut tool_sizes: Vec<(usize, String)> = Vec::new();
    for (i, m) in session.messages.iter().enumerate() {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        *by_role.entry(role).or_default() += 1;
        if m.role == Role::Tool {
            let chars = match &m.content {
                MessageContent::Text(t) => t.chars().count(),
                MessageContent::Blocks(b) => b
                    .iter()
                    .map(|bl| match bl {
                        whycode_core::types::ContentBlock::Text { text }
                        | whycode_core::types::ContentBlock::ToolResult { content: text, .. } => {
                            text.chars().count()
                        }
                        _ => 0,
                    })
                    .sum(),
            };
            let label = m.name.clone().unwrap_or_else(|| format!("tool#{i}"));
            tool_sizes.push((chars, label));
        }
    }
    lines.push(format!("  messages:  {}", session.messages.len()));
    for (role, n) in by_role {
        lines.push(format!("    {role}: {n}"));
    }
    tool_sizes.sort_by_key(|e| std::cmp::Reverse(e.0));
    if !tool_sizes.is_empty() {
        lines.push("  largest tool results:".into());
        for (chars, label) in tool_sizes.into_iter().take(8) {
            lines.push(format!("    {label}: {chars} chars"));
        }
    }

    let profile = whycode_tools::ToolProfile::parse(&config.session.tool_profile);
    let activated = agent.activated_tools_snapshot();
    lines.push(format!(
        "  tools:     profile={} activated={}",
        profile.as_str(),
        if activated.is_empty() {
            "(none)".into()
        } else {
            activated.join(",")
        }
    ));
    lines.push(format!("  memory:    enabled={}", config.memory.enabled));
    if let Some(cwd) = agent.cwd_override_path() {
        lines.push(format!("  cwd:       override {}", cwd.display()));
    } else {
        lines.push(format!("  cwd:       {}", session.project_path.display()));
    }
    lines.join("\n")
}

/// Git status + short diff for the project (Claude Code `/diff` spirit).
fn project_diff_report(project_dir: &std::path::Path) -> String {
    let mut out = String::from("Diff\n");
    let status = std::process::Command::new("git")
        .args(["status", "--short", "--branch"])
        .current_dir(project_dir)
        .output();
    match status {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            if s.trim().is_empty() {
                out.push_str("  (clean working tree)\n");
            } else {
                out.push_str("  status:\n");
                for line in s.lines().take(80) {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            out.push_str(&format!(
                "  git status failed: {}\n",
                err.trim().lines().next().unwrap_or("unknown")
            ));
            return out;
        }
        Err(e) => {
            out.push_str(&format!("  git unavailable: {e}\n"));
            return out;
        }
    }

    let diff = std::process::Command::new("git")
        .args(["diff", "--stat", "HEAD"])
        .current_dir(project_dir)
        .output();
    if let Ok(o) = diff
        && o.status.success()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if !s.trim().is_empty() {
            out.push_str("  unstaged/staged vs HEAD:\n");
            for line in s.lines().take(60) {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }

    let staged = std::process::Command::new("git")
        .args(["diff", "--stat", "--cached"])
        .current_dir(project_dir)
        .output();
    if let Ok(o) = staged
        && o.status.success()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if !s.trim().is_empty() {
            out.push_str("  staged only:\n");
            for line in s.lines().take(40) {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }

    out
}

/// Session + last-turn token usage (Claude Code `/cost` spirit).
fn cost_report(session: &Session, app: &TuiApp) -> String {
    let mut lines = vec!["Cost / usage".to_string()];
    let u = &session.usage;
    if u.is_empty() {
        lines.push(format!(
            "  session:   ~{} tokens (estimated; provider has not reported usage yet)",
            session.token_count()
        ));
    } else {
        lines.push(format!(
            "  session:   {} in / {} out · total {}",
            format_token_count(u.input_tokens),
            format_token_count(u.output_tokens),
            format_token_count(u.total())
        ));
        if let Some(c) = u.cache_creation_input_tokens.filter(|n| *n > 0) {
            lines.push(format!("  cache write: {}", format_token_count(c)));
        }
        if let Some(r) = u.cache_read_input_tokens.filter(|n| *n > 0) {
            lines.push(format!("  cache read:  {}", format_token_count(r)));
        }
    }
    if let Some(ref turn) = app.turn_usage {
        lines.push(format!(
            "  last turn: {} in / {} out · total {}",
            format_token_count(turn.input_tokens),
            format_token_count(turn.output_tokens),
            format_token_count(turn.total())
        ));
    } else {
        lines.push("  last turn: (none yet)".into());
    }
    lines.push(format!(
        "  context:   {} / {} ({}%)",
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent()
    ));
    lines
        .push("  note:      providers bill differently; figures are token counts, not USD.".into());
    lines.join("\n")
}

/// Claude Code–style environment check: keys, sandbox, git, tools, jobs.
fn doctor_report(
    session: &Session,
    app: &TuiApp,
    config: &Config,
    agent: &whycode_agent::Agent,
    project_dir: &std::path::Path,
) -> String {
    use std::path::Path;

    let mut lines = vec!["Doctor".to_string()];

    // ── Provider / model ──────────────────────────────────────────────
    let provider = app.provider_name.as_str();
    let model = app.model_name.as_str();
    lines.push(format!("  provider:     {provider}"));
    lines.push(format!("  model:        {model}"));
    lines.push(format!("  agent:        {}", agent.info.name));
    lines.push(format!("  tool_profile: {}", config.session.tool_profile));

    // API key present? (never print the key)
    let env_name = format!("{}_API_KEY", provider.to_uppercase());
    let key_ok = config
        .providers
        .get(provider)
        .and_then(|pc| pc.api_key.as_ref())
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
        || std::env::var(&env_name)
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false);
    lines.push(format!(
        "  api_key:      {}",
        if key_ok {
            "set"
        } else {
            "MISSING — /connect or env"
        }
    ));

    // ── Paths ─────────────────────────────────────────────────────────
    lines.push(format!("  project:      {}", project_dir.display()));
    lines.push(format!("  session_id:   {}", session.id));

    let git = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    lines.push(format!(
        "  git_repo:     {}",
        if git { "yes" } else { "no" }
    ));

    // ── Safety ────────────────────────────────────────────────────────
    lines.push(format!(
        "  bash_risk:    {}",
        config.security.bash_risk_threshold
    ));
    lines.push(format!(
        "  sandbox:      mode={} network={}",
        config.security.sandbox, config.security.sandbox_network
    ));
    #[cfg(target_os = "linux")]
    {
        let bwrap = Path::new("/usr/bin/bwrap").is_file() || which_bwrap();
        lines.push(format!(
            "  bwrap:        {}",
            if bwrap {
                "available"
            } else {
                "not found (host fallback)"
            }
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        lines.push("  bwrap:        n/a (non-Linux)".into());
    }

    // ── Automation ────────────────────────────────────────────────────
    let bg = agent.background_registry();
    let running = bg.running_count();
    let total = bg.list().len();
    lines.push(format!(
        "  background:   {running} running / {total} known (max {})",
        config.automation.max_background_jobs
    ));
    lines.push(format!(
        "  swarm:        enabled={} worktrees={}",
        config.swarm.enabled, config.swarm.worktrees
    ));
    lines.push(format!(
        "  compaction:   threshold={}",
        config.session.compaction_threshold
    ));
    lines.push(format!(
        "  context:      {} / {} ({}%)",
        format_token_count(app.context_used),
        format_token_count(app.max_context_tokens),
        app.context_percent()
    ));

    // ── Quick health summary ──────────────────────────────────────────
    let mut issues = Vec::new();
    if !key_ok {
        issues.push("API key missing for active provider");
    }
    if !project_dir.is_dir() {
        issues.push("project directory missing");
    }
    if issues.is_empty() {
        lines.push("  status:       ok".into());
    } else {
        lines.push(format!("  status:       issues — {}", issues.join("; ")));
    }

    lines.join("\n")
}

#[cfg(target_os = "linux")]
fn which_bwrap() -> bool {
    std::process::Command::new("which")
        .arg("bwrap")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn session_details(session: &Session, agent: &str, app: &TuiApp, config: &Config) -> String {
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
        "  model_race: {} (after {}ms)\n  response_cache: {}\n",
        config.session.model_race, config.session.race_after_ms, config.session.response_cache
    ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_toast_takes_first_line_and_trims() {
        assert_eq!(truncate_toast("short", 20), "short");
        assert_eq!(truncate_toast("first\nsecond", 20), "first");
        assert_eq!(truncate_toast("  padded  ", 20), "padded");
        let long = "x".repeat(100);
        let out = truncate_toast(&long, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn mouse_move_does_not_force_redraw_other_events_do() {
        let moved = Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        let wheel = Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 1,
            row: 1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        let key = Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char('a')));
        assert!(!event_forces_redraw(&moved));
        assert!(event_forces_redraw(&wheel));
        assert!(event_forces_redraw(&key));
        assert!(event_forces_redraw(&Event::Resize(80, 24)));
    }

    #[test]
    fn rfc3339_parser_accepts_and_rejects() {
        assert!(parse_session_rfc3339("2026-01-02T03:04:05Z").is_some());
        assert!(parse_session_rfc3339("2026-01-02T03:04:05+02:00").is_some());
        assert!(parse_session_rfc3339("not a date").is_none());
        assert!(parse_session_rfc3339("").is_none());
    }

    #[test]
    fn short_session_id_truncates_long_ids() {
        assert_eq!(short_session_id("abc"), "abc");
        assert_eq!(short_session_id("abcdefgh"), "abcdefgh");
        assert_eq!(short_session_id("abcdefghijkl"), "abcdefgh…");
    }

    #[test]
    fn turn_done_status_formats_cancel_and_usage() {
        let app = TuiApp::new(TuiAppConfig::default());
        let s = format_turn_done_status(&app, "build", "anthropic", "m", None, true);
        assert_eq!(s, "Turn cancelled.");
        let s = format_turn_done_status(&app, "build", "anthropic", "m", Some(4500), true);
        assert_eq!(s, "Turn cancelled in 4.5s");

        let mut app = TuiApp::new(TuiAppConfig::default());
        let s = format_turn_done_status(&app, "build", "anthropic", "m", None, false);
        assert_eq!(s, "Done");

        app.turn_usage = Some(whycode_core::types::Usage {
            input_tokens: 1200,
            output_tokens: 340,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(500),
        });
        let s = format_turn_done_status(&app, "build", "anthropic", "m", Some(4200), false);
        assert!(s.contains("Worked for 4.2s"), "{s}");
        assert!(s.contains("in"), "{s}");
        assert!(s.contains("out"), "{s}");
        assert!(s.contains("cached"), "{s}");
    }

    #[test]
    fn snapshot_cells_reads_every_symbol() {
        use ratatui::buffer::Buffer;
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        buf[(0, 0)].set_symbol("a");
        buf[(1, 1)].set_symbol("b");
        let rows = snapshot_cells(&buf);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "a");
        assert_eq!(rows[1][1], "b");
        assert_eq!(rows[0][1], " ");
    }

    #[test]
    fn expand_at_files_inlines_existing_and_keeps_missing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("note.txt");
        std::fs::write(&f, "hello world").unwrap();
        let out = expand_at_files("read @note.txt now", dir.path());
        assert!(out.contains("hello world"), "{out}");
        assert!(out.contains("--- file: note.txt ---"), "{out}");
        assert!(out.ends_with("now"), "{out}");

        let out = expand_at_files("see @missing.txt", dir.path());
        assert!(
            out.contains("@missing.txt"),
            "missing must stay literal: {out}"
        );

        // Absolute path works too.
        let out = expand_at_files(&format!("@{} done", f.display()), dir.path());
        assert!(out.contains("hello world"), "{out}");
    }

    #[test]
    fn expand_at_files_truncates_huge_files() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.txt");
        std::fs::write(&f, "x".repeat(AT_FILE_MAX_CHARS + 100)).unwrap();
        let out = expand_at_files("@big.txt", dir.path());
        assert!(out.contains("characters omitted"), "{out}");
    }

    #[test]
    fn expand_at_files_bare_at_stays_literal() {
        let out = expand_at_files("email me @ now", std::path::Path::new("/work"));
        assert!(out.contains("@ now"), "{out}");
    }

    #[test]
    fn memory_settings_for_sets_agent_bank() {
        let config = Config::default();
        let base = memory_settings(&config);
        assert!(base.enabled);
        assert!(base.agent_bank.is_none());
        let scoped = memory_settings_for(&config, Some("worker".into()));
        assert_eq!(scoped.agent_bank.as_deref(), Some("worker"));
    }

    #[test]
    fn resolve_session_latest_and_prefix() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();
        // Empty db → latest is None.
        assert!(
            resolve_and_load_session(&db, RESUME_LATEST)
                .unwrap()
                .is_none()
        );
        assert!(resolve_and_load_session(&db, "nope").unwrap().is_none());

        let mut s1 = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
        s1.add_user_message("first");
        s1.save_to_db(&db).unwrap();
        let id1 = s1.id.clone();

        let mut s2 = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
        s2.add_user_message("second");
        s2.save_to_db(&db).unwrap();

        // Exact id.
        let loaded = resolve_and_load_session(&db, &id1).unwrap().expect("exact");
        assert_eq!(loaded.id, id1);

        // Unique prefix (8 chars) matches.
        let prefix: String = id1.chars().take(8).collect();
        let loaded = resolve_and_load_session(&db, &prefix)
            .unwrap()
            .expect("prefix");
        assert_eq!(loaded.id, id1);

        // RESUME_LATEST → most recently updated (s2).
        let latest = resolve_and_load_session(&db, RESUME_LATEST)
            .unwrap()
            .expect("latest");
        assert_eq!(latest.id, s2.id);
    }

    #[test]
    fn apply_panel_update_sets_preview_and_toast() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        apply_panel_update(
            &mut app,
            whycode_core::PanelUpdate::File {
                path: "a.rs".into(),
                text: "fn main() {}".into(),
            },
        );
        assert!(app.sidebar.visible);
        assert_eq!(app.sidebar.active_tab, crate::app::SidebarTab::Preview);
        assert!(matches!(
            &app.sidebar.preview,
            crate::app::SidebarPreview::File { path, text }
                if path == "a.rs" && text == "fn main() {}"
        ));

        apply_panel_update(
            &mut app,
            whycode_core::PanelUpdate::Diff {
                path: "b.rs".into(),
                unified: "-a\n+b".into(),
            },
        );
        assert!(matches!(
            &app.sidebar.preview,
            crate::app::SidebarPreview::Diff { path, unified }
                if path == "b.rs" && unified == "-a\n+b"
        ));

        apply_panel_update(
            &mut app,
            whycode_core::PanelUpdate::Mermaid {
                source: "graph TD".into(),
            },
        );
        assert!(matches!(
            &app.sidebar.preview,
            crate::app::SidebarPreview::Mermaid { source } if source == "graph TD"
        ));

        apply_panel_update(&mut app, whycode_core::PanelUpdate::Clear);
        assert!(matches!(
            app.sidebar.preview,
            crate::app::SidebarPreview::None
        ));
    }

    #[test]
    fn cost_report_handles_empty_and_filled_usage() {
        let mut session = Session::new(PathBuf::from("/work/proj"), "sys".into());
        session.add_user_message("hello");
        let app = TuiApp::new(TuiAppConfig::default());

        // No provider usage yet → estimated line.
        let out = cost_report(&session, &app);
        assert!(out.contains("estimated"), "{out}");
        assert!(out.contains("last turn: (none yet)"), "{out}");

        session.usage = whycode_core::types::Usage {
            input_tokens: 1200,
            output_tokens: 300,
            cache_creation_input_tokens: Some(500),
            cache_read_input_tokens: Some(9000),
        };
        let out = cost_report(&session, &app);
        assert!(out.contains("1.2k in / 300 out"), "{out}");
        assert!(out.contains("cache write: 500"), "{out}");
        assert!(out.contains("cache read:  9k"), "{out}");
        assert!(out.contains("total 11k"), "{out}"); // includes cache tokens
    }

    #[test]
    fn cost_report_includes_last_turn_usage() {
        let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.turn_usage = Some(whycode_core::types::Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        let out = cost_report(&session, &app);
        assert!(out.contains("last turn: 100 in / 50 out"), "{out}");
    }

    #[test]
    fn context_report_lists_roles_and_tool_sizes() {
        let mut session = Session::new(PathBuf::from("/work/proj"), "sys".into());
        session.add_user_message("do it");
        session.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "tc1".into(),
            content: "short result".into(),
            is_error: false,
        }]);
        let app = TuiApp::new(TuiAppConfig::default());
        let config = Config::default();
        let agent = Agent::new(whycode_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycode_core::types::PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        });
        let out = context_report(&session, &app, &config, &agent);
        assert!(out.contains("Context"), "{out}");
        assert!(out.contains("messages:  2"), "{out}");
        assert!(out.contains("user: 1"), "{out}");
        assert!(out.contains("tool: 1"), "{out}");
        assert!(out.contains("largest tool results"), "{out}");
        assert!(out.contains("profile="), "{out}");
        assert!(out.contains("memory:    enabled="), "{out}");
        assert!(out.contains("cwd:"), "{out}");
    }

    #[test]
    fn load_sidebar_todos_missing_and_valid() {
        let dir = tempfile::tempdir().unwrap();
        // No todos.json → empty.
        assert!(load_sidebar_todos(dir.path()).is_empty());

        let whycode = dir.path().join(".whycode");
        std::fs::create_dir_all(&whycode).unwrap();
        std::fs::write(
            whycode.join("todos.json"),
            r#"{"todos": [
                {"content": "finish task", "status": "pending"},
                {"content": "done item", "status": "completed"},
                {"content": "working now", "status": "in_progress"},
                {"content": "skipped", "status": "cancelled"},
                {"content": "no status"}
            ]}"#,
        )
        .unwrap();
        let todos = load_sidebar_todos(dir.path());
        assert_eq!(todos.len(), 5);
        assert_eq!(todos[0], "☐ finish task");
        assert_eq!(todos[1], "☑ done item");
        assert_eq!(todos[2], "… working now");
        assert_eq!(todos[3], "✗ skipped");
        assert_eq!(todos[4], "☐ no status");
    }

    #[test]
    fn load_sidebar_todos_invalid_json_and_wrong_shape() {
        let dir = tempfile::tempdir().unwrap();
        let whycode = dir.path().join(".whycode");
        std::fs::create_dir_all(&whycode).unwrap();
        std::fs::write(whycode.join("todos.json"), "not json {{{").unwrap();
        assert!(load_sidebar_todos(dir.path()).is_empty());
        std::fs::write(whycode.join("todos.json"), r#"{"other": 1}"#).unwrap();
        assert!(load_sidebar_todos(dir.path()).is_empty());
    }

    #[test]
    fn configured_models_from_providers_and_oauth() {
        use std::sync::OnceLock;
        static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
        // Isolate WHYCODE_HOME so TokenStore reads a temp dir, not user keys.
        let prev = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", dir.path()) };

        let mut config = Config::default();
        config.providers.insert(
            "acme".into(),
            whycode_core::types::ProviderConfig {
                name: "acme".into(),
                api_key: None,
                api_base: None,
                base_url: None,
                headers: None,
                models: vec!["acme-1".into(), "acme-2".into()],
                tool_arguments: None,
                extra: Default::default(),
            },
        );
        let out = configured_models(&config);
        assert!(out.contains(&("acme".to_string(), "acme-1".to_string())));
        assert!(out.contains(&("acme".to_string(), "acme-2".to_string())));

        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODE_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODE_HOME") },
        }
    }

    #[test]
    fn format_token_count_scales() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1000), "1k");
        assert_eq!(format_token_count(1200), "1.2k");
        assert_eq!(format_token_count(200_000), "200k");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_200_000), "1.2M");
        // Sub-million stays in the k branch.
        assert_eq!(format_token_count(999_500), "999.5k");
    }

    #[test]
    fn session_details_reports_usage_and_flags() {
        let mut session = Session::new(PathBuf::from("/work/proj"), "sys".into());
        session.add_user_message("hi");
        let app = TuiApp::new(TuiAppConfig::default());
        let config = Config::default();

        // No usage yet → estimated line.
        let out = session_details(&session, "build", &app, &config);
        assert!(out.contains("title:"), "{out}");
        assert!(out.contains("agent:     build"), "{out}");
        assert!(out.contains("messages:  1"), "{out}");
        assert!(out.contains("estimated"), "{out}");
        assert!(out.contains("prompt_cache:"), "{out}");
        assert!(out.contains("model_race:"), "{out}");
        assert!(out.contains("swarm:"), "{out}");

        // With usage → input/output/cache lines.
        session.usage = whycode_core::types::Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: Some(5),
            cache_read_input_tokens: Some(7),
        };
        let out = session_details(&session, "build", &app, &config);
        assert!(out.contains("input:     10"), "{out}");
        assert!(out.contains("output:    20"), "{out}");
        assert!(out.contains("cache write: 5"), "{out}");
        assert!(out.contains("cache read:  7"), "{out}");
        assert!(out.contains("total:     42"), "{out}");
        assert!(!out.contains("estimated"), "{out}");
    }

    #[test]
    fn doctor_report_lists_checks() {
        let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
        let app = TuiApp::new(TuiAppConfig::default());
        let config = Config::default();
        let agent = Agent::new(whycode_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycode_core::types::PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        });
        let dir = tempfile::tempdir().unwrap();
        let out = doctor_report(&session, &app, &config, &agent, dir.path());
        assert!(out.contains("Doctor"), "{out}");
        assert!(out.contains("provider:"), "{out}");
        assert!(out.contains("model:"), "{out}");
        assert!(out.contains("agent:        build"), "{out}");
        assert!(out.contains("api_key:"), "{out}");
        assert!(out.contains("git_repo:     no"), "{out}");
        assert!(out.contains("bash_risk:"), "{out}");
        assert!(out.contains("sandbox:"), "{out}");
        assert!(out.contains("background:"), "{out}");
        assert!(out.contains("swarm:"), "{out}");
        assert!(out.contains("context:"), "{out}");
        assert!(out.contains("status:"), "{out}");
    }
}
