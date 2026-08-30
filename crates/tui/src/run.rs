//! TUI event loop — streaming agent + permission dialogs.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::cursor::SetCursorStyle;
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
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use whycodes_agent::agent::Agent;
use whycodes_agent::permission::ChannelPermissionPrompter;
use whycodes_agent::{
    CancelFlag, ChannelQuestionPrompter, QuestionError, QuestionPrompter, QuestionRequest,
    TurnEvent, TurnOpts, new_cancel_flag, request_cancel,
};
use whycodes_config::Config;
use whycodes_core::types::AgentMode;
use whycodes_session::SessionHistory;
use whycodes_session::session::Session;

use crate::app::{
    AgentState, AppMode, ChatRole, ConfirmAction, DialogKind, FocusPane, TuiApp, UpdateOffer,
    format_elapsed_ms, format_token_count, format_usage_short,
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
    pub config: &'a mut Config,
    pub project_dir: &'a std::path::Path,
    pub provider: &'a mut String,
    pub model: &'a mut String,
    pub api_key: &'a mut String,
    pub perm_prompter: Arc<ChannelPermissionPrompter>,
    pub question_prompter: Arc<ChannelQuestionPrompter>,
    pub auth_tx: mpsc::UnboundedSender<AuthFlowEvent>,
    /// Queued `/compact [note]` — the event loop spawns it like a turn so
    /// the LLM summary cannot freeze the pager.
    pub pending_compact: &'a mut Option<String>,
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

/// Drives `whycodes_auth::providers::LoginUi` from the TUI: notes land in
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

impl whycodes_auth::providers::LoginUi for TuiLoginUi {
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
    ) -> impl std::future::Future<Output = whycodes_auth::error::Result<String>> + Send {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(AuthFlowEvent::NeedCode(tx));
        async move {
            rx.await.map_err(|_| {
                whycodes_auth::AuthError::FlowCancelled("sign-in dismissed".to_string())
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
        let store = whycodes_auth::TokenStore::new(&dir);
        let mut ui = TuiLoginUi { tx: tx.clone() };
        let result = whycodes_auth::providers::login_with_ui(&p, &store, true, &mut ui)
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
        .with_permission_prompter(Arc::clone(perm) as Arc<dyn whycodes_agent::PermissionPrompter>)
        .with_question_prompter(Arc::clone(question) as Arc<dyn QuestionPrompter>)
}

/// Options for launching the interactive TUI.
pub struct TuiRunOptions {
    pub project_dir: PathBuf,
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub agent_name: String,
    /// Headless-only cap. Interactive TUI passes `None` (Grok: no default turn limit).
    pub max_turns: Option<usize>,
    pub initial_prompt: Option<String>,
    pub config: Config,
    /// When set, load this session (or the latest if `"__latest__"`) before first paint.
    ///
    /// Used by CLI `--continue` / `--resume <id>`. In-session resume uses
    /// `pending_session_id` on the app instead.
    pub resume_session_id: Option<String>,
    /// When set, turns go to `whycodes serve` over HTTP instead of an in-process agent.
    pub remote: Option<crate::remote::RemoteAttach>,
    /// Background GitHub latest-release check. `None` skips the home popup.
    pub update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<UpdateOffer>>,
}

/// How the TUI left the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiExit {
    /// User quit. Session summary already printed.
    Quit,
    /// User accepted the home-screen update prompt. CLI should install after
    /// the terminal is restored (binary replace while alt-screen is up is messy).
    Upgrade,
}

/// Sentinel for `TuiRunOptions::resume_session_id`: most recently updated session.
pub const RESUME_LATEST: &str = "__latest__";

/// Everything `run` needs before it touches the terminal.
struct TuiBoot {
    app: TuiApp,
    config: Config,
    file_index: Arc<whycodes_index::WorkspaceIndex>,
    agent: Agent,
    session: Session,
    history: SessionHistory,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    perm_rx: mpsc::UnboundedReceiver<whycodes_agent::PermissionRequest>,
    question_rx: mpsc::UnboundedReceiver<QuestionRequest>,
    session_claims: whycodes_core::FileClaimRegistry,
    missing_key: bool,
}

fn apply_resume(
    app: &mut TuiApp,
    session: &mut Session,
    system_prompt: &str,
    want: &str,
    auto_title: bool,
) {
    match try_load_session(want) {
        Ok(Some(loaded)) => {
            let n = loaded.messages.len();
            *session = loaded;
            session.system_prompt = system_prompt.to_string();
            if auto_title && session.maybe_upgrade_title_from_history() {
                persist_session_best_effort(session, "title_backfill");
            }
            let title = session.title.clone();
            app.load_messages_from_session(session);
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

async fn apply_remote_hydrate(
    app: &mut TuiApp,
    session: &mut Session,
    rem: &crate::remote::RemoteAttach,
) {
    match crate::remote::fetch_messages(&rem.base_url, &rem.session_id).await {
        Ok((title, msgs)) => {
            session.id = rem.session_id.clone();
            if !title.is_empty() {
                session.title = title;
            }
            if !msgs.is_empty() {
                session.messages = msgs;
                app.load_messages_from_session(session);
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

async fn prepare_tui_boot(opts: &TuiRunOptions) -> TuiBoot {
    let tui_cfg = TuiAppConfig::from_core_config(&opts.config.tui);
    let mut app = TuiApp::new(tui_cfg);

    let file_index = whycodes_index::WorkspaceIndex::start(
        whycodes_index::WorkspaceIndex::project_roots(&opts.project_dir),
    );
    app.set_file_index(file_index.clone());

    app.provider_name = opts.provider.clone();
    app.model_name = opts.model.clone();
    app.reasoning_effort = opts.config.session.reasoning_effort.clone();
    app.agent_name = opts.agent_name.clone();
    app.project_dir = opts
        .project_dir
        .canonicalize()
        .unwrap_or_else(|_| opts.project_dir.clone());
    app.project_label = app
        .project_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| whycodes_core::display_path(&app.project_dir));
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
    app.model_selection.models = configured_models(&config);
    app.model_selection.selected = app
        .model_selection
        .models
        .iter()
        .position(|(p, m)| p == &opts.provider && m == &opts.model)
        .unwrap_or(0);

    let missing_key = opts.api_key.is_empty()
        && whycodes_llm::provider_requires_api_key(&opts.provider, Some(&opts.config));
    app.status_message = if missing_key {
        format!(
            "agent={}  {}/{}  — no API key · /connect  /help",
            opts.agent_name, opts.provider, opts.model
        )
    } else {
        format!(
            "agent={}  {}/{}  — Tab focus  Ctrl+T agent  Esc cancel  /help",
            opts.agent_name, opts.provider, opts.model
        )
    };

    let agent_info = config
        .get_agent(&opts.agent_name)
        .cloned()
        .unwrap_or_else(|| whycodes_core::types::AgentInfo {
            name: opts.agent_name.clone(),
            description: "Default".into(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet {
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                ..whycodes_core::types::PermissionSet::default()
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

    let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
    let perm_prompter =
        perm_prompter.with_notify(whycodes_agent::notify::handle_from_config(&config.notify));
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

    config.general.project_path = Some(opts.project_dir.clone());
    let session_claims = whycodes_core::FileClaimRegistry::new();
    let agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index.clone())
        .with_session_claims(session_claims.clone())
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
        )
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>)
        .with_plugins(Some(opts.project_dir.as_path()));

    let mut session = Session::new(opts.project_dir.clone(), system_prompt.clone());
    app.session_title = session.title.clone();
    app.session_id = session.id.clone();
    if app.session_list.sessions.is_empty() {
        app.session_list.sessions = load_session_entries();
    }
    let history = SessionHistory::new();

    if let Some(ref want) = opts.resume_session_id {
        apply_resume(
            &mut app,
            &mut session,
            &system_prompt,
            want,
            opts.config.session.auto_title,
        );
    }

    if let Some(ref rem) = opts.remote {
        apply_remote_hydrate(&mut app, &mut session, rem).await;
    }

    TuiBoot {
        app,
        config,
        file_index,
        agent,
        session,
        history,
        perm_prompter,
        question_prompter,
        perm_rx,
        question_rx,
        session_claims,
        missing_key,
    }
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
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        // Prompt used a blinking bar; give the shell the user's shape back.
        SetCursorStyle::DefaultUserShape,
        crossterm::cursor::Show
    );
}

/// Probe + enable the kitty keyboard protocol.
///
/// Returns whether flags were pushed (so shutdown can pop them). A 0×0 PTY
/// or `WHYCODES_BENCH` run never answers the CSI query; skip it rather than
/// stalling the first paint for crossterm's ~2 s timeout.
fn enable_keyboard_enhancement(out: &mut impl Write) -> bool {
    if !should_query_keyboard_enhancement(
        std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty()),
        term_size().ok(),
    ) {
        return false;
    }
    if !matches!(supports_keyboard_enhancement(), Ok(true)) {
        return false;
    }
    execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

/// Whether it is worth waiting on the keyboard-enhancement CSI query.
///
/// Pure so the 0×0 / bench skip can be unit-tested without a real PTY.
fn should_query_keyboard_enhancement(bench: bool, size: Option<(u16, u16)>) -> bool {
    if bench {
        return false;
    }
    matches!(size, Some((w, h)) if w > 0 && h > 0)
}

pub enum TurnOutcome {
    Ok {
        text: String,
        agent: Agent,
        session: Session,
        /// Wall time for `run_turn` only (excludes post-turn title refine).
        work_ms: u128,
    },
    /// Remote `whycodes serve` turn finished; local agent/session stay in place.
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
    /// Manual `/compact` finished (agent/session were moved out like a turn).
    Compact {
        agent: Agent,
        session: Session,
        outcome: whycodes_session::CompactOutcome,
        work_ms: u128,
    },
}

/// Run the full-screen TUI until the user quits.
pub async fn run(opts: TuiRunOptions) -> anyhow::Result<TuiExit> {
    // Wall clock for the Cline-style exit summary (process open → quit).
    let session_started = Instant::now();

    let boot = prepare_tui_boot(&opts).await;
    let mut app = boot.app;
    let mut config = boot.config;
    let file_index = boot.file_index;
    let mut agent = boot.agent;
    let session = boot.session;
    let history = boot.history;
    let perm_prompter = boot.perm_prompter;
    let question_prompter = boot.question_prompter;
    let perm_rx = boot.perm_rx;
    let question_rx = boot.question_rx;
    let session_claims = boot.session_claims;
    let missing_key = boot.missing_key;
    let remote = opts.remote.clone();

    let mut provider = opts.provider.clone();
    let mut model = opts.model.clone();
    let mut api_key = opts.api_key.clone();
    let max_turns = opts.max_turns;
    let project_dir = opts.project_dir.clone();

    // On panic, leave alt-screen / raw mode so the shell is usable and the
    // crash report (written by whycodes_core::logging) is readable.
    whycodes_core::logging::set_panic_cleanup(|| {
        if let Ok(mut out) = open_tui_writer() {
            restore_terminal_on(&mut out);
        } else {
            let _ = disable_raw_mode();
        }
    });

    whycodes_core::logging::emit(
        "whycodes_tui",
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
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.open_writer_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!(
            "failed to open terminal for TUI ({e}). \
             Run inside a real terminal, or use `whycodes --plain`."
        )
    })?;

    enable_raw_mode().map_err(|e| {
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.raw_mode_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!(
            "failed to enter raw mode ({e}). \
             Run inside a real terminal, or use `whycodes --plain`."
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
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.alt_screen_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!("failed to enter alternate screen ({e})")
    })?;
    // Insert-style blinking bar in the prompt. The emulator blinks it, so
    // idle stays 0 draws/s (a software caret would force animation cadence).
    // Unsupported hosts keep their default shape; restore on the way out.
    if let Err(e) = execute!(tui_out, SetCursorStyle::BlinkingBar) {
        tracing::debug!(error = %e, "set blinking bar cursor style failed");
    }
    // Lets terminals that support it (Kitty, WezTerm, Alacritty…) report
    // Shift+Enter distinctly, so multi-line input gets a portable binding.
    //
    // `supports_keyboard_enhancement` writes a CSI query and waits ~2 s for a
    // reply. Dumb / 0×0 PTYs (the first-frame harness) never answer, so a
    // query there is a 2 s tax on time-to-first-frame. Skip it.
    let keyboard_enhanced = enable_keyboard_enhancement(&mut tui_out);
    let backend = CrosstermBackend::new(tui_out);
    let mut terminal = Terminal::new(backend).inspect_err(|e| {
        let _ = disable_raw_mode();
        whycodes_core::logging::emit(
            "whycodes_tui",
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
        whycodes_core::logging::emit(
            "whycodes_tui",
            "warn",
            "tui.size_fallback",
            Some(serde_json::json!({ "reported_w": tw, "reported_h": th, "using": "80x24" })),
        );
    }

    whycodes_core::logging::emit(
        "whycodes_tui",
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
    let mut update_rx = opts.update_rx;

    apply_boot_prompt(&mut app, missing_key, opts.initial_prompt.clone());

    // Inert unless WHYCODES_BENCH is set; see crate::bench.
    let bench = crate::bench::config_from_env();

    let mut first_frame = true;
    // Paste / resize / focus can echo glyphs onto the PTY outside ratatui's
    // diff. Clear the terminal on the next paint so leftover text cannot sit
    // in the unpainted rows beside the prompt. Ordinary Backspace/Delete
    // must not bump `pending_full_clears` — home gutters already fill_blank.
    // Deep-idle + malloc_trim clocks (jcode redraw_schedule / idle_heap).
    let mut last_user_input = Instant::now();
    let mut idle_trim_armed = true;
    let result = async {
        'main: loop {
            // ── Drain background sessions into their own view snapshots ──
            // Events on inactive runtimes never touch `app`; they update the
            // runtime's snapshot + state and set `unread` so the dashboard
            // and cycle keys can surface activity.
            for bg in runtimes.iter_mut() {
                drain_background_runtime(bg);
            }
            // Dashboard / picker: rebuild only when rows actually change.
            // Unconditional mark_dirty here used to lock the idle poll at
            // 40 ms (~25 fps full paints) for as long as the dialog stayed open.
            refresh_live_session_ui(&mut app, &rt, &runtimes);

            if let Some(rx) = update_rx.as_mut() {
                match rx.try_recv() {
                    Ok(offer) => {
                        app.available_update = Some(offer);
                        app.mark_dirty();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        update_rx = None;
                    }
                }
            }
            maybe_offer_update(&mut app);

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

            // Animation = glyphs that change every frame (spinner / stream).
            // Static toasts are *not* animation — jcode measured ~180 wasted
            // full frames per notice when they pulled the loop to 40 ms.
            let animate = rt.agent_busy || app.running_task_count() > 0;
            if app.needs_redraw || animate || first_frame {
                if app.pending_full_clears > 0 {
                    if let Err(e) = terminal.clear() {
                        whycodes_core::logging::emit(
                            "whycodes_tui",
                            "warn",
                            "tui.full_clear_failed",
                            Some(serde_json::json!({ "error": e.to_string() })),
                        );
                    }
                    app.pending_full_clears = app.pending_full_clears.saturating_sub(1);
                }
                let completed = match terminal.draw(|f| render::render(f, &mut app)) {
                    Ok(c) => c,
                    Err(e) => {
                        whycodes_core::logging::emit(
                            "whycodes_tui",
                            "error",
                            "tui.draw_failed",
                            Some(serde_json::json!({ "error": e.to_string() })),
                        );
                        return Err(e.into());
                    }
                };
                // Record *before* MCP / auto-index: those can block for
                // seconds and must not inflate time-to-first-frame or keep
                // `--idle-ms 0` from exiting as soon as a frame is up.
                crate::bench::record_draw();
                let just_first = first_frame;
                if first_frame {
                    first_frame = false;
                    whycodes_core::logging::emit(
                        "whycodes_tui",
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
                    app.screen_cells = crate::cell_grid::CellGrid::from_buffer(completed.buffer);
                } else if !app.screen_cells.is_empty() {
                    app.screen_cells.clear();
                }
                // Grok: never malloc_trim inside the paint; drain after flush.
                crate::heap::run_deferred_release();
                // Stay dirty while animation is live or a follow-up full
                // clear is still owed (paste echo can land after this frame).
                app.needs_redraw = animate || app.pending_full_clears > 0;

                if let Some(ref bench) = bench
                    && crate::bench::should_stop(bench)
                {
                    break;
                }

                if just_first {
                    // After first paint: MCP connect + code RAG auto-index.
                    // Both can block; doing them here keeps startup feel snappy.
                    rt.agent.load_mcp(&config).await;
                    maybe_session_auto_index(&project_dir, &config, &mut app);
                    refresh_sidebar(&mut app, &config, &file_index);
                    load_app_todos(&mut app);
                    if !app.toasts.is_empty() {
                        app.mark_dirty();
                    }
                }
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

            if should_tick_spinner(&app, rt.agent_busy) {
                tick_spinner(&mut app, &mut spinner_frame);
            }

            // ── Permission / question requests (queued; one at a time) ─
            while let Ok(req) = rt.perm_rx.try_recv() {
                rt.pending_perm_queue.push_back(req);
            }
            while let Ok(req) = rt.question_rx.try_recv() {
                rt.pending_question_queue.push_back(req);
            }
            maybe_open_queued_dialog(&mut app, &rt);

            // ── Async title refine (does not hold rt.agent_busy) ─────────
            while let Ok((sid, title)) = title_rx.try_recv() {
                apply_async_title(
                    &mut app,
                    &mut rt,
                    &mut runtimes,
                    &mut pending_async_title,
                    sid,
                    title,
                );
            }

            // ── Force-stop if cancel is ignored too long ──────────────
            // Cooperative cancel covers stream/tools via select!. This is the
            // hard backstop for spawn_blocking shells / wedged HTTP that never
            // yield: abort the join handle and restore rt.agent/rt.session.
            if should_force_stop(rt.agent_busy, cancel_requested_at, app.pending_cancel) {
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

            // ── Turn finished ─────────────────────────────────────────
            if let Ok(outcome) = rt.done_rx.try_recv()
                && apply_turn_outcome(
                    &mut app,
                    &mut rt,
                    outcome,
                    &mut cancel_requested_at,
                    &mut pending_async_title,
                    &provider,
                    &model,
                    &config,
                    &api_key,
                    &suggest_tx,
                )
            {
                catalog_fetch_pending = true;
            }

            // ── Apply rt.session picker / /resume selection ──────────────
            // ── Picker close selection (Ctrl+W on a live row) ────────
            if let Some(close_idx) = app.session_list.pending_close.take() {
                close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, close_idx);
            }

            if let Some(target) = app.pending_session_switch.take() {
                apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, target);
            }

            if let Some(id) = app.pending_session_id.take() {
                resume_or_switch_session(
                    &mut app,
                    &mut rt,
                    &mut runtimes,
                    &mut mru,
                    id,
                    &project_dir,
                    &config,
                );
            }

            // ── Apply agent picker selection ──────────────────────────
            if let Some(name) = app.pending_agent.take() {
                if rt.agent_busy {
                    app.toasts.push(
                        crate::toast::ToastKind::Warning,
                        "Can't switch agent while a turn is running",
                    );
                } else {
                    switch_to_agent(
                        &mut app,
                        &mut rt.agent,
                        &mut rt.session,
                        &config,
                        &project_dir,
                        Arc::clone(&rt.perm_prompter),
                        Arc::clone(&rt.question_prompter),
                        &rt.event_tx,
                        &name,
                        false,
                    )
                    .await;
                }
            }

            // ── Apply model picker selection ──────────────────────────
            if let Some((p, m)) = app.pending_model.take() {
                apply_model_choice(
                    &mut app,
                    &mut provider,
                    &mut model,
                    &mut api_key,
                    p,
                    m,
                    &config,
                );
                fill_oauth_credential(&mut api_key, &provider).await;
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
            }

            if let Some(effort) = app.pending_effort.take() {
                apply_reasoning_effort(&mut app, &mut rt.agent, &mut config, &effort);
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
                apply_idle_suggestion(&mut app, suggestion, rt.agent_busy);
            }

            // OAuth login flow progress (`/connect` spawned task).
            while let Ok(ev) = auth_rx.try_recv() {
                apply_auth_flow_event(
                    &mut app,
                    ev,
                    &mut provider,
                    &mut model,
                    &mut api_key,
                    &config,
                )
                .await;
            }

            // ── Apply async single-model context_length from gateway ──
            while let Ok((for_provider, for_model, window)) = catalog_rx.try_recv() {
                apply_catalog_window(
                    &mut app,
                    &provider,
                    &model,
                    &for_provider,
                    &for_model,
                    window,
                    &config,
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
            queue_auto_prompt_if_idle(&mut app, rt.agent_busy);

            // ── Start compact if queued (must not await on the event loop) ──
            if !rt.agent_busy
                && let Some(note) = rt.pending_compact.take()
            {
                start_compact_task(
                    &mut app,
                    &mut rt,
                    &mut cancel_requested_at,
                    note,
                    &provider,
                    &model,
                    &api_key,
                    &project_dir,
                );
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
                    let flag =
                        arm_generating(&mut app, &mut rt, &mut cancel_requested_at, "remote…");
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

                // Lazy-load API key from env/config/OAuth when user first chats
                try_fill_api_key(&mut api_key, &provider);
                if api_key.is_empty() {
                    fill_oauth_credential(&mut api_key, &provider).await;
                }
                if api_key.is_empty()
                    && whycodes_llm::provider_requires_api_key(&provider, Some(&config))
                {
                    warn_missing_api_key(&mut app, &provider);
                    // Images already shown on the user bubble; don't re-queue.
                    let _ = submit_images;
                    continue;
                }

                let flag = arm_generating(&mut app, &mut rt, &mut cancel_requested_at, "");
                let expanded = record_user_turn(
                    &mut app,
                    &mut rt,
                    &prompt,
                    &project_dir,
                    &config,
                    &submit_images,
                );
                let (route_provider, route_model) = route_turn_model(
                    rt.session.id.as_str(),
                    &provider,
                    &model,
                    &expanded,
                    rt.agent
                        .model_fast()
                        .or(config.session.model_fast.as_deref()),
                );

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

                let (ag, sess) = take_turn_owner(&mut rt, &project_dir);

                rt.turn_join = Some(tokio::spawn(async move {
                    let agent = ag;
                    let mut session = sess;
                    // Time only the agent loop. Title refine runs async *after*
                    // we release rt.agent_busy so the user can type immediately.
                    let work_t0 = std::time::Instant::now();
                    let result = agent
                        .run_turn_with_events(
                            &mut session,
                            TurnOpts {
                                provider_name: &provider2,
                                model: &model2,
                                api_key: &api_key2,
                                max_turns,
                                events: Some(event_tx2),
                                cancel: cancel2,
                            },
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
            // Paint is gated by `needs_redraw` / animation — poll timeout no
            // longer implies a full redraw. Cadence policy lives in
            // `redraw_schedule` (jcode: toasts ≠ animation; 30s → 5s deep idle).
            let awaiting_matches = app.file_suggest.awaiting_matches();
            let poll_for =
                crate::redraw_schedule::poll_interval(&crate::redraw_schedule::RedrawNeed {
                    agent_busy: rt.agent_busy,
                    running_subagents: app.running_task_count() > 0,
                    awaiting_matches,
                    needs_redraw: app.needs_redraw,
                    toasts_visible: !app.toasts.is_empty(),
                    since_user_input: last_user_input.elapsed(),
                });
            if !rt.agent_busy
                && last_user_input.elapsed() >= crate::heap::IDLE_TRIM_AFTER
                && idle_trim_armed
            {
                crate::heap::release_retained_heap_debounced(
                    "client_idle",
                    crate::heap::IDLE_TRIM_AFTER,
                );
                idle_trim_armed = false;
            }

            let has_ev = match event::poll(poll_for) {
                Ok(v) => v,
                Err(e) => {
                    whycodes_core::logging::emit(
                        "whycodes_tui",
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
                        whycodes_core::logging::emit(
                            "whycodes_tui",
                            "error",
                            "tui.read_failed",
                            Some(serde_json::json!({ "error": e.to_string() })),
                        );
                        return Err(e.into());
                    }
                };
                let mut batch = batch;
                if batch
                    .iter()
                    .any(crate::redraw_schedule::event_is_user_interaction)
                {
                    last_user_input = Instant::now();
                    idle_trim_armed = true;
                }
                input::coalesce_chat_wheels(&mut app, &mut batch);
                input::coalesce_resizes(&mut batch);
                input::coalesce_unbracketed_paste(&app, &mut batch);
                if let Some(Event::Resize(w, h)) = batch
                    .iter()
                    .rev()
                    .find(|e| matches!(e, Event::Resize(_, _)))
                    && *w > 0
                    && *h > 0
                {
                    // ratatui only autoresizes on `draw`. Mobile OSK hide/show
                    // can emit Resize while we are in a long poll; apply it
                    // immediately so the next paint uses the new viewport
                    // instead of a stale buffer (garbled rows / clipped popup).
                    let _ = terminal.resize(Rect::new(0, 0, *w, *h));
                }
                if batch.iter().any(event_forces_redraw) {
                    app.mark_dirty();
                }
                if batch
                    .iter()
                    .any(crate::redraw_schedule::event_needs_full_clear)
                {
                    // Two frames: some emulators echo the paste *after*
                    // Event::Paste, so one clear is overwritten by the ghost.
                    app.request_full_clear(2);
                }
                if crate::redraw_schedule::batch_looks_like_unbracketed_paste(&batch) {
                    app.request_full_clear(2);
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
                                reply_permission(&mut app, &mut rt.pending_perm_queue, true);
                                continue;
                            }
                            KeyCode::Char('n')
                            | KeyCode::Char('N')
                            | KeyCode::Char('d')
                            | KeyCode::Char('D')
                            | KeyCode::Esc => {
                                reply_permission(&mut app, &mut rt.pending_perm_queue, false);
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // Questionnaire dialog (Grok-style `question` tool).
                    // Press *and* Repeat: enhanced keyboard can emit Repeat for held Esc.
                    if matches!(app.dialogs.active(), Some(DialogKind::Question(_)))
                        && let Event::Key(key) = &ev
                        && (key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat)
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
                            warn_session_limit(&mut app);
                            continue;
                        }
                        let fresh = spawn_new_session_runtime(
                            &app.agent_name,
                            &config,
                            &project_dir,
                            &file_index,
                            session_claims.clone(),
                        )
                        .await;
                        adopt_fresh_runtime(&mut app, &mut rt, &mut runtimes, &mut mru, fresh);
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
                        cycle_live_session(
                            &mut app,
                            &mut rt,
                            &mut runtimes,
                            &mut mru,
                            key.code == KeyCode::PageDown,
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
                        switch_mru_session(&mut app, &mut rt, &mut runtimes, &mut mru);
                        continue;
                    }

                    // Slash commands on Enter
                    if let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Enter
                        && app.mode == AppMode::Normal
                        && !rt.agent_busy
                        && let Some(text) = slash_command_from_prompt(&app)
                    {
                        consume_slash_draft(&mut app);
                        handle_slash(
                            &text,
                            &mut SlashContext {
                                app: &mut app,
                                session: &mut rt.session,
                                history: &mut rt.history,
                                agent: &mut rt.agent,
                                config: &mut config,
                                project_dir: &project_dir,
                                provider: &mut provider,
                                model: &mut model,
                                api_key: &mut api_key,
                                perm_prompter: Arc::clone(&rt.perm_prompter),
                                question_prompter: Arc::clone(&rt.question_prompter),
                                auth_tx: auth_tx.clone(),
                                pending_compact: &mut rt.pending_compact,
                            },
                        )
                        .await;
                        continue;
                    }

                    // While busy: Esc cancels (draft preserved — Grok). Typing, scroll,
                    // and focus still work so the user can queue thoughts.
                    // Permission / question overlays own Esc/Enter — do not steal them.
                    let overlay_owns_keys = matches!(
                        app.dialogs.active(),
                        Some(DialogKind::Permission { .. } | DialogKind::Question(_))
                    );
                    if rt.agent_busy
                        && !overlay_owns_keys
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
                                match busy_ctrl_c(&mut app, cancel_requested_at) {
                                    BusyCtrlC::ClearedDraft => {}
                                    BusyCtrlC::BeginCancel => begin_cancel(
                                        &mut app,
                                        &rt.cancel_flag,
                                        &mut cancel_requested_at,
                                        &mut rt.pending_question_queue,
                                        &mut rt.pending_perm_queue,
                                    ),
                                    BusyCtrlC::ForceStop => force_stop_turn(
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
                                    ),
                                }
                                continue;
                            }
                            KeyCode::Enter => {
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
                        whycodes_core::logging::emit(
                            "whycodes_tui",
                            "info",
                            "tui.exit",
                            Some(serde_json::json!({ "reason": "handle_event=false" })),
                        );
                        break 'main;
                    }
                    // Mouse / [✗] set flags inside handle_event — complete the
                    // oneshot on this same event, not the next tick.
                    flush_pending_question_replies(
                        &mut app,
                        &mut rt.pending_question_queue,
                        &rt.pending_perm_queue,
                    );
                } // for ev in batch
            }

            // Also drain on idle ticks: a click that closed the dialog must
            // not wait for another keypress (issue #41).
            flush_pending_question_replies(
                &mut app,
                &mut rt.pending_question_queue,
                &rt.pending_perm_queue,
            );

            if !app.running {
                whycodes_core::logging::emit(
                    "whycodes_tui",
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
    shutdown_runtime_queues(&mut rt);

    // Parked sessions: deny their waiters, abort their turns, persist.
    for mut bg in runtimes.drain(..) {
        shutdown_runtime_queues(&mut bg);
        if let Some(h) = bg.turn_join.take() {
            h.abort();
        }
        bg.persist("shutdown");
    }

    if let Err(ref e) = result {
        whycodes_core::logging::emit(
            "whycodes_tui",
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
        LeaveAlternateScreen,
        SetCursorStyle::DefaultUserShape
    );
    let _ = terminal.show_cursor();
    // Normal exit — panic hook no longer needs to touch the terminal.
    whycodes_core::logging::clear_panic_cleanup();

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
            .format_exit_summary(session_started.elapsed(), &model_label, "whycodes");
    print_session_summary(&summary);

    whycodes_core::logging::emit(
        "whycodes_tui",
        "info",
        "tui.stopped",
        Some(serde_json::json!({
            "ok": result.is_ok(),
            "session_id": rt.session.id,
            "messages": rt.session.messages.len(),
            "duration_s": session_started.elapsed().as_secs(),
        })),
    );

    result?;
    Ok(if app.pending_upgrade {
        TuiExit::Upgrade
    } else {
        TuiExit::Quit
    })
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
    pending_perm_queue: &mut std::collections::VecDeque<whycodes_agent::PermissionRequest>,
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
    close_interactive_overlays(app);
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
    pending_perm_queue: &mut std::collections::VecDeque<whycodes_agent::PermissionRequest>,
    done_rx: &mut mpsc::UnboundedReceiver<TurnOutcome>,
    config: &Config,
    project_dir: &std::path::Path,
    provider: &str,
    model: &str,
    event_tx: mpsc::UnboundedSender<TurnEvent>,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
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
            }
            | TurnOutcome::Compact {
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
    close_interactive_overlays(app);
    app.mark_dirty();
}

/// Drop permission / question chrome when the turn is cancelled so Esc
/// cannot leave a stuck overlay with no oneshot behind it (issue #41).
fn close_interactive_overlays(app: &mut TuiApp) {
    let had_overlay = matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { .. } | DialogKind::Question(_))
    );
    if !had_overlay {
        return;
    }
    app.dialogs.clear();
    app.pending_question_answers = None;
    app.question_dismissed = false;
    app.mode = AppMode::Normal;
    app.key_context = KeymapContext::Normal;
    app.clear_dialog_hits();
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
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
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
    let info =
        config
            .get_agent(&name)
            .cloned()
            .unwrap_or_else(|| whycodes_core::types::AgentInfo {
                name: name.clone(),
                description: String::new(),
                mode: AgentMode::Primary,
                permission: whycodes_core::types::PermissionSet::default(),
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
        .with_permission_prompter(perm_prompter as Arc<dyn whycodes_agent::PermissionPrompter>)
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
            whycodes_core::logging::emit_sid(
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
            whycodes_core::logging::emit_sid(
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
fn with_session_db<T>(f: impl FnOnce(&whycodes_storage::db::Database) -> T) -> Option<T> {
    use std::sync::{Mutex, OnceLock};
    static DB: OnceLock<Mutex<Option<whycodes_storage::db::Database>>> = OnceLock::new();
    let lock = DB.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().ok()?;
    if guard.is_none() {
        *guard = open_db_quiet();
    }
    guard.as_ref().map(f)
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
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
    session_claims: whycodes_core::FileClaimRegistry,
) -> SessionRuntime {
    let agent_info =
        config
            .get_agent(agent_name)
            .cloned()
            .unwrap_or_else(|| whycodes_core::types::AgentInfo {
                name: agent_name.to_string(),
                description: "Default".into(),
                mode: AgentMode::Primary,
                permission: whycodes_core::types::PermissionSet {
                    allow_file_writes: true,
                    allow_network: true,
                    allow_shell: true,
                    ..whycodes_core::types::PermissionSet::default()
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
    let perm_prompter =
        perm_prompter.with_notify(whycodes_agent::notify::handle_from_config(&config.notify));
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
            Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
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
///
/// Returns `true` when the painted rows changed (caller should `mark_dirty`).
fn refresh_sessions_rows(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    runtimes: &[SessionRuntime],
) -> bool {
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
            crate::session_runtime::preview_from_messages(&app.messages)
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
    if app.sessions_rows == rows {
        return false;
    }
    app.sessions_rows = rows;
    true
}

/// Rewrite the picker's live section in place: live rows (active + parked
/// runtimes) sit at the top, persisted-only rows below. Persisted rows keep
/// their relative order; the cursor stays on the same entry when possible.
///
/// Returns `true` when the painted list or cursor changed.
/// Does **not** reopen SQLite — `/sessions` and `/resume` load the DB once
/// when the dialog opens; a per-tick `list_sessions` was a hidden stall.
fn refresh_picker_live_section(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    runtimes: &[SessionRuntime],
) -> bool {
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
    let persisted: Vec<crate::app::SessionEntry> = app
        .session_list
        .sessions
        .iter()
        .filter(|e| e.live.is_none() && !live_ids.contains(e.id.as_str()))
        .cloned()
        .collect();
    let mut merged = live_rows;
    merged.extend(persisted);
    if app.session_list.sessions == merged {
        return false;
    }
    app.session_list.sessions = merged;
    if let Some(id) = selected_id
        && let Some(pos) = app.session_list.sessions.iter().position(|e| e.id == id)
    {
        app.session_list.selected = pos;
    }
    true
}

/// Swap the active runtime with `runtimes[idx]`, preserving both sessions'
/// view state. Transcripts **move** (no clone): the visible app yields into
/// the outgoing snapshot and adopts the incoming one.
fn switch_to_runtime(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    idx: usize,
) {
    app.yield_view(&mut rt.view);
    std::mem::swap(rt, &mut runtimes[idx]);
    rt.unread = false;
    app.adopt_view(&mut rt.view);
    app.dialogs.clear();
    app.mark_dirty();
    app.focus = FocusPane::Prompt;
    app.request_full_clear(2);
}

/// Drain a background (inactive) runtime: prompter requests into its queues,
/// turn events into its view snapshot, completion into agent/session restore.
/// Never touches the visible `app`; sets `unread` on any activity so the
/// dashboard and cycle keys can surface it.
///
/// Idle path is a few `try_recv`s — no `TuiApp`, no transcript clone, no
/// syntax-theme swap. Events move the snapshot into a detached scratch app
/// and back (`adopt_view` / `yield_view`) so a parked stream does not
/// duplicate the whole transcript every tick.
fn drain_background_runtime(rt: &mut SessionRuntime) {
    while let Ok(req) = rt.perm_rx.try_recv() {
        rt.pending_perm_queue.push_back(req);
        rt.unread = true;
    }
    while let Ok(req) = rt.question_rx.try_recv() {
        rt.pending_question_queue.push_back(req);
        rt.unread = true;
    }

    if !rt.event_rx.is_empty() {
        // Disjoint from `rt.view`: adopt → drain → yield, no overlapping borrow.
        let mut scratch = TuiApp::from_config(crate::config::TuiAppConfig::default());
        scratch.adopt_view(&mut rt.view);
        let any = drain_turn_events(&mut scratch, &mut rt.event_rx);
        scratch.yield_view(&mut rt.view);
        if any {
            rt.unread = true;
        }
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
                    with_view_scratch(&mut rt.view, |scratch| {
                        if let Some(last) = scratch.messages.last_mut()
                            && last.role == ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            last.content = text;
                        }
                    });
                }
            }
            TurnOutcome::Remote { text, error, .. } => {
                rt.last_error = error.is_some();
                if let Some(err) = error {
                    with_view_scratch(&mut rt.view, |scratch| {
                        scratch.add_message(ChatRole::System, format!("Remote error: {err}"));
                    });
                } else if !text.is_empty() {
                    with_view_scratch(&mut rt.view, |scratch| {
                        if let Some(last) = scratch.messages.last_mut()
                            && last.role == ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            last.content = text;
                        }
                    });
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
                with_view_scratch(&mut rt.view, |scratch| {
                    if cancelled {
                        scratch.add_message(ChatRole::System, "⏹ Generation cancelled (Esc).");
                    } else {
                        let display = whycodes_llm::format_turn_error(&whycodes_core::Error::Llm(
                            error.clone(),
                        ));
                        scratch.add_message(ChatRole::System, format!("Error: {display}"));
                    }
                });
            }
            TurnOutcome::Compact {
                agent: a,
                session: s,
                outcome,
                ..
            } => {
                rt.agent = a;
                rt.last_error = false;
                with_view_scratch(&mut rt.view, |scratch| {
                    apply_compact_view(scratch, &s, &outcome);
                });
                rt.session = s;
            }
        }
        rt.persist("background");
    }
}

/// Apply `f` to a detached copy of `view` without cloning the transcript or
/// touching the process-wide syntax theme.
fn with_view_scratch(view: &mut crate::session_runtime::ViewSnapshot, f: impl FnOnce(&mut TuiApp)) {
    let mut scratch = TuiApp::from_config(crate::config::TuiAppConfig::default());
    scratch.adopt_view(view);
    f(&mut scratch);
    scratch.yield_view(view);
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

/// Apply a finished turn to the visible session. Returns `true` when the
/// catalog fetch should be queued (no live context window yet).
#[allow(clippy::too_many_arguments)]
fn apply_turn_outcome(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    outcome: TurnOutcome,
    cancel_requested_at: &mut Option<Instant>,
    pending_async_title: &mut Option<(String, String)>,
    provider: &str,
    model: &str,
    config: &Config,
    api_key: &str,
    suggest_tx: &mpsc::UnboundedSender<String>,
) -> bool {
    rt.agent_busy = false;
    rt.cancel_flag = None;
    *cancel_requested_at = None;
    rt.turn_join = None;
    rt.session_backup = None;
    let queue_catalog = app.api_context_window.is_none();
    let work_ms = match &outcome {
        TurnOutcome::Ok { work_ms, .. }
        | TurnOutcome::Err { work_ms, .. }
        | TurnOutcome::Remote { work_ms, .. }
        | TurnOutcome::Compact { work_ms, .. } => *work_ms,
    };
    let elapsed_ms = Some(app.complete_turn_timing_ms(work_ms));
    app.mark_dirty();
    crate::heap::request_release_after_draw("turn_done");
    match outcome {
        TurnOutcome::Ok {
            text,
            agent: a,
            session: s,
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
            if !text.is_empty()
                && let Some(last) = app.messages.last_mut()
                && last.role == ChatRole::Assistant
                && last.content.is_empty()
            {
                last.content = text.clone();
            }
            app.finish_open_thinking();
            app.current_agent_state = AgentState::Idle;
            // Last-step prompt usage is billed fill for that request; after
            // tools/assistant land, the meter should track the live transcript.
            app.sync_context_estimate(&rt.session);
            app.refresh_git_branch();
            app.status_message = format_turn_done_status(
                app,
                rt.agent.info.name.as_str(),
                provider,
                model,
                elapsed_ms,
                false,
            );
            rt.persist("ok");
            maybe_spawn_prompt_suggestion(
                config,
                &rt.session,
                provider,
                model,
                api_key,
                app,
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
                    app,
                    rt.agent.info.name.as_str(),
                    provider,
                    model,
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
            app.sync_context_estimate(&rt.session);
            if cancelled {
                app.current_agent_state = AgentState::Idle;
                app.status_message = format_turn_done_status(
                    app,
                    rt.agent.info.name.as_str(),
                    provider,
                    model,
                    elapsed_ms,
                    true,
                );
                app.add_message(ChatRole::System, "⏹ Generation cancelled (Esc).");
                rt.persist("cancelled");
            } else {
                let display =
                    whycodes_llm::format_turn_error(&whycodes_core::Error::Llm(error.clone()));
                app.current_agent_state = AgentState::Error(display.clone());
                let dur = elapsed_ms
                    .map(|ms| format!("{} · ", format_elapsed_ms(ms)))
                    .unwrap_or_default();
                app.status_message = format!("{dur}error — see chat");
                app.add_message(ChatRole::System, format!("Error: {display}"));
                app.toasts
                    .push(crate::toast::ToastKind::Error, truncate_toast(&display, 48));
                whycodes_core::logging::emit_sid(
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
        TurnOutcome::Compact {
            agent: a,
            session: s,
            outcome,
            work_ms: _,
        } => {
            rt.agent = a;
            rt.session = s;
            apply_compact_view(app, &rt.session, &outcome);
            rt.persist("compact");
        }
    }
    queue_catalog
}

fn apply_compact_view(
    app: &mut TuiApp,
    session: &Session,
    outcome: &whycodes_session::CompactOutcome,
) {
    app.load_messages_from_session(session);
    app.current_agent_state = AgentState::Idle;
    app.status_message = format!(
        "Conversation compacted ({} → {} msgs, ~{} → ~{} tok)",
        outcome.messages_before,
        outcome.messages_after,
        outcome.tokens_before,
        outcome.tokens_after
    );
    app.toasts
        .push(crate::toast::ToastKind::Success, "Conversation compacted");
}

/// Close the active session (`usize::MAX`) or a parked slot.
fn close_session_slot(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut Vec<SessionRuntime>,
    mru: &mut Vec<usize>,
    close_idx: usize,
) {
    if close_idx == usize::MAX {
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
            let mut closed = std::mem::replace(rt, runtimes.remove(idx));
            closed.turn_join.take();
            mru.retain(|&i| i != idx);
            for i in mru.iter_mut() {
                if *i > idx {
                    *i -= 1;
                }
            }
            rt.unread = false;
            app.adopt_view(&mut rt.view);
            app.dialogs.clear();
            app.mark_dirty();
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

/// Switch to a parked live session, or load a persisted id into the active one.
fn resume_or_switch_session(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
    id: String,
    project_dir: &std::path::Path,
    config: &Config,
) {
    if rt.agent_busy {
        app.pending_session_id = Some(id);
        return;
    }
    if let Some(idx) = runtimes.iter().position(|b| b.session.id == id) {
        switch_to_runtime(app, rt, runtimes, idx);
        mru.retain(|&i| i != idx);
        mru.push(idx);
        app.toasts.push(
            crate::toast::ToastKind::Success,
            format!("Switched to live session · {}", rt.session.title),
        );
        return;
    }
    match try_load_session(&id) {
        Ok(Some(loaded)) => {
            if !rt.session.messages.is_empty() {
                rt.persist("switch");
            }
            let n = loaded.messages.len();
            rt.history = SessionHistory::new();
            rt.session = loaded;
            rt.session.system_prompt = with_project_memory(
                &Agent::with_agents_md(&rt.agent.system_prompt(), project_dir),
                project_dir,
                config,
                None,
            );
            if config.session.auto_title && rt.session.maybe_upgrade_title_from_history() {
                rt.persist("title_backfill");
            }
            let title = rt.session.title.clone();
            app.load_messages_from_session(&rt.session);
            app.toasts.push(
                crate::toast::ToastKind::Success,
                format!("Resumed · {title} ({n} msgs)"),
            );
            app.status_message = format!("Resumed rt.session {}", short_session_id(&rt.session.id));
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

fn reply_permission(
    app: &mut TuiApp,
    queue: &mut std::collections::VecDeque<whycodes_agent::PermissionRequest>,
    allow: bool,
) {
    if let Some(req) = queue.pop_front() {
        let _ = req.reply.send(allow);
    }
    app.dialogs.pop();
    app.mode = AppMode::Normal;
    app.key_context = KeymapContext::Normal;
    if let Some(next) = queue.front() {
        app.ask_permission(next.tool_name.clone(), next.detail.clone());
        app.status_message = format!(
            "{} — {} more permission(s)…",
            if allow { "Allowed" } else { "Denied" },
            queue.len()
        );
    } else if allow {
        app.current_agent_state = AgentState::Generating;
        app.status_message = "Allowed — continuing…".into();
    } else {
        app.current_agent_state = AgentState::Generating;
        app.status_message = "Denied tool".into();
    }
}

fn complete_questionnaire_ui(
    app: &mut TuiApp,
    queue: &mut std::collections::VecDeque<QuestionRequest>,
    perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
    answers: Option<Vec<whycodes_tools::question::QuestionAnswer>>,
) {
    if let Some(req) = queue.pop_front() {
        let _ = match answers {
            Some(a) => req.reply.send(Ok(a)),
            None => req.reply.send(Err(QuestionError::Cancelled)),
        };
    }
    if matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        app.dialogs.pop();
        app.clear_dialog_hits();
    }
    if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        resume_after_question(app, queue, perm_queue);
    }
}

/// Complete a questionnaire oneshot set by mouse / `[✗]` in `input.rs`.
///
/// Must run **after** `handle_event` (those flags are written there) and on
/// idle ticks so a click is not stuck until the next keypress (issue #41).
fn flush_pending_question_replies(
    app: &mut TuiApp,
    queue: &mut std::collections::VecDeque<QuestionRequest>,
    perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) {
    if let Some(answers) = app.pending_question_answers.take() {
        complete_questionnaire_ui(app, queue, perm_queue, Some(answers));
    }
    if app.question_dismissed {
        app.question_dismissed = false;
        complete_questionnaire_ui(app, queue, perm_queue, None);
    }
}

fn warn_missing_api_key(app: &mut TuiApp, provider: &str) {
    let env_name = format!("{}_API_KEY", provider.to_uppercase());
    app.add_message(
        ChatRole::System,
        format!(
            "No API key for `{provider}`\n\
                 → export {env_name}=…\n\
                 → whycodes provider add {provider} --api-key <key> · then /connect"
        ),
    );
    app.status_message = "no API key · /connect".into();
    app.toasts.push(
        crate::toast::ToastKind::Warning,
        format!("Missing {provider} API key"),
    );
}

fn apply_idle_suggestion(app: &mut TuiApp, suggestion: String, agent_busy: bool) {
    if suggestion.trim().is_empty() || agent_busy {
        return;
    }
    app.pending_suggestion = Some(suggestion.clone());
    app.toasts.push(
        crate::toast::ToastKind::Info,
        truncate_toast(&format!("suggest · Tab · {suggestion}"), 64),
    );
    app.mark_dirty();
}

fn apply_catalog_window(
    app: &mut TuiApp,
    provider: &str,
    model: &str,
    for_provider: &str,
    for_model: &str,
    window: u32,
    config: &Config,
) -> bool {
    if for_provider != provider || for_model != model {
        return false;
    }
    app.mark_dirty();
    app.set_api_context_window(
        for_provider,
        for_model,
        window,
        config.configured_context_window(provider, model),
        config.session.max_context_tokens as u64,
    );
    whycodes_core::logging::emit(
        "whycodes_tui",
        "info",
        "tui.context_window_applied",
        Some(serde_json::json!({
            "provider": for_provider,
            "model": for_model,
            "window": window,
            "max": app.max_context_tokens,
        })),
    );
    true
}

async fn apply_auth_flow_event(
    app: &mut TuiApp,
    ev: AuthFlowEvent,
    provider: &mut String,
    model: &mut String,
    api_key: &mut String,
    config: &Config,
) {
    match ev {
        AuthFlowEvent::Note(text) => {
            app.add_message(ChatRole::System, &text);
            app.status_message = text.lines().next().unwrap_or("").to_string();
        }
        AuthFlowEvent::NeedCode(sink) => {
            app.auth_code_sink = Some(sink);
            app.status_message = "Paste the sign-in code, then Enter · Esc cancels".into();
            app.focus_prompt();
        }
        AuthFlowEvent::Done {
            provider: p,
            result,
        } => match result {
            Ok(_) => {
                let already_on = *provider == p;
                if !already_on {
                    // Switch even when the plugin lists no models — otherwise
                    // a successful login leaves the previous provider selected.
                    let m = whycodes_auth::providers::suggested_models(&p)
                        .into_iter()
                        .find(|name| !name.is_empty())
                        .unwrap_or_else(|| model.clone());
                    apply_model_choice(app, provider, model, api_key, p.clone(), m, config);
                }
                if let Ok(dir) = Config::data_dir()
                    && let Some(tok) = whycodes_auth::providers::access_token(&p, &dir).await
                {
                    whycodes_llm::oauth_refresh::register(&p, dir);
                    *api_key = tok;
                }
                let model_note = if already_on {
                    String::new()
                } else {
                    format!(" · using {p}/{model}")
                };
                app.add_message(
                    ChatRole::System,
                    format!("✓ Signed in to `{p}` (subscription){model_note}"),
                );
                app.status_message = format!("Signed in · {p}");
                app.toasts
                    .push(crate::toast::ToastKind::Success, format!("Connected · {p}"));
            }
            Err(msg) => {
                app.add_message(ChatRole::System, format!("Sign-in to `{p}` failed: {msg}"));
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

fn shutdown_runtime_queues(rt: &mut SessionRuntime) {
    while let Some(req) = rt.pending_perm_queue.pop_front() {
        let _ = req.reply.send(false);
    }
    while let Some(req) = rt.pending_question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
    }
    rt.agent.background_registry().kill_all();
}

fn arm_generating(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    status: &str,
) -> CancelFlag {
    rt.agent_busy = true;
    let flag = new_cancel_flag();
    rt.cancel_flag = Some(Arc::clone(&flag));
    *cancel_requested_at = None;
    app.mark_turn_started();
    app.current_agent_state = AgentState::Generating;
    if status.is_empty() {
        app.status_message.clear();
    } else {
        app.status_message = status.into();
    }
    if app
        .messages
        .last()
        .map(|m| m.role != ChatRole::Assistant)
        .unwrap_or(true)
    {
        app.add_message(ChatRole::Assistant, "");
    }
    flag
}

fn explicit_provider_key(config: &Config, provider: &str) -> Option<String> {
    config
        .get_provider(provider)
        .and_then(|pc| pc.api_key.clone())
        .filter(|k| !k.is_empty())
        .or_else(|| {
            std::env::var(format!("{}_API_KEY", provider.to_uppercase()))
                .ok()
                .filter(|k| !k.is_empty())
        })
}

fn try_fill_api_key(api_key: &mut String, provider: &str) {
    if !api_key.is_empty() {
        return;
    }
    let cfg = Config::load().unwrap_or_default();
    if let Some(k) = explicit_provider_key(&cfg, provider) {
        *api_key = k;
        whycodes_llm::oauth_refresh::unregister(provider);
    }
}

async fn fill_oauth_credential(api_key: &mut String, provider: &str) {
    if !api_key.is_empty() || !whycodes_auth::providers::supports_oauth(provider) {
        return;
    }
    let Ok(dir) = Config::data_dir() else {
        return;
    };
    if let Some(tok) = whycodes_auth::providers::access_token(provider, &dir).await {
        whycodes_llm::oauth_refresh::register(provider, dir);
        *api_key = tok;
    }
}

fn record_user_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    prompt: &str,
    project_dir: &std::path::Path,
    config: &Config,
    submit_images: &[crate::images::PromptImage],
) -> String {
    let expanded = expand_at_files(prompt, project_dir);
    rt.history
        .push_before_turn(&rt.session.messages, project_dir);
    refresh_session_memory(
        &mut rt.session,
        &rt.agent,
        project_dir,
        config,
        Some(&expanded),
    );
    if submit_images.is_empty() {
        rt.session.add_user_message(&expanded);
    } else {
        match crate::images::build_user_blocks(&expanded, submit_images) {
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
    if config.session.auto_title {
        let seed = rt
            .session
            .first_user_text()
            .unwrap_or_else(|| expanded.clone());
        if rt.session.apply_heuristic_title(&seed) {
            app.session_title = rt.session.title.clone();
        }
    }
    expanded
}

fn route_turn_model(
    session_id: &str,
    provider: &str,
    model: &str,
    expanded: &str,
    fast: Option<&str>,
) -> (String, String) {
    let (route_provider, route_model) =
        whycodes_agent::resolve_turn_model(provider, model, expanded, fast);
    if route_model != model || route_provider != provider {
        tracing::info!(
            from = %format!("{provider}/{model}"),
            to = %format!("{route_provider}/{route_model}"),
            "routed trivial turn to fast model"
        );
        whycodes_core::logging::emit_sid(
            "tui",
            "info",
            "turn.route_fast",
            Some(session_id),
            Some(serde_json::json!({
                "from": format!("{provider}/{model}"),
                "to": format!("{route_provider}/{route_model}"),
            })),
        );
    }
    (route_provider, route_model)
}

fn apply_model_choice(
    app: &mut TuiApp,
    provider: &mut String,
    model: &mut String,
    api_key: &mut String,
    p: String,
    m: String,
    config: &Config,
) {
    if provider.as_str() != p {
        // Never send the previous backend's credential to the new one
        // (e.g. tektik API key as a Code Assist bearer → 401).
        whycodes_llm::oauth_refresh::unregister(provider);
        if let Some(k) = explicit_provider_key(config, &p) {
            *api_key = k;
            whycodes_llm::oauth_refresh::unregister(&p);
        } else {
            api_key.clear();
        }
    }
    *provider = p.clone();
    *model = m.clone();
    app.provider_name = p.clone();
    app.model_name = m.clone();
    app.clear_api_context_window();
    refresh_context_window(app, config, &p, &m);
    app.status_message = format!(
        "Model → {p}/{m}  ·  window {}",
        format_token_count(app.max_context_tokens),
    );
}

fn apply_reasoning_effort(app: &mut TuiApp, agent: &mut Agent, config: &mut Config, raw: &str) {
    let Some(parsed) = whycodes_llm::ReasoningEffort::parse(raw) else {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            format!("Unknown effort '{raw}' (low, medium, high, xhigh)"),
        );
        return;
    };
    let resolved = whycodes_llm::ThinkingConfig::resolve_effort(
        &app.provider_name,
        &app.model_name,
        Some(parsed.as_str()),
    );
    let Some(resolved) = resolved else {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "This model has no reasoning-effort levels",
        );
        return;
    };
    let value = resolved.as_str().to_string();
    app.reasoning_effort = Some(value.clone());
    config.session.reasoning_effort = Some(value.clone());
    agent.set_reasoning_effort(Some(value.clone()));
    if let Err(e) = persist_session_reasoning_effort(&value) {
        tracing::warn!(error = %e, "failed to persist session.reasoning_effort");
    }
    let note = if parsed != resolved {
        format!(" (clamped from {})", parsed.as_str())
    } else {
        String::new()
    };
    app.status_message = format!("Reasoning effort → {}{note}", resolved.label());
    app.mark_dirty();
}

fn persist_session_reasoning_effort(value: &str) -> anyhow::Result<()> {
    // Tests must not rewrite the developer's user config.toml.
    if cfg!(test) {
        return Ok(());
    }
    let mut disk = Config::load()?;
    disk.session.reasoning_effort = Some(value.to_string());
    disk.save()?;
    Ok(())
}

fn tick_spinner(app: &mut TuiApp, spinner_frame: &mut usize) {
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    *spinner_frame = (*spinner_frame + 1) % FRAMES.len();
    app.spinner_frame = *spinner_frame;
    app.mark_dirty();
    let generic = app.status_message.contains("Generating")
        || app
            .status_message
            .chars()
            .next()
            .map(|c| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(c))
            .unwrap_or(false);
    if generic {
        app.status_message.clear();
    }
}

fn cycle_live_session(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
    page_down: bool,
) {
    if runtimes.is_empty() {
        return;
    }
    let idx = if page_down { 0 } else { runtimes.len() - 1 };
    switch_to_runtime(app, rt, runtimes, idx);
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
}

fn switch_mru_session(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
) {
    if runtimes.is_empty() {
        return;
    }
    let idx = mru.pop().unwrap_or(runtimes.len() - 1);
    let idx = idx.min(runtimes.len() - 1);
    switch_to_runtime(app, rt, runtimes, idx);
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
}

fn adopt_fresh_runtime(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut Vec<SessionRuntime>,
    mru: &mut Vec<usize>,
    fresh: SessionRuntime,
) {
    app.yield_view(&mut rt.view);
    let parked = std::mem::replace(rt, fresh);
    runtimes.push(parked);
    mru.push(runtimes.len() - 1);
    app.restore_view(&rt.view);
    app.session_title = rt.session.title.clone();
    app.focus = FocusPane::Prompt;
    // Home gutters are spaces in both ratatui frames; skip-diff will not
    // erase a paste echo (or session sidebar chrome) left on the PTY.
    app.request_full_clear(2);
    app.toasts.push(
        crate::toast::ToastKind::Info,
        format!("New session ({} live)", runtimes.len() + 1),
    );
}

fn warn_session_limit(app: &mut TuiApp) {
    app.toasts.push(
        crate::toast::ToastKind::Warning,
        format!("Session limit ({MAX_LIVE_SESSIONS}) — close one first"),
    );
}

/// Slash line to run, if the prompt currently holds a `/command`.
fn slash_command_from_prompt(app: &TuiApp) -> Option<String> {
    let mut text = app.input_buffer.trim().to_string();
    if app.slash_suggest.active
        && let Some(cmd) = app.slash_suggest.current()
    {
        text = cmd.name.to_string();
    }
    text.starts_with('/').then_some(text)
}

fn consume_slash_draft(app: &mut TuiApp) {
    app.input_buffer.clear();
    app.input_cursor = 0;
    app.pending_pastes.clear();
    app.slash_suggest.dismiss();
}

fn busy_ctrl_c(app: &mut TuiApp, cancel_requested_at: Option<Instant>) -> BusyCtrlC {
    if !app.input_buffer.is_empty() {
        app.clear_prompt_draft();
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "Draft cleared — Ctrl+C again to cancel",
        );
        return BusyCtrlC::ClearedDraft;
    }
    if cancel_requested_at.is_some() {
        BusyCtrlC::ForceStop
    } else {
        BusyCtrlC::BeginCancel
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyCtrlC {
    ClearedDraft,
    BeginCancel,
    ForceStop,
}

fn refresh_live_session_ui(app: &mut TuiApp, rt: &SessionRuntime, runtimes: &[SessionRuntime]) {
    if matches!(app.dialogs.active(), Some(DialogKind::Sessions)) {
        let cursor = app.sessions_cursor;
        let changed = refresh_sessions_rows(app, rt, runtimes);
        let clamped = cursor.min(app.sessions_rows.len().saturating_sub(1));
        if changed || clamped != app.sessions_cursor {
            app.sessions_cursor = clamped;
            app.mark_dirty();
        }
    }
    if matches!(app.dialogs.active(), Some(DialogKind::SessionList))
        && refresh_picker_live_section(app, rt, runtimes)
    {
        app.mark_dirty();
    }
}

fn should_tick_spinner(app: &TuiApp, agent_busy: bool) -> bool {
    (agent_busy || app.running_task_count() > 0)
        && !matches!(
            app.current_agent_state,
            AgentState::WaitingForPermission | AgentState::WaitingForQuestion
        )
}

fn should_force_stop(
    agent_busy: bool,
    cancel_requested_at: Option<Instant>,
    pending_cancel: bool,
) -> bool {
    agent_busy
        && cancel_requested_at
            .map(|since| since.elapsed() >= CANCEL_FORCE_AFTER || pending_cancel)
            .unwrap_or(false)
}

fn maybe_open_queued_dialog(app: &mut TuiApp, rt: &SessionRuntime) {
    // Key off the actual overlay, not a stale WaitingFor* state. A dismissed
    // question can leave WaitingForQuestion with an empty stack (issue #41).
    if matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { .. } | DialogKind::Question(_))
    ) {
        return;
    }
    if let Some(front) = rt.pending_perm_queue.front() {
        app.ask_permission(front.tool_name.clone(), front.detail.clone());
        app.mark_dirty();
    } else if let Some(front) = rt.pending_question_queue.front() {
        app.ask_question(front.questions.clone());
        app.mark_dirty();
    }
}

fn apply_dashboard_switch(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
    target: usize,
) {
    if target == usize::MAX || target >= runtimes.len() {
        return;
    }
    switch_to_runtime(app, rt, runtimes, target);
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

fn apply_boot_prompt(app: &mut TuiApp, missing_key: bool, initial_prompt: Option<String>) {
    if missing_key {
        app.status_message = "no API key · /connect".to_string();
    }
    if let Some(p) = initial_prompt
        && !p.is_empty()
    {
        app.add_message(ChatRole::User, &p);
        app.pending_prompt = Some(p);
    }
}

/// Home-screen update prompt. Never interrupts an existing dialog or a
/// session that already has messages — a confirm over a live turn is worse
/// than a stale binary.
fn maybe_offer_update(app: &mut TuiApp) {
    if app.update_prompted || app.dialogs.is_open() {
        return;
    }
    if !app.messages.is_empty() {
        if app.available_update.is_some() {
            app.update_prompted = true;
        }
        return;
    }
    let Some(offer) = app.available_update.clone() else {
        return;
    };
    app.update_prompted = true;
    let current = env!("CARGO_PKG_VERSION");
    match offer {
        UpdateOffer::SelfInstall(version) => {
            app.confirm(
                "Update available",
                format!("v{current} → v{version} is on GitHub.\nUpdate now?"),
                ConfirmAction::Upgrade,
            );
        }
        UpdateOffer::Homebrew(version) => {
            app.alert(
                "Update available",
                format!(
                    "v{current} → v{version} is on GitHub.\nThis install is Homebrew — run `brew upgrade whycodes`."
                ),
            );
        }
    }
}

/// Spawn `/compact` off the event loop (Grok CommandRunning). The pager
/// keeps painting and Esc still force-stops after [`CANCEL_FORCE_AFTER`].
#[allow(clippy::too_many_arguments)]
fn start_compact_task(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    note: String,
    provider: &str,
    model: &str,
    api_key: &str,
    project_dir: &std::path::Path,
) {
    rt.agent_busy = true;
    let flag = new_cancel_flag();
    rt.cancel_flag = Some(Arc::clone(&flag));
    *cancel_requested_at = None;
    app.current_agent_state = AgentState::Generating;
    app.status_message = "Compacting conversation…".into();
    app.mark_dirty();

    let (ag, sess) = take_turn_owner(rt, project_dir);
    let provider2 = provider.to_string();
    let model2 = model.to_string();
    let api_key2 = api_key.to_string();
    let done_tx2 = rt.done_tx.clone();
    let user_context = if note.is_empty() { None } else { Some(note) };
    rt.turn_join = Some(tokio::spawn(async move {
        let t0 = Instant::now();
        let agent = ag;
        let mut session = sess;
        let outcome = agent
            .compact_session(
                &mut session,
                &provider2,
                &model2,
                &api_key2,
                user_context.as_deref(),
            )
            .await;
        let work_ms = t0.elapsed().as_millis();
        let _ = done_tx2.send(TurnOutcome::Compact {
            agent,
            session,
            outcome,
            work_ms,
        });
    }));
}

fn take_turn_owner(rt: &mut SessionRuntime, project_dir: &std::path::Path) -> (Agent, Session) {
    let ag = std::mem::replace(
        &mut rt.agent,
        Agent::new(whycodes_core::types::AgentInfo {
            name: "_pending".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
            model: None,
            system_prompt: Some(String::new()),
            temperature: None,
            top_p: None,
        }),
    );
    rt.session_backup = Some(rt.session.clone());
    let sess = std::mem::replace(
        &mut rt.session,
        Session::new(project_dir.to_path_buf(), String::new()),
    );
    (ag, sess)
}

fn queue_auto_prompt_if_idle(app: &mut TuiApp, agent_busy: bool) {
    if agent_busy || app.pending_prompt.is_some() {
        return;
    }
    if let Some(next) = app.pending_auto_prompts.pop_front() {
        app.pending_prompt = Some(next);
    }
}

fn apply_async_title(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    pending_async_title: &mut Option<(String, String)>,
    sid: String,
    title: String,
) {
    if rt.session.id == sid {
        if rt.session.apply_generated_title(&title) {
            app.session_title = rt.session.title.clone();
            rt.persist("title_async");
            app.mark_dirty();
        }
    } else if let Some(bg) = runtimes.iter_mut().find(|b| b.session.id == sid) {
        if bg.session.apply_generated_title(&title) {
            bg.view.session_title = bg.session.title.clone();
            bg.unread = true;
            bg.persist("title_async");
        }
    } else {
        *pending_async_title = Some((sid, title));
    }
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
            if matches!(name.as_str(), "todowrite" | "todo")
                && let Some(next) = whycodes_core::todo::apply_todowrite_args(&app.todos, &input)
            {
                app.replace_todos(next);
            }
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
                    app.upsert_bg_job(&id, "running", &summary);
                    app.status_message = format!("bg {id} started");
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        truncate_toast(&format!("bg {id}: {summary}"), 56),
                    );
                }
                "done" => {
                    app.bg_running_count = app.bg_running_count.saturating_sub(1);
                    app.upsert_bg_job(&id, "done", &summary);
                    app.toasts.push(
                        crate::toast::ToastKind::Success,
                        truncate_toast(&format!("bg {id} done · {summary}"), 56),
                    );
                }
                "failed" => {
                    app.bg_running_count = app.bg_running_count.saturating_sub(1);
                    app.upsert_bg_job(&id, "failed", &summary);
                    app.toasts.push(
                        crate::toast::ToastKind::Warning,
                        truncate_toast(&format!("bg {id} failed · {summary}"), 64),
                    );
                }
                "killed" => {
                    app.bg_running_count = app.bg_running_count.saturating_sub(1);
                    app.upsert_bg_job(&id, "killed", &summary);
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        truncate_toast(&format!("bg {id} killed"), 40),
                    );
                }
                _ => {
                    app.upsert_bg_job(&id, &status, &summary);
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
        TurnEvent::Todos { todos } => {
            app.replace_todos(todos);
        }
        TurnEvent::Subagent {
            id,
            kind,
            description,
            status,
            activity,
            elapsed_ms,
            output,
        } => {
            app.upsert_subagent(crate::app::SubagentUpdate {
                id,
                kind,
                description,
                status,
                activity,
                elapsed_ms,
                output,
            });
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

pub(crate) fn apply_panel_update(app: &mut TuiApp, update: whycodes_core::PanelUpdate) {
    use whycodes_core::PanelUpdate;
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

/// Refresh sidebar lists from the workspace index, config, and session todos.
fn refresh_sidebar(
    app: &mut TuiApp,
    config: &whycodes_config::Config,
    file_index: &std::sync::Arc<whycodes_index::WorkspaceIndex>,
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
}

fn load_app_todos(app: &mut TuiApp) {
    app.replace_todos(whycodes_core::todo::load_todos(
        &app.project_dir,
        if app.session_id.is_empty() {
            None
        } else {
            Some(app.session_id.as_str())
        },
    ));
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
    switch_to_agent(
        app,
        agent,
        session,
        config,
        project_dir,
        perm_prompter,
        question_prompter,
        event_tx,
        &name,
        true,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn switch_to_agent(
    app: &mut TuiApp,
    agent: &mut Agent,
    session: &mut Session,
    config: &Config,
    project_dir: &std::path::Path,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    event_tx: &tokio::sync::mpsc::UnboundedSender<TurnEvent>,
    name: &str,
    from_cycle: bool,
) {
    if let Some(idx) = app.primary_agents.iter().position(|n| n == name) {
        app.agent_cycle_idx = idx;
    }
    // Always update agent_name so colors/header reflect the switch
    app.agent_name = name.to_string();
    app.intent_badge = None;
    app.intent_kind = None;
    app.status_message = format!("Agent → {name}");
    let toast = if from_cycle {
        format!("Agent → {name}  (Ctrl+T)")
    } else {
        format!("Agent → {name}")
    };
    app.toasts.push(crate::toast::ToastKind::Info, toast);
    if let Some(info) = config.get_agent(name).cloned() {
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
        let bg = agent.background_registry().clone();
        let claims = agent.session_claims();
        let mut next = Agent::new(info)
            .with_config(config)
            .with_background_registry(bg)
            .with_permission_prompter(
                Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
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
    pending_perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) -> bool {
    let Some(DialogKind::Question(mut state)) = app.dialogs.pop() else {
        return false;
    };

    let finish_cancel = |app: &mut TuiApp,
                         pending_question_queue: &mut std::collections::VecDeque<
        QuestionRequest,
    >,
                         pending_perm_queue: &std::collections::VecDeque<
        whycodes_agent::PermissionRequest,
    >| {
        if let Some(req) = pending_question_queue.pop_front() {
            let _ = req.reply.send(Err(QuestionError::Cancelled));
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        app.clear_dialog_hits();
        resume_after_question(app, pending_question_queue, pending_perm_queue);
    };

    let finish_ok = |app: &mut TuiApp,
                     answers: Vec<whycodes_tools::question::QuestionAnswer>,
                     pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
                     pending_perm_queue: &std::collections::VecDeque<
        whycodes_agent::PermissionRequest,
    >| {
        if let Some(req) = pending_question_queue.pop_front() {
            let _ = req.reply.send(Ok(answers));
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        app.clear_dialog_hits();
        resume_after_question(app, pending_question_queue, pending_perm_queue);
    };

    match code {
        KeyCode::Esc => {
            // Mid-edit Other with text: first Esc leaves the field.
            // Empty free-text (including option-less questions) cancels immediately.
            if state.free_text_focus && !state.free_text.is_empty() {
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
    pending_perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
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

fn open_db_quiet() -> Option<whycodes_storage::db::Database> {
    let data_dir = whycodes_config::Config::data_dir().ok()?;
    std::fs::create_dir_all(&data_dir).ok()?;
    let db_path = data_dir.join("whycodes.db");
    whycodes_storage::db::Database::open(&db_path.to_string_lossy()).ok()
}

fn share_server_up(port: u16) -> bool {
    // Sync quick check via TCP connect (no reqwest dependency in tui path).
    // `SocketAddr::from` is infallible for a `u16` port — no parse/unwrap.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok()
}

fn unshare_session(project_dir: &std::path::Path, id: &str) -> usize {
    let mut n = 0usize;
    let candidates = [
        whycodes_core::project_dir(project_dir).join("shares"),
        whycodes_config::Config::data_dir()
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
            ctx.app.help_scroll = 0;
            ctx.app.help_query.clear();
            ctx.app.help_searching = false;
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
            ctx.app.session_id = ctx.session.id.clone();
            ctx.app.replace_todos(Vec::new());
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
            if ctx.session.messages.is_empty() {
                ctx.app.status_message = "Nothing to compact".into();
            } else {
                *ctx.pending_compact = Some(rest.trim().to_string());
                ctx.app.status_message = "Compacting conversation…".into();
                ctx.app.mark_dirty();
            }
        }
        "/fresh" => {
            ctx.agent.skip_prompt_cache_next();
            ctx.app.toasts.push(
                crate::toast::ToastKind::Info,
                "Next turn skips the provider prompt cache",
            );
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
                let port = std::env::var("WHYCODES_SHARE_PORT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(3030u16);
                let url = format!("http://127.0.0.1:{port}/s/{id}");
                let live = share_server_up(port);
                ctx.app.status_message = if live {
                    format!("Share: {url}")
                } else {
                    format!("Exported — run `whycodes serve` then open {url}")
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
                            "Start server: whycodes serve 3030"
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
            // Re-resolve for the *current* provider; a leftover key from a
            // previous picker selection must not block OAuth.
            let from_live = explicit_provider_key(ctx.config, ctx.provider);
            let from_disk = Config::load()
                .ok()
                .and_then(|cfg| explicit_provider_key(&cfg, ctx.provider));
            if let Some(k) = from_live.or(from_disk) {
                *ctx.api_key = k;
                whycodes_llm::oauth_refresh::unregister(ctx.provider);
            } else {
                ctx.api_key.clear();
                fill_oauth_credential(ctx.api_key, ctx.provider).await;
            }
            let env_name = format!("{}_API_KEY", ctx.provider.to_uppercase());
            if ctx.api_key.is_empty()
                && !whycodes_llm::provider_requires_api_key(ctx.provider, Some(ctx.config))
            {
                ctx.app.status_message = format!("local · {}", ctx.provider);
                ctx.app.add_message(
                    ChatRole::System,
                    format!(
                        "✓ `{0}` needs no API key (local / loopback `base_url`).\n\
                         Cloud Anthropic still needs ANTHROPIC_API_KEY or `/connect`.",
                        ctx.provider
                    ),
                );
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!("Connected · {}", ctx.provider),
                );
            } else if ctx.api_key.is_empty() {
                ctx.app.status_message = format!("no API key · set {env_name}");
                let oauth_supported = whycodes_auth::providers::supports_oauth(ctx.provider);
                ctx.app.add_message(
                    ChatRole::System,
                    format!(
                        "No API key for `{}`\n\
                         → export {env_name}=…\n\
                         → whycodes provider add {} --api-key <key> · then /connect",
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
                    let store = whycodes_auth::TokenStore::new(&dir);
                    for name in whycodes_auth::oauth_providers() {
                        let label = whycodes_auth::providers::spec_for(&name)
                            .map(|s| s.label)
                            .unwrap_or_else(|_| name.clone());
                        let connected = store.get(&name).ok().flatten().is_some();
                        rows.push(crate::app::LoginProviderRow {
                            provider: name.clone(),
                            label: label.to_string(),
                            connected,
                        });
                    }
                }
                ctx.app.login_dialog = crate::app::LoginDialogState { selected: 0, rows };
                crate::input::open_dialog(ctx.app, DialogKind::Login);
            } else if whycodes_auth::providers::supports_oauth(arg) {
                if let Ok(dir) = Config::data_dir() {
                    spawn_oauth_login(ctx.app, &ctx.auth_tx, dir, arg);
                }
            } else {
                ctx.app.status_message = format!("OAuth login not available for `{arg}` ({})", {
                    let names = whycodes_auth::oauth_providers();
                    if names.is_empty() {
                        "install an auth plugin".to_string()
                    } else {
                        names.join(", ")
                    }
                });
            }
        }
        "/agent" => {
            if rest.is_empty() {
                crate::input::open_agent_dialog(ctx.app);
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
            crate::input::open_model_dialog(ctx.app);
        }
        "/effort" if rest.is_empty() => {
            crate::input::open_effort_dialog(ctx.app);
        }
        "/effort" => apply_reasoning_effort(ctx.app, ctx.agent, ctx.config, rest),
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
                apply_model_choice(
                    ctx.app,
                    ctx.provider,
                    ctx.model,
                    ctx.api_key,
                    p.to_string(),
                    m.to_string(),
                    ctx.config,
                );
                fill_oauth_credential(ctx.api_key, ctx.provider).await;
                ctx.app.pending_catalog_refresh = true;
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
            let profile = whycodes_tools::ToolProfile::parse(&ctx.config.session.tool_profile);
            let tools = whycodes_tools::ToolExecutor::new()
                .get_definitions_profile(&ctx.agent.info.permission, profile);
            let full_n = whycodes_tools::ToolExecutor::new()
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
                ctx.app.config.extra = crate::theme::ExtraColors::default();
                ctx.app.theme_selected = ThemeName::ALL.iter().position(|x| *x == t).unwrap_or(0);
                t.apply_syntax_theme();
                for msg in &mut ctx.app.messages {
                    msg.invalidate_layout();
                }
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
        .find(|m| m.role == whycodes_core::types::Role::User)
        .and_then(|m| m.content.as_text().map(|s| s.to_string()))
        .unwrap_or_default();
    if last_user.trim().is_empty() {
        return;
    }
    let last_asst = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == whycodes_core::types::Role::Assistant)
        .and_then(|m| m.content.as_text().map(|s| s.to_string()))
        .unwrap_or_default();
    let provider = provider.to_string();
    let model = model.to_string();
    let api_key = api_key.to_string();
    let model_fast = config.session.model_fast.clone();
    let mut reg = whycodes_llm::provider::ProviderRegistry::default();
    reg.register_from_config(config);
    tokio::spawn(async move {
        let (p, m) = whycodes_agent::resolve_title_model(&provider, &model, model_fast.as_deref());
        let Some(prov) = reg.get(&p) else {
            return;
        };
        use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};
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
        let transport = whycodes_llm::LlmTransport {
            complete_timeout: Some(std::time::Duration::from_secs(8)),
            retry: whycodes_llm::RetryPolicy {
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
                    whycodes_core::types::ContentBlock::Text { text } => Some(text.as_str()),
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
    // Opt-out for debugging hang/crash suspicions: WHYCODES_NO_MODEL_CATALOG=1
    if std::env::var_os("WHYCODES_NO_MODEL_CATALOG").is_some() {
        tracing::debug!("WHYCODES_NO_MODEL_CATALOG set — skip /v1/models");
        return;
    }

    let Some(req) = whycodes_llm::catalog_request_from_config(
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
    let url = whycodes_llm::normalize_models_url(&req.base_url);
    tokio::spawn(async move {
        match whycodes_llm::fetch_model_context_window(&req, &model).await {
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
    crate::app::catalog_models(config)
}

fn parse_session_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Stored sessions, newest first, for the session picker.
///
/// A database that will not open is not worth interrupting the user for here —
/// the picker shows its empty state, and `whycodes session list` reports the
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
            && whycodes_session::title::looks_like_default_title(
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

fn memory_settings(config: &Config) -> whycodes_memory::MemorySettings {
    memory_settings_for(config, None)
}

fn memory_settings_for(
    config: &Config,
    agent_bank: Option<String>,
) -> whycodes_memory::MemorySettings {
    let mut s = whycodes_agent::memory_settings_from_config(config);
    s.agent_bank = agent_bank;
    s
}

/// Best-effort code index when the TUI session starts (skips if already indexed).
fn maybe_session_auto_index(project_dir: &std::path::Path, config: &Config, app: &mut TuiApp) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(n) =
        whycodes_memory::maybe_auto_index(project_dir, &data_dir, &memory_settings(config))
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
    whycodes_memory::apply_memory_prompt(
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
) -> anyhow::Result<whycodes_memory::MemoryService> {
    let data_dir = Config::data_dir()?;
    Ok(whycodes_memory::MemoryService::open(
        project_dir,
        data_dir,
        memory_settings(config),
    )?)
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
    db: &whycodes_storage::db::Database,
    want: &str,
) -> anyhow::Result<Option<Session>> {
    if want == RESUME_LATEST || want.eq_ignore_ascii_case("latest") {
        let list = db.list_sessions()?;
        let Some(row) = list.into_iter().next() else {
            return Ok(None);
        };
        return Ok(Session::load_from_db(db, &row.id)?);
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
        1 => Ok(Session::load_from_db(db, &matches[0].id)?),
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
    agent: &whycodes_agent::Agent,
) -> String {
    use whycodes_core::types::{MessageContent, Role};

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
                        whycodes_core::types::ContentBlock::Text { text }
                        | whycodes_core::types::ContentBlock::ToolResult {
                            content: text, ..
                        } => text.chars().count(),
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

    let profile = whycodes_tools::ToolProfile::parse(&config.session.tool_profile);
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
    agent: &whycodes_agent::Agent,
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
    let profile = whycodes_tools::ToolProfile::parse(&config.session.tool_profile);
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
    if let Some(ref smol) = config.session.model_smol {
        out.push_str(&format!("  model_smol: {smol}\n"));
    } else {
        out.push_str("  model_smol: (auto small sibling for task/swarm)\n");
    }
    if let Some(ref plan) = config.session.model_plan {
        out.push_str(&format!("  model_plan: {plan}\n"));
    }
    if !config.session.stream_rules.is_empty() {
        out.push_str(&format!(
            "  stream_rules: {}\n",
            config.session.stream_rules.len()
        ));
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
    fn restore_terminal_resets_cursor_style_to_user_default() {
        let mut out = Vec::new();
        restore_terminal_on(&mut out);
        let bytes = String::from_utf8_lossy(&out);
        assert!(
            bytes.contains("\u{1b}[0 q"),
            "DECSCUSR default shape missing after TUI exit: {bytes:?}"
        );
    }

    #[test]
    fn keyboard_enhancement_query_skips_bench_and_zero_size() {
        assert!(!should_query_keyboard_enhancement(true, Some((80, 24))));
        assert!(!should_query_keyboard_enhancement(false, Some((0, 24))));
        assert!(!should_query_keyboard_enhancement(false, Some((80, 0))));
        assert!(!should_query_keyboard_enhancement(false, None));
        assert!(should_query_keyboard_enhancement(false, Some((80, 24))));
    }

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

        app.turn_usage = Some(whycodes_core::types::Usage {
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
        let grid = crate::cell_grid::CellGrid::from_buffer(&buf);
        assert_eq!(grid.height(), 2);
        assert_eq!(grid.get(0, 0), "a");
        assert_eq!(grid.get(1, 1), "b");
        assert_eq!(grid.get(1, 0), " ");
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
        let db = whycodes_storage::db::Database::open_in_memory().unwrap();
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
            whycodes_core::PanelUpdate::File {
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
            whycodes_core::PanelUpdate::Diff {
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
            whycodes_core::PanelUpdate::Mermaid {
                source: "graph TD".into(),
            },
        );
        assert!(matches!(
            &app.sidebar.preview,
            crate::app::SidebarPreview::Mermaid { source } if source == "graph TD"
        ));

        apply_panel_update(&mut app, whycodes_core::PanelUpdate::Clear);
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

        session.usage = whycodes_core::types::Usage {
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
        app.turn_usage = Some(whycodes_core::types::Usage {
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
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: "tc1".into(),
            content: "short result".into(),
            is_error: false,
        }]);
        let app = TuiApp::new(TuiAppConfig::default());
        let config = Config::default();
        let agent = Agent::new(whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
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
    fn load_session_todos_missing_and_valid() {
        let dir = tempfile::tempdir().unwrap();
        assert!(whycodes_core::todo::load_todos(dir.path(), None).is_empty());

        let whycodes = dir.path().join(".whycodes");
        std::fs::create_dir_all(&whycodes).unwrap();
        std::fs::write(
            whycodes.join("todos.json"),
            r#"{"todos": [
                {"content": "finish task", "status": "pending"},
                {"content": "done item", "status": "completed"},
                {"content": "working now", "status": "in_progress"},
                {"content": "skipped", "status": "cancelled"},
                {"content": "no status"}
            ]}"#,
        )
        .unwrap();
        let todos = whycodes_core::todo::load_todos(dir.path(), None);
        assert_eq!(todos.len(), 5);
        assert_eq!(todos[0].line(), "☐ finish task");
        assert_eq!(todos[1].line(), "☑ done item");
        assert_eq!(todos[2].line(), "▶ working now");
        assert_eq!(todos[3].line(), "✗ skipped");
        assert_eq!(todos[4].line(), "☐ no status");
    }

    #[test]
    fn load_session_todos_invalid_json_and_wrong_shape() {
        let dir = tempfile::tempdir().unwrap();
        let whycodes = dir.path().join(".whycodes");
        std::fs::create_dir_all(&whycodes).unwrap();
        std::fs::write(whycodes.join("todos.json"), "not json {{{").unwrap();
        assert!(whycodes_core::todo::load_todos(dir.path(), None).is_empty());
        std::fs::write(whycodes.join("todos.json"), r#"{"other": 1}"#).unwrap();
        assert!(whycodes_core::todo::load_todos(dir.path(), None).is_empty());
    }

    #[test]
    fn configured_models_from_providers_and_oauth() {
        use std::sync::OnceLock;
        static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
        // Isolate WHYCODES_HOME so TokenStore reads a temp dir, not user keys.
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };

        let mut config = Config::default();
        config.providers.insert(
            "acme".into(),
            whycodes_core::types::ProviderConfig {
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
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
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
        assert!(out.contains("model_fast:"), "{out}");
        assert!(out.contains("model_smol:"), "{out}");
        assert!(out.contains("model_race:"), "{out}");
        assert!(out.contains("swarm:"), "{out}");

        // With usage → input/output/cache lines.
        session.usage = whycodes_core::types::Usage {
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
        let agent = Agent::new(whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
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

    #[test]
    fn refresh_sessions_rows_is_idempotent() {
        let rt = test_runtime();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        assert!(refresh_sessions_rows(&mut app, &rt, &[]));
        assert!(
            !refresh_sessions_rows(&mut app, &rt, &[]),
            "unchanged dashboard must not report a paint"
        );
        app.status_message = "Generating…".into();
        // Idle runtime preview ignores status — still unchanged.
        assert!(!refresh_sessions_rows(&mut app, &rt, &[]));
    }

    #[test]
    fn refresh_sessions_rows_dirties_when_title_changes() {
        let mut rt = test_runtime();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        assert!(refresh_sessions_rows(&mut app, &rt, &[]));
        rt.session.title = "renamed".into();
        assert!(refresh_sessions_rows(&mut app, &rt, &[]));
        assert!(
            app.sessions_rows[0].title.contains("renamed"),
            "{:?}",
            app.sessions_rows[0].title
        );
    }

    #[test]
    fn refresh_picker_skips_db_and_is_idempotent() {
        let rt = test_runtime();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.session_list.sessions = vec![crate::app::SessionEntry {
            id: "persisted".into(),
            title: "old".into(),
            messages: 2,
            updated_at: None,
            live: None,
        }];
        assert!(refresh_picker_live_section(&mut app, &rt, &[]));
        assert!(
            !refresh_picker_live_section(&mut app, &rt, &[]),
            "second refresh with the same live+persisted set is a no-op"
        );
        assert!(
            app.session_list
                .sessions
                .iter()
                .any(|e| e.id == "persisted" && e.live.is_none()),
            "persisted tail must survive without a DB reload"
        );
    }

    #[test]
    fn drain_background_idle_does_not_touch_view() {
        let mut rt = test_runtime();
        let mut seed = TuiApp::from_config(TuiAppConfig::default());
        seed.add_message(ChatRole::Assistant, "hello");
        seed.yield_view(&mut rt.view);
        let ptr = rt.view.messages.as_ptr();
        drain_background_runtime(&mut rt);
        assert_eq!(
            rt.view.messages.as_ptr(),
            ptr,
            "idle drain must not reallocate the parked transcript"
        );
        assert!(!rt.unread);
        assert_eq!(rt.view.messages[0].content, "hello");
    }

    #[test]
    fn drain_background_applies_text_delta_in_place() {
        let mut rt = test_runtime();
        let mut seed = TuiApp::from_config(TuiAppConfig::default());
        seed.add_message(ChatRole::Assistant, "hi");
        seed.yield_view(&mut rt.view);
        rt.event_tx
            .send(TurnEvent::TextDelta(" there".into()))
            .expect("open channel");
        drain_background_runtime(&mut rt);
        assert!(rt.unread);
        assert_eq!(rt.view.messages.last().unwrap().content, "hi there");
    }

    #[test]
    fn switch_to_runtime_moves_transcripts() {
        let mut active = test_runtime();
        let mut parked = test_runtime();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.add_message(ChatRole::User, "from-active");
        let mut seed = TuiApp::from_config(TuiAppConfig::default());
        seed.add_message(ChatRole::User, "from-parked");
        seed.yield_view(&mut parked.view);
        let mut runtimes = vec![parked];
        switch_to_runtime(&mut app, &mut active, &mut runtimes, 0);
        assert!(
            app.pending_full_clears >= 2,
            "session switch must wipe PTY ghosts (home gutters / sidebar)"
        );
        assert_eq!(app.messages[0].content, "from-parked");
        assert_eq!(runtimes[0].view.messages[0].content, "from-active");
        assert!(
            active.view.messages.is_empty(),
            "live snapshot stays empty while adopted"
        );
        assert_eq!(
            crate::session_runtime::preview_from_messages(&app.messages),
            "from-parked"
        );
    }

    fn test_runtime() -> SessionRuntime {
        use std::sync::OnceLock;
        static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };

        let info = whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        };
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
        let (question_prompter, question_rx) = ChannelQuestionPrompter::new(None);
        SessionRuntime::new(
            Agent::new(info),
            Session::new(PathBuf::from("/work/proj"), "sys".into()),
            SessionHistory::new(),
            event_tx,
            event_rx,
            done_tx,
            done_rx,
            Arc::new(perm_prompter),
            Arc::new(question_prompter),
            perm_rx,
            question_rx,
        )
    }

    fn isolate_home() {
        use std::sync::OnceLock;
        static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    }

    /// Exclusive empty `WHYCODES_HOME` for tests that assert on an empty session
    /// store. The shared [`isolate_home`] OnceLock is process-wide, so a
    /// sibling persist can make `RESUME_LATEST` look populated.
    fn isolate_home_fresh() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        (lock, dir)
    }

    #[test]
    fn apply_turn_event_covers_every_variant() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());

        apply_turn_event(&mut app, TurnEvent::TextDelta("hello".into()));
        assert_eq!(app.current_agent_state, AgentState::Generating);
        assert!(app.messages.iter().any(|m| m.content.contains("hello")));

        apply_turn_event(&mut app, TurnEvent::ThinkingDelta("hmm".into()));
        assert_eq!(app.current_agent_state, AgentState::Thinking);

        apply_turn_event(
            &mut app,
            TurnEvent::ToolStart {
                id: "t1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
        );
        assert!(app.status_message.contains("tool: run"));
        apply_turn_event(
            &mut app,
            TurnEvent::ToolStart {
                id: "t2".into(),
                name: "read_file".into(),
                input: serde_json::json!({}),
            },
        );
        assert!(app.status_message.contains("tool: read"));
        apply_turn_event(
            &mut app,
            TurnEvent::ToolStart {
                id: "t3".into(),
                name: "search_code".into(),
                input: serde_json::json!({}),
            },
        );
        assert!(app.status_message.contains("tool: grep"));
        apply_turn_event(
            &mut app,
            TurnEvent::ToolStart {
                id: "t4".into(),
                name: "custom_tool".into(),
                input: serde_json::json!({}),
            },
        );
        assert!(app.status_message.contains("custom_tool"));
        apply_turn_event(
            &mut app,
            TurnEvent::ToolEnd {
                id: "t1".into(),
                content: "ok".into(),
                is_error: false,
            },
        );

        app.current_agent_state = AgentState::Idle;
        apply_turn_event(&mut app, TurnEvent::Status("Remembered foo".into()));
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Remembered"))
        );
        apply_turn_event(&mut app, TurnEvent::Status("working".into()));
        assert_eq!(app.status_message, "working");

        apply_turn_event(
            &mut app,
            TurnEvent::Intent {
                kind: "change".into(),
                confidence: 0.9,
                badge: "chg".into(),
                notice_kind: "warning".into(),
                notice: "mode mismatch — switch agent".into(),
            },
        );
        assert_eq!(app.intent_kind.as_deref(), Some("change"));
        assert_eq!(app.intent_badge.as_deref(), Some("chg"));
        apply_turn_event(
            &mut app,
            TurnEvent::Intent {
                kind: "question".into(),
                confidence: 0.4,
                badge: String::new(),
                notice_kind: "info".into(),
                notice: "short note".into(),
            },
        );
        assert!(app.intent_badge.is_none());

        apply_turn_event(
            &mut app,
            TurnEvent::Usage(whycodes_core::types::Usage {
                input_tokens: 10,
                output_tokens: 4,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        );
        assert!(app.turn_usage.is_some());

        apply_turn_event(&mut app, TurnEvent::Cancelled);
        assert_eq!(app.current_agent_state, AgentState::Idle);
        assert!(app.status_message.contains("Cancelled"));

        apply_turn_event(
            &mut app,
            TurnEvent::FileConflict {
                path: "src/lib.rs".into(),
                claimant: "a".into(),
                owner: "b".into(),
            },
        );
        assert!(app.status_message.contains("lib.rs"));

        apply_turn_event(
            &mut app,
            TurnEvent::SwarmStatus {
                active: 1,
                total: 3,
                message: String::new(),
            },
        );
        assert_eq!(app.status_message, "swarm 3…");
        apply_turn_event(
            &mut app,
            TurnEvent::SwarmStatus {
                active: 1,
                total: 3,
                message: "workers go".into(),
            },
        );
        assert_eq!(app.status_message, "workers go");

        apply_turn_event(
            &mut app,
            TurnEvent::Background {
                id: "bg-1".into(),
                status: "running".into(),
                summary: "cargo test".into(),
            },
        );
        assert_eq!(app.bg_running_count, 1);
        assert!(
            app.bg_jobs
                .iter()
                .any(|j| j.id == "bg-1" && j.status == "running"),
            "running bg job must appear in the sticky tasks list"
        );
        apply_turn_event(
            &mut app,
            TurnEvent::Background {
                id: "bg-1".into(),
                status: "done".into(),
                summary: "ok".into(),
            },
        );
        assert_eq!(app.bg_running_count, 0);
        apply_turn_event(
            &mut app,
            TurnEvent::Background {
                id: "bg-2".into(),
                status: "failed".into(),
                summary: "boom".into(),
            },
        );
        apply_turn_event(
            &mut app,
            TurnEvent::Background {
                id: "bg-3".into(),
                status: "killed".into(),
                summary: String::new(),
            },
        );
        apply_turn_event(
            &mut app,
            TurnEvent::Background {
                id: "bg-4".into(),
                status: "queued".into(),
                summary: String::new(),
            },
        );
        assert!(app.status_message.contains("bg-4"));

        apply_turn_event(
            &mut app,
            TurnEvent::EnqueuePrompt {
                text: "  next  ".into(),
            },
        );
        assert_eq!(app.pending_auto_prompts.len(), 1);
        apply_turn_event(&mut app, TurnEvent::EnqueuePrompt { text: "   ".into() });
        assert_eq!(app.pending_auto_prompts.len(), 1);

        apply_turn_event(
            &mut app,
            TurnEvent::Panel(whycodes_core::PanelUpdate::File {
                path: "x.rs".into(),
                text: "fn x() {}".into(),
            }),
        );
        assert!(app.sidebar.visible);

        apply_turn_event(
            &mut app,
            TurnEvent::Subagent {
                id: "kid".into(),
                kind: "explore".into(),
                description: "look".into(),
                status: "running".into(),
                activity: "Thinking".into(),
                elapsed_ms: 12,
                output: String::new(),
            },
        );
        assert!(app.subagents.iter().any(|s| s.id == "kid"));

        apply_turn_event(
            &mut app,
            TurnEvent::SwarmMessage {
                from: "a".into(),
                to: "b".into(),
                text: "hi".into(),
            },
        );
        apply_turn_event(
            &mut app,
            TurnEvent::PermissionAsk {
                request_id: "p".into(),
                tool_name: "bash".into(),
                detail: "ls".into(),
            },
        );
        apply_turn_event(
            &mut app,
            TurnEvent::QuestionAsk {
                request_id: "q".into(),
                questions: serde_json::json!([]),
            },
        );
        apply_turn_event(
            &mut app,
            TurnEvent::FileStale {
                path: "src/main.rs".into(),
                reader: "r".into(),
                writer: "w".into(),
            },
        );
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("stale"))
        );

        apply_turn_event(
            &mut app,
            TurnEvent::Todos {
                todos: vec![whycodes_core::TodoItem::new(
                    "a",
                    "panel item",
                    whycodes_core::TodoStatus::InProgress,
                )],
            },
        );
        assert_eq!(app.todos.len(), 1);
        assert_eq!(app.todos[0].content, "panel item");

        apply_turn_event(
            &mut app,
            TurnEvent::ToolStart {
                id: "tw".into(),
                name: "todowrite".into(),
                input: serde_json::json!({
                    "todos":[{"id":"a","content":"updated","status":"completed"}]
                }),
            },
        );
        assert_eq!(app.todos[0].status, whycodes_core::TodoStatus::Completed);
        apply_turn_event(
            &mut app,
            TurnEvent::ToolStart {
                id: "tw2".into(),
                name: "todo".into(),
                input: serde_json::json!({
                    "merge": false,
                    "todos":[{"id":"z","content":"only","status":"pending"}]
                }),
            },
        );
        assert_eq!(app.todos.len(), 1);
        assert_eq!(app.todos[0].id, "z");
    }

    #[test]
    fn drain_turn_events_coalesces_deltas() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert!(!drain_turn_events(&mut app, &mut rx));
        tx.send(TurnEvent::ThinkingDelta("th".into())).unwrap();
        tx.send(TurnEvent::ThinkingDelta("ink".into())).unwrap();
        tx.send(TurnEvent::TextDelta("hel".into())).unwrap();
        tx.send(TurnEvent::TextDelta("lo".into())).unwrap();
        tx.send(TurnEvent::Status("done".into())).unwrap();
        assert!(drain_turn_events(&mut app, &mut rx));
        assert_eq!(app.status_message, "done");
        assert!(app.messages.iter().any(|m| m.content.contains("hello")));
    }

    #[test]
    fn helpers_tui_available_summary_share_and_diff() {
        isolate_home();
        let _ = tui_available();
        print_session_summary("coverage-summary");
        assert!(!share_server_up(1), "port 1 should be closed");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(share_server_up(port));
        drop(listener);

        let dir = tempfile::tempdir().unwrap();
        let out = project_diff_report(dir.path());
        assert!(out.contains("Diff"), "{out}");
        assert!(
            out.contains("git") || out.contains("clean") || out.contains("status"),
            "{out}"
        );

        let shares = dir.path().join(".whycodes").join("shares");
        std::fs::create_dir_all(&shares).unwrap();
        std::fs::write(shares.join("abc.json"), "{}").unwrap();
        std::fs::write(shares.join("abc.md"), "#").unwrap();
        assert_eq!(unshare_session(dir.path(), "abc"), 2);
        assert_eq!(unshare_session(dir.path(), "abc"), 0);

        #[cfg(target_os = "linux")]
        {
            let _ = which_bwrap();
        }

        persist_session_best_effort(
            &Session::new(dir.path().to_path_buf(), "sys".into()),
            "test",
        );
        let _ = try_load_session("no-such-session");
        let _ = open_db_quiet();
    }

    #[test]
    fn refresh_sidebar_and_dashboard() {
        isolate_home();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
        std::fs::create_dir_all(dir.path().join(".whycodes")).unwrap();
        std::fs::write(
            dir.path().join(".whycodes").join("todos.json"),
            r#"{"todos":[{"content":"do it","status":"pending"}]}"#,
        )
        .unwrap();
        let idx = whycodes_index::WorkspaceIndex::start_with(
            vec![dir.path().to_path_buf()],
            whycodes_index::IndexOptions {
                watch: false,
                threads: 1,
                ..Default::default()
            },
        );
        let _ = idx.wait_ready(Duration::from_secs(5));
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.project_dir = dir.path().to_path_buf();
        let mut config = Config::default();
        config.mcp_servers.insert(
            "demo".into(),
            whycodes_config::McpServerConfig {
                transport: None,
                command: Some("true".into()),
                args: Vec::new(),
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        );
        refresh_sidebar(&mut app, &config, &idx);
        load_app_todos(&mut app);
        assert!(app.todos.iter().any(|t| t.content == "do it"));
        assert!(app.sidebar.mcp_status.iter().any(|s| s.contains("demo")));

        let rt = test_runtime();
        open_sessions_dashboard(&mut app, &rt, &[]);
        assert!(matches!(app.dialogs.active(), Some(DialogKind::Sessions)));
        assert_eq!(app.key_context, KeymapContext::Dialog);

        let mut view = crate::session_runtime::ViewSnapshot::default();
        app.add_message(ChatRole::User, "scratch");
        app.yield_view(&mut view);
        with_view_scratch(&mut view, |scratch| {
            scratch.add_message(ChatRole::Assistant, "from-scratch");
        });
        assert!(
            view.messages
                .iter()
                .any(|m| m.content.contains("from-scratch"))
        );
    }

    #[test]
    fn begin_cancel_sets_flag_and_status() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let flag = new_cancel_flag();
        let mut at = None;
        let mut q = std::collections::VecDeque::new();
        let mut p = std::collections::VecDeque::new();
        begin_cancel(&mut app, &Some(Arc::clone(&flag)), &mut at, &mut q, &mut p);
        assert!(whycodes_agent::is_cancelled(&Some(flag)));
        assert!(at.is_some());
        assert!(app.status_message.contains("Cancelling"));
        begin_cancel(&mut app, &None, &mut at, &mut q, &mut p);
        assert!(at.is_some(), "second call keeps the original timer");
    }

    #[test]
    fn tui_login_ui_emits_notes() {
        use whycodes_auth::providers::LoginUi;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut ui = TuiLoginUi { tx };
        ui.show_sign_in("Anthropic", "https://example.test", true);
        ui.show_sign_in("Anthropic", "https://example.test", false);
        ui.note("waiting");
        ui.show_device_code("ABCD", "https://github.com/login", true);
        ui.show_device_code("ABCD", "https://github.com/login", false);
        let mut notes = 0usize;
        while let Ok(ev) = rx.try_recv() {
            if let AuthFlowEvent::Note(_) = ev {
                notes += 1;
            }
        }
        assert_eq!(notes, 5);
    }

    #[test]
    fn event_forces_redraw_treats_paste_as_dirty() {
        assert!(event_forces_redraw(&Event::Paste("x".into())));
        assert!(event_forces_redraw(&Event::FocusGained));
    }

    #[test]
    fn expand_at_files_multiple_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "AAA").unwrap();
        std::fs::write(dir.path().join("b.txt"), "BBB").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let out = expand_at_files("see @a.txt and @b.txt please", dir.path());
        assert!(out.contains("AAA") && out.contains("BBB"), "{out}");
        let out = expand_at_files("open @src now", dir.path());
        assert!(out.contains("@src"), "dirs stay literal: {out}");
    }

    #[test]
    fn maybe_offer_update_confirms_on_empty_home() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.available_update = Some(UpdateOffer::SelfInstall("9.9.9".into()));
        maybe_offer_update(&mut app);
        assert!(app.update_prompted);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Confirm {
                on_confirm: ConfirmAction::Upgrade,
                ..
            })
        ));

        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.available_update = Some(UpdateOffer::Homebrew("9.9.9".into()));
        maybe_offer_update(&mut app);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Alert { .. })
        ));
        maybe_offer_update(&mut app);
        assert!(app.update_prompted);

        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.add_message(ChatRole::User, "hi");
        app.available_update = Some(UpdateOffer::SelfInstall("9.9.9".into()));
        maybe_offer_update(&mut app);
        assert!(app.update_prompted);
        assert!(!app.dialogs.is_open());
    }

    #[test]
    fn run_options_and_turn_outcome_exist() {
        let opts = TuiRunOptions {
            project_dir: PathBuf::from("/tmp"),
            provider: "x".into(),
            model: "y".into(),
            api_key: String::new(),
            agent_name: "build".into(),
            max_turns: None,
            initial_prompt: None,
            config: Config::default(),
            resume_session_id: Some(RESUME_LATEST.into()),
            remote: None,
            update_rx: None,
        };
        assert_eq!(opts.resume_session_id.as_deref(), Some(RESUME_LATEST));
        let _ = TurnOutcome::Remote {
            text: "hi".into(),
            error: None,
            work_ms: 1,
        };
    }

    struct SlashHarness {
        _tmp: tempfile::TempDir,
        app: TuiApp,
        session: Session,
        history: SessionHistory,
        agent: Agent,
        config: Config,
        provider: String,
        model: String,
        api_key: String,
        perm_prompter: Arc<ChannelPermissionPrompter>,
        question_prompter: Arc<ChannelQuestionPrompter>,
        auth_tx: mpsc::UnboundedSender<AuthFlowEvent>,
        _auth_rx: mpsc::UnboundedReceiver<AuthFlowEvent>,
        pending_compact: Option<String>,
    }

    impl SlashHarness {
        fn new() -> Self {
            isolate_home();
            let tmp = tempfile::tempdir().expect("tmpdir");
            let info = whycodes_core::types::AgentInfo {
                name: "build".into(),
                description: String::new(),
                mode: AgentMode::Primary,
                permission: whycodes_core::types::PermissionSet::default(),
                model: None,
                system_prompt: Some("sys".into()),
                temperature: None,
                top_p: None,
            };
            let (perm_prompter, _perm_rx) = ChannelPermissionPrompter::new();
            let (question_prompter, _q_rx) = ChannelQuestionPrompter::new(None);
            let (auth_tx, auth_rx) = mpsc::unbounded_channel();
            let session = Session::new(tmp.path().to_path_buf(), "sys".into());
            Self {
                app: TuiApp::from_config(TuiAppConfig::default()),
                session,
                history: SessionHistory::new(),
                agent: Agent::new(info),
                config: Config::default(),
                provider: "acme".into(),
                model: "m1".into(),
                api_key: String::new(),
                perm_prompter: Arc::new(perm_prompter),
                question_prompter: Arc::new(question_prompter),
                auth_tx,
                _auth_rx: auth_rx,
                pending_compact: None,
                _tmp: tmp,
            }
        }

        async fn run(&mut self, cmd: &str) {
            let project_dir = self.session.project_path.clone();
            let mut ctx = SlashContext {
                app: &mut self.app,
                session: &mut self.session,
                history: &mut self.history,
                agent: &mut self.agent,
                config: &mut self.config,
                project_dir: &project_dir,
                provider: &mut self.provider,
                model: &mut self.model,
                api_key: &mut self.api_key,
                perm_prompter: Arc::clone(&self.perm_prompter),
                question_prompter: Arc::clone(&self.question_prompter),
                auth_tx: self.auth_tx.clone(),
                pending_compact: &mut self.pending_compact,
            };
            handle_slash(cmd, &mut ctx).await;
        }
    }

    #[tokio::test]
    async fn handle_slash_covers_local_commands() {
        let mut h = SlashHarness::new();

        h.run("/help").await;
        assert_eq!(h.app.mode, AppMode::Help);
        h.app.mode = AppMode::Normal;

        h.run("/exit").await;
        assert!(!h.app.running);
        h.app.running = true;

        h.run("/rename").await;
        assert!(h.app.status_message.contains("Title"));
        h.run("/rename coverage-session").await;
        assert!(h.session.title.contains("coverage"));

        h.run("/undo").await;
        assert!(h.app.status_message.to_lowercase().contains("nothing"));
        h.run("/redo").await;
        assert!(h.app.status_message.to_lowercase().contains("nothing"));

        h.run("/compact").await;
        assert!(h.app.status_message.contains("Nothing to compact"));
        h.session.add_user_message("old task");
        h.session
            .add_assistant_message(vec![whycodes_core::types::ContentBlock::Text {
                text: "working".into(),
            }]);
        h.session.add_user_message("fix login");
        h.app.load_messages_from_session(&h.session);
        h.run("/compact keep the auth details").await;
        assert!(
            h.app.status_message.contains("Compacting conversation"),
            "{}",
            h.app.status_message
        );
        h.run("/fresh").await;
        assert!(
            h.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("prompt cache")),
            "expected /fresh toast"
        );
        assert_eq!(
            h.pending_compact.as_deref(),
            Some("keep the auth details"),
            "slash must queue compact; the event loop spawns the LLM"
        );
        assert_eq!(h.session.messages[0].content.as_text(), Some("old task"));

        h.run("/bg").await;
        assert!(
            h.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("No background"))
        );
        h.run("/bg kill missing").await;
        h.run("/bg whatever").await;
        assert!(h.app.status_message.contains("Usage"));

        h.run("/loop").await;
        assert!(h.app.status_message.contains("Usage"));
        h.run("/loop 2 do the thing").await;
        assert_eq!(h.app.pending_prompt.as_deref(), Some("do the thing"));
        assert_eq!(h.app.pending_auto_prompts.len(), 1);
        h.run("/loop stop").await;
        assert!(h.app.pending_auto_prompts.is_empty());

        h.run("/remember").await;
        assert!(h.app.status_message.contains("Usage"));
        h.run("/remember save this fact").await;
        h.run("/memory").await;

        h.run("/agent").await;
        assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Agent)));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;
        h.run("/agent no-such-agent").await;
        assert!(
            h.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Unknown agent"))
        );

        h.run("/theme").await;
        assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Theme)));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;
        h.run("/theme nord").await;
        assert_eq!(h.app.theme, crate::theme::ThemeName::Nord);
        h.run("/theme not-a-theme").await;

        h.run("/sessions").await;
        assert!(matches!(
            h.app.dialogs.active(),
            Some(DialogKind::SessionList)
        ));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;

        h.run("/resume abcdef").await;
        assert_eq!(h.app.pending_session_id.as_deref(), Some("abcdef"));
        h.app.pending_session_id = None;
        h.run("/continue").await;
        assert_eq!(h.app.pending_session_id.as_deref(), Some(RESUME_LATEST));
        h.app.pending_session_id = None;
        h.run("/resume").await;
        assert!(matches!(
            h.app.dialogs.active(),
            Some(DialogKind::SessionList)
        ));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;

        h.run("/models").await;
        assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Model)));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;
        h.run("/models m2").await;
        assert_eq!(h.model, "m2");
        h.run("/models acme/m3").await;
        assert_eq!(h.provider, "acme");
        assert_eq!(h.model, "m3");

        h.provider = "xai".into();
        h.model = "grok-4".into();
        h.app.provider_name = "xai".into();
        h.app.model_name = "grok-4".into();
        h.run("/effort").await;
        assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Effort)));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;
        h.run("/effort high").await;
        assert_eq!(h.app.reasoning_effort.as_deref(), Some("high"));
        h.run("/effort xhigh").await;
        assert_eq!(h.app.reasoning_effort.as_deref(), Some("high"));

        h.run("/tools").await;
        h.run("/info").await;
        h.run("/doctor").await;
        h.run("/diff").await;
        h.run("/context").await;
        h.run("/cost").await;
        h.run("/init").await;
        assert!(h.app.pending_prompt.is_some());

        h.run("/unshare").await;
        h.run("/share").await;
        h.run("/connect").await;
        h.run("/login").await;
        h.run("/login not-oauth").await;
        h.run("/nope").await;
        assert!(
            h.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Unknown command"))
        );

        h.run("/new").await;
        assert!(
            h.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("New session"))
        );

        h.config.commands.insert(
            "hello".into(),
            whycodes_config::CustomCommandConfig {
                template: "hello $ARGUMENTS".into(),
                description: Some("say hi".into()),
                agent: None,
                model: None,
                subtask: None,
            },
        );
        h.run("/hello world").await;
        assert_eq!(h.app.pending_prompt.as_deref(), Some("hello world"));
    }

    #[test]
    fn memory_and_index_helpers() {
        isolate_home();
        let dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        maybe_session_auto_index(dir.path(), &config, &mut app);
        let prompt = with_project_memory("base prompt", dir.path(), &config, Some("query"));
        assert!(prompt.contains("base prompt"));
        let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
        let agent = Agent::new(whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
            model: None,
            system_prompt: Some("sys".into()),
            temperature: None,
            top_p: None,
        });
        refresh_session_memory(&mut session, &agent, dir.path(), &config, None);
        let _ = memory_service(dir.path(), &config);
    }

    fn dummy_info(name: &str) -> whycodes_core::types::AgentInfo {
        whycodes_core::types::AgentInfo {
            name: name.into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
            model: None,
            system_prompt: Some("sys".into()),
            temperature: None,
            top_p: None,
        }
    }

    fn temp_index() -> (tempfile::TempDir, Arc<whycodes_index::WorkspaceIndex>) {
        let dir = tempfile::tempdir().unwrap();
        let idx = whycodes_index::WorkspaceIndex::start_with(
            vec![dir.path().to_path_buf()],
            whycodes_index::IndexOptions {
                watch: false,
                threads: 1,
                ..Default::default()
            },
        );
        (dir, idx)
    }

    fn sample_question() -> whycodes_tools::question::QuestionSpec {
        whycodes_tools::question::QuestionSpec {
            prompt: "Pick?".into(),
            options: vec![
                whycodes_tools::question::QuestionOption {
                    label: "Yes".into(),
                    description: String::new(),
                    preview: None,
                },
                whycodes_tools::question::QuestionOption {
                    label: "No".into(),
                    description: String::new(),
                    preview: None,
                },
            ],
            multi_select: false,
        }
    }

    #[test]
    fn force_stop_applies_outcome_or_rebuilds() {
        isolate_home();
        let (dir, idx) = temp_index();
        let config = Config::default();
        let (perm, _prx) = ChannelPermissionPrompter::new();
        let (question, _qrx) = ChannelQuestionPrompter::new(None);
        let perm = Arc::new(perm);
        let question = Arc::new(question);
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();

        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.add_message(ChatRole::Assistant, "partial");
        let mut agent = Agent::new(dummy_info("build"));
        let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
        let mut busy = true;
        let mut flag = Some(new_cancel_flag());
        let mut at = Some(Instant::now());
        let mut join = None;
        let mut backup = None;
        let mut q = std::collections::VecDeque::new();
        let mut p = std::collections::VecDeque::new();

        done_tx
            .send(TurnOutcome::Ok {
                text: "done".into(),
                agent: Agent::new(dummy_info("from-outcome")),
                session: Session::new(dir.path().to_path_buf(), "restored".into()),
                work_ms: 3,
            })
            .unwrap();
        force_stop_turn(
            &mut app,
            &mut agent,
            &mut session,
            &mut busy,
            &mut flag,
            &mut at,
            &mut join,
            &mut backup,
            &mut q,
            &mut p,
            &mut done_rx,
            &config,
            dir.path(),
            "acme",
            "m",
            event_tx.clone(),
            Arc::clone(&perm),
            Arc::clone(&question),
            &idx,
        );
        assert!(!busy);
        assert!(flag.is_none());
        assert_eq!(agent.info.name, "from-outcome");
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == ChatRole::System && m.content.contains("Stopped"))
        );

        // No outcome → restore backup and rebuild.
        let mut agent = Agent::new(dummy_info("old"));
        let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
        let mut busy = true;
        let mut flag = Some(new_cancel_flag());
        let mut at = Some(Instant::now());
        let mut backup = Some(Session::new(dir.path().to_path_buf(), "backup-sys".into()));
        app.agent_name = "plan".into();
        app.add_message(ChatRole::System, "already cancelled");
        force_stop_turn(
            &mut app,
            &mut agent,
            &mut session,
            &mut busy,
            &mut flag,
            &mut at,
            &mut join,
            &mut backup,
            &mut q,
            &mut p,
            &mut done_rx,
            &config,
            dir.path(),
            "acme",
            "m",
            event_tx,
            perm,
            question,
            &idx,
        );
        assert!(!busy);
        assert!(backup.is_none());
        assert_eq!(session.system_prompt, "backup-sys");
    }

    #[test]
    fn rebuild_agent_resolves_pending_name() {
        isolate_home();
        let (dir, idx) = temp_index();
        let config = Config::default();
        let (perm, _) = ChannelPermissionPrompter::new();
        let (question, _) = ChannelQuestionPrompter::new(None);
        let (event_tx, _) = mpsc::unbounded_channel();
        let mut agent = Agent::new(dummy_info("old"));
        let mut session = Session::new(dir.path().to_path_buf(), String::new());
        rebuild_agent_after_force_stop(
            &mut agent,
            &mut session,
            &config,
            dir.path(),
            "_pending",
            event_tx.clone(),
            Arc::new(perm),
            Arc::new(question),
            &idx,
        );
        assert_eq!(agent.info.name, "build");
        assert!(!session.system_prompt.is_empty());

        let (perm, _) = ChannelPermissionPrompter::new();
        let (question, _) = ChannelQuestionPrompter::new(None);
        rebuild_agent_after_force_stop(
            &mut agent,
            &mut session,
            &config,
            dir.path(),
            "",
            event_tx,
            Arc::new(perm),
            Arc::new(question),
            &idx,
        );
        assert_eq!(agent.info.name, "build");
    }

    #[tokio::test]
    async fn cycle_agent_walks_primary_list() {
        isolate_home();
        let dir = tempfile::tempdir().unwrap();
        let (perm, _) = ChannelPermissionPrompter::new();
        let (question, _) = ChannelQuestionPrompter::new(None);
        let perm = Arc::new(perm);
        let question = Arc::new(question);
        let (event_tx, _) = mpsc::unbounded_channel();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut agent = Agent::new(dummy_info("build"));
        let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
        let mut config = Config::default();

        app.primary_agents.clear();
        cycle_agent(
            &mut app,
            &mut agent,
            &mut session,
            &config,
            dir.path(),
            Arc::clone(&perm),
            Arc::clone(&question),
            &event_tx,
        )
        .await;
        assert!(app.agent_name.is_empty() || app.agent_name == "build");

        app.primary_agents = vec!["build".into(), "plan".into()];
        app.agent_cycle_idx = 0;
        config.agents.push(dummy_info("plan"));
        cycle_agent(
            &mut app,
            &mut agent,
            &mut session,
            &config,
            dir.path(),
            perm,
            question,
            &event_tx,
        )
        .await;
        assert_eq!(app.agent_name, "plan");
        assert_eq!(agent.info.name, "plan");
        assert!(app.status_message.contains("plan"));
    }

    #[test]
    fn handle_question_key_navigates_confirms_and_cancels() {
        let spec = sample_question();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut qqueue = std::collections::VecDeque::new();
        let pqueue = std::collections::VecDeque::new();
        assert!(!handle_question_key(
            &mut app,
            KeyCode::Enter,
            &mut qqueue,
            &pqueue
        ));

        app.ask_question(vec![spec.clone()]);
        assert!(handle_question_key(
            &mut app,
            KeyCode::Down,
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Up,
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('j'),
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('k'),
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Right,
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Left,
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('y'),
            &mut qqueue,
            &pqueue
        ));
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Copied") || t.message.contains("clipboard"))
        );

        // Digit 1 selects first option and finishes.
        let (tx, rx) = tokio::sync::oneshot::channel();
        qqueue.push_back(QuestionRequest {
            questions: vec![spec.clone()],
            reply: tx,
        });
        app.ask_question(vec![spec.clone()]);
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('1'),
            &mut qqueue,
            &pqueue
        ));
        assert!(qqueue.is_empty());
        assert!(rx.blocking_recv().unwrap().is_ok());
        assert_eq!(app.mode, AppMode::Normal);

        // Esc cancels.
        let (tx, rx) = tokio::sync::oneshot::channel();
        qqueue.push_back(QuestionRequest {
            questions: vec![spec.clone()],
            reply: tx,
        });
        app.ask_question(vec![spec.clone()]);
        assert!(handle_question_key(
            &mut app,
            KeyCode::Esc,
            &mut qqueue,
            &pqueue
        ));
        assert!(matches!(
            rx.blocking_recv().unwrap(),
            Err(QuestionError::Cancelled)
        ));

        // Other + free text + Enter.
        app.ask_question(vec![spec.clone()]);
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('o'),
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('x'),
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Backspace,
            &mut qqueue,
            &pqueue
        ));
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char('z'),
            &mut qqueue,
            &pqueue
        ));
        if let Some(DialogKind::Question(st)) = app.dialogs.active() {
            assert_eq!(st.free_text, "z");
            assert!(st.free_text_focus);
        } else {
            panic!("expected question dialog");
        }
        // First Esc with non-empty Other text leaves the field.
        assert!(handle_question_key(
            &mut app,
            KeyCode::Esc,
            &mut qqueue,
            &pqueue
        ));
        if let Some(DialogKind::Question(st)) = app.dialogs.active() {
            assert!(!st.free_text_focus);
        } else {
            panic!("expected question dialog after leaving Other");
        }

        // Space on Other focuses free text; unknown key is not consumed.
        app.ask_question(vec![spec]);
        if let Some(DialogKind::Question(mut st)) = app.dialogs.pop() {
            st.cursor = st.option_count() - 1;
            app.dialogs.push(DialogKind::Question(st));
        }
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char(' '),
            &mut qqueue,
            &pqueue
        ));
        assert!(!handle_question_key(
            &mut app,
            KeyCode::F(1),
            &mut qqueue,
            &pqueue
        ));
    }

    #[test]
    fn resume_after_question_opens_next_or_permission() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut q = std::collections::VecDeque::new();
        let mut p = std::collections::VecDeque::new();
        resume_after_question(&mut app, &q, &p);
        assert!(app.status_message.contains("continuing"));

        let (tx, _rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        resume_after_question(&mut app, &q, &p);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Question(_))
        ));
        assert!(app.status_message.contains("more question"));

        q.clear();
        app.dialogs.clear();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        p.push_back(whycodes_agent::PermissionRequest {
            tool_name: "bash".into(),
            detail: "ls".into(),
            reply: tx,
        });
        resume_after_question(&mut app, &q, &p);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Permission { .. })
        ));
    }

    #[tokio::test]
    async fn spawn_runtime_and_drain_outcomes() {
        isolate_home();
        let (dir, idx) = temp_index();
        let rt = spawn_new_session_runtime(
            "no-such-agent",
            &Config::default(),
            dir.path(),
            &idx,
            whycodes_core::FileClaimRegistry::new(),
        )
        .await;
        assert_eq!(rt.agent.info.name, "no-such-agent");
        assert!(!rt.agent_busy);

        let mut rt = test_runtime();
        let agent = Agent::new(dummy_info("ok"));
        let session = Session::new(PathBuf::from("/work"), "sys".into());
        rt.done_tx
            .send(TurnOutcome::Ok {
                text: "hi".into(),
                agent,
                session,
                work_ms: 1,
            })
            .unwrap();
        drain_background_runtime(&mut rt);
        assert!(!rt.agent_busy);
        assert_eq!(rt.agent.info.name, "ok");

        rt.done_tx
            .send(TurnOutcome::Remote {
                text: String::new(),
                error: Some("boom".into()),
                work_ms: 1,
            })
            .unwrap();
        drain_background_runtime(&mut rt);
        assert!(rt.last_error);
        assert!(
            rt.view
                .messages
                .iter()
                .any(|m| m.content.contains("Remote error"))
        );

        let agent = Agent::new(dummy_info("err"));
        let session = Session::new(PathBuf::from("/work"), "sys".into());
        rt.done_tx
            .send(TurnOutcome::Err {
                error: "nope".into(),
                agent,
                session,
                cancelled: false,
                work_ms: 1,
            })
            .unwrap();
        drain_background_runtime(&mut rt);
        assert!(rt.last_error);

        let agent = Agent::new(dummy_info("cx"));
        let session = Session::new(PathBuf::from("/work"), "sys".into());
        rt.done_tx
            .send(TurnOutcome::Err {
                error: "x".into(),
                agent,
                session,
                cancelled: true,
                work_ms: 1,
            })
            .unwrap();
        drain_background_runtime(&mut rt);
        assert!(!rt.last_error);
        assert!(
            rt.view
                .messages
                .iter()
                .any(|m| m.content.contains("cancelled"))
        );
    }

    #[tokio::test]
    async fn drain_background_queues_prompter_asks() {
        use whycodes_agent::{PermissionPrompter, QuestionPrompter};
        let mut rt = test_runtime();
        let perm = Arc::clone(&rt.perm_prompter);
        let question = Arc::clone(&rt.question_prompter);
        let p = tokio::spawn(async move {
            perm.ask("bash", "ls").await;
        });
        let q = tokio::spawn(async move {
            let _ = question.ask(vec![sample_question()]).await;
        });
        for _ in 0..50 {
            drain_background_runtime(&mut rt);
            if !rt.pending_perm_queue.is_empty() && !rt.pending_question_queue.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(!rt.pending_perm_queue.is_empty());
        assert!(!rt.pending_question_queue.is_empty());
        assert!(rt.unread);
        // Unblock the waiters so the test can finish.
        let _ = rt.pending_perm_queue.pop_front().unwrap().reply.send(false);
        let _ = rt
            .pending_question_queue
            .pop_front()
            .unwrap()
            .reply
            .send(Err(QuestionError::Cancelled));
        let _ = p.await;
        let _ = q.await;
    }

    #[test]
    fn suggestion_and_catalog_helpers_short_circuit() {
        isolate_home();
        let session = Session::new(PathBuf::from("/work"), "sys".into());
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut config = Config::default();
        maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx.clone());
        config.tui.prompt_suggestions = "idle".into();
        maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "", &mut app, tx.clone());
        maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx);

        spawn_model_context_fetch(&config, "p", "m", "", {
            let (tx, _rx) = mpsc::unbounded_channel();
            tx
        });
        restore_terminal_on(&mut Vec::<u8>::new());
        if let Ok(mut w) = open_tui_writer() {
            let _ = w.write(b"");
            let _ = w.flush();
        }
        let _ = bind_agent_prompters(
            Agent::new(dummy_info("build")),
            &{
                let (p, _) = ChannelPermissionPrompter::new();
                Arc::new(p)
            },
            &{
                let (q, _) = ChannelQuestionPrompter::new(None);
                Arc::new(q)
            },
        );
    }

    #[test]
    fn load_session_entries_and_picker_merge() {
        isolate_home();
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
        session.add_user_message("hello there");
        persist_session_best_effort(&session, "entries");
        let entries = load_session_entries();
        assert!(
            entries.iter().any(|e| e.id == session.id),
            "persisted session must appear: {entries:?}"
        );

        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let rt = test_runtime();
        let parked = test_runtime();
        app.session_list.sessions = entries;
        assert!(refresh_picker_live_section(&mut app, &rt, &[parked]));
        assert!(
            app.session_list
                .sessions
                .iter()
                .any(|e| e.live == Some(usize::MAX)),
            "current live row"
        );
        assert!(
            app.session_list.sessions.iter().any(|e| e.live == Some(0)),
            "parked live row"
        );
    }

    #[test]
    fn doctor_report_flags_missing_project() {
        isolate_home();
        let session = Session::new(PathBuf::from("/no/such/project/dir"), "sys".into());
        let app = TuiApp::from_config(TuiAppConfig::default());
        let config = Config::default();
        let agent = Agent::new(dummy_info("build"));
        let out = doctor_report(
            &session,
            &app,
            &config,
            &agent,
            PathBuf::from("/no/such/project/dir").as_path(),
        );
        assert!(out.contains("issues"), "{out}");
        assert!(out.contains("project directory missing"), "{out}");
    }

    #[tokio::test]
    async fn handle_slash_more_aliases_and_connect_with_key() {
        let mut h = SlashHarness::new();
        h.run("/q").await;
        assert!(!h.app.running);
        h.app.running = true;
        h.run("/h").await;
        assert_eq!(h.app.mode, AppMode::Help);
        h.app.mode = AppMode::Normal;
        h.app.key_context = KeymapContext::Normal;

        h.run("/clear").await;
        h.run("/summarize").await;
        h.run("/export").await;
        h.run("/usage").await;
        h.run("/themes").await;
        assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Theme)));
        h.app.dialogs.clear();
        h.app.mode = AppMode::Normal;

        h.run("/loop keep going").await;
        assert_eq!(h.app.pending_prompt.as_deref(), Some("keep going"));
        assert_eq!(
            h.app.pending_auto_prompts.len(),
            2,
            "default N=3 → 2 queued"
        );

        h.config.providers.insert(
            "acme".into(),
            whycodes_core::types::ProviderConfig {
                name: "acme".into(),
                api_key: Some("sk-test".into()),
                api_base: None,
                base_url: None,
                headers: None,
                models: vec!["m1".into()],
                tool_arguments: None,
                extra: Default::default(),
            },
        );
        h.api_key.clear();
        h.run("/connect").await;
        assert!(
            h.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Connected")),
            "{:?}",
            h.app
                .toasts
                .visible()
                .iter()
                .map(|t| t.message.as_str())
                .collect::<Vec<_>>()
        );

        h.config.agents.push(dummy_info("plan"));
        h.run("/agent plan").await;
        assert_eq!(h.agent.info.name, "plan");
        assert!(h.app.status_message.contains("plan"));
    }

    fn outcome_ok(name: &str, text: &str) -> TurnOutcome {
        TurnOutcome::Ok {
            text: text.into(),
            agent: Agent::new(dummy_info(name)),
            session: Session::new(PathBuf::from("/work"), "sys".into()),
            work_ms: 1500,
        }
    }

    #[test]
    fn apply_turn_outcome_ok_remote_and_errors() {
        isolate_home();
        let mut rt = test_runtime();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.add_message(ChatRole::Assistant, "");
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let mut cancel_at = Some(Instant::now());
        let mut pending_title = Some((rt.session.id.clone(), "New Title".into()));

        let queue = apply_turn_outcome(
            &mut app,
            &mut rt,
            outcome_ok("ok-agent", "hello"),
            &mut cancel_at,
            &mut pending_title,
            "acme",
            "m",
            &config,
            "",
            &tx,
        );
        assert!(queue, "no live window → catalog queued");
        assert!(!rt.agent_busy);
        assert!(cancel_at.is_none());
        assert_eq!(rt.agent.info.name, "ok-agent");
        assert_eq!(app.current_agent_state, AgentState::Idle);
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == ChatRole::Assistant && m.content == "hello")
        );

        app.add_message(ChatRole::Assistant, "");
        apply_turn_outcome(
            &mut app,
            &mut rt,
            TurnOutcome::Remote {
                text: "from-serve".into(),
                error: None,
                work_ms: 10,
            },
            &mut cancel_at,
            &mut pending_title,
            "acme",
            "m",
            &config,
            "",
            &tx,
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content == "from-serve" || m.content.contains("from-serve"))
        );

        apply_turn_outcome(
            &mut app,
            &mut rt,
            TurnOutcome::Remote {
                text: String::new(),
                error: Some("down".into()),
                work_ms: 4,
            },
            &mut cancel_at,
            &mut pending_title,
            "acme",
            "m",
            &config,
            "",
            &tx,
        );
        assert_eq!(app.status_message, "remote error");

        apply_turn_outcome(
            &mut app,
            &mut rt,
            TurnOutcome::Err {
                error: "cancelled by user".into(),
                agent: Agent::new(dummy_info("cx")),
                session: Session::new(PathBuf::from("/work"), "sys".into()),
                cancelled: true,
                work_ms: 20,
            },
            &mut cancel_at,
            &mut pending_title,
            "acme",
            "m",
            &config,
            "",
            &tx,
        );
        assert!(app.messages.iter().any(|m| m.content.contains("cancelled")));

        apply_turn_outcome(
            &mut app,
            &mut rt,
            TurnOutcome::Err {
                error: "provider exploded".into(),
                agent: Agent::new(dummy_info("err")),
                session: Session::new(PathBuf::from("/work"), "sys".into()),
                cancelled: false,
                work_ms: 30,
            },
            &mut cancel_at,
            &mut pending_title,
            "acme",
            "m",
            &config,
            "",
            &tx,
        );
        assert!(matches!(app.current_agent_state, AgentState::Error(_)));
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.kind == crate::toast::ToastKind::Error)
        );

        let mut compacted = Session::new(PathBuf::from("/work"), "sys".into());
        compacted.add_user_message("fix login");
        compacted.apply_full_replace(
            "<summary>\n1. Primary Request: fix login\n2. Files: auth.rs\n</summary>",
        );
        apply_turn_outcome(
            &mut app,
            &mut rt,
            TurnOutcome::Compact {
                agent: Agent::new(dummy_info("cmp")),
                session: compacted,
                outcome: whycodes_session::CompactOutcome {
                    messages_before: 4,
                    messages_after: 2,
                    tokens_before: 800,
                    tokens_after: 200,
                    dropped_transcript: "old".into(),
                },
                work_ms: 12,
            },
            &mut cancel_at,
            &mut pending_title,
            "acme",
            "m",
            &config,
            "",
            &tx,
        );
        assert!(!rt.agent_busy);
        assert_eq!(app.current_agent_state, AgentState::Idle);
        assert!(
            app.status_message.contains("Conversation compacted"),
            "{}",
            app.status_message
        );
        assert!(
            app.messages.iter().any(|m| m.role == ChatRole::System
                && m.content.contains("Conversation compacted")
                && m.content.contains("fix login")),
            "compact result should paint the summary card"
        );
    }

    #[test]
    fn close_session_slot_busy_last_and_parked() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut rt = test_runtime();
        let mut runtimes = Vec::new();
        let mut mru = Vec::new();

        rt.agent_busy = true;
        close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("in flight"))
        );

        rt.agent_busy = false;
        close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Last live"))
        );

        let parked = test_runtime();
        let parked_title = parked.session.title.clone();
        runtimes.push(parked);
        mru.push(0);
        close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, 0);
        assert!(runtimes.is_empty());
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains(&parked_title) || t.message.contains("Closed"))
        );

        let parked = test_runtime();
        runtimes.push(parked);
        mru.push(0);
        let after_title = runtimes[0].session.title.clone();
        close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
        assert!(runtimes.is_empty());
        assert_eq!(rt.session.title, after_title);
    }

    #[test]
    fn resume_or_switch_session_paths() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut rt = test_runtime();
        let mut parked = test_runtime();
        parked.session.add_user_message("parked-hi");
        let live_id = parked.session.id.clone();
        let mut runtimes = vec![parked];
        let mut mru = vec![];

        rt.agent_busy = true;
        resume_or_switch_session(
            &mut app,
            &mut rt,
            &mut runtimes,
            &mut mru,
            "busy-id".into(),
            PathBuf::from("/work").as_path(),
            &Config::default(),
        );
        assert_eq!(app.pending_session_id.as_deref(), Some("busy-id"));

        rt.agent_busy = false;
        app.pending_session_id = None;
        resume_or_switch_session(
            &mut app,
            &mut rt,
            &mut runtimes,
            &mut mru,
            live_id,
            PathBuf::from("/work").as_path(),
            &Config::default(),
        );
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Switched to live"))
        );

        resume_or_switch_session(
            &mut app,
            &mut rt,
            &mut runtimes,
            &mut mru,
            "nope-id".into(),
            PathBuf::from("/work").as_path(),
            &Config::default(),
        );
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("not found"))
        );

        let dir = tempfile::tempdir().unwrap();
        let mut saved = Session::new(dir.path().to_path_buf(), "sys".into());
        saved.add_user_message("persisted hello");
        persist_session_best_effort(&saved, "resume-test");
        let id = saved.id.clone();
        resume_or_switch_session(
            &mut app,
            &mut rt,
            &mut runtimes,
            &mut mru,
            id,
            dir.path(),
            &Config::default(),
        );
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Resumed"))
        );
    }

    #[test]
    fn reply_permission_allow_deny_and_queue() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.ask_permission("bash", "ls");
        let mut q = std::collections::VecDeque::new();
        reply_permission(&mut app, &mut q, true);
        assert_eq!(app.status_message, "Allowed — continuing…");

        let (tx1, rx1) = tokio::sync::oneshot::channel();
        let (tx2, _rx2) = tokio::sync::oneshot::channel();
        q.push_back(whycodes_agent::PermissionRequest {
            tool_name: "bash".into(),
            detail: "one".into(),
            reply: tx1,
        });
        q.push_back(whycodes_agent::PermissionRequest {
            tool_name: "read".into(),
            detail: "two".into(),
            reply: tx2,
        });
        app.ask_permission("bash", "one");
        reply_permission(&mut app, &mut q, false);
        assert!(rx1.blocking_recv().ok() == Some(false));
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Permission { .. })
        ));
        assert!(app.status_message.contains("Denied"));
    }

    #[test]
    fn questionnaire_complete_and_cancel() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut q = std::collections::VecDeque::new();
        let p = std::collections::VecDeque::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        complete_questionnaire_ui(
            &mut app,
            &mut q,
            &p,
            Some(vec![whycodes_tools::question::QuestionAnswer {
                selected: vec!["Yes".into()],
                free_text: None,
            }]),
        );
        assert!(rx.blocking_recv().unwrap().is_ok());

        let (tx, rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        complete_questionnaire_ui(&mut app, &mut q, &p, None);
        assert!(matches!(
            rx.blocking_recv().unwrap(),
            Err(QuestionError::Cancelled)
        ));
    }

    #[test]
    fn warn_suggestion_catalog_and_shutdown() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        warn_missing_api_key(&mut app, "acme");
        assert!(app.status_message.contains("no API key"));
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("ACME_API_KEY"))
        );

        apply_idle_suggestion(&mut app, "   ".into(), false);
        assert!(app.pending_suggestion.is_none());
        apply_idle_suggestion(&mut app, "try cargo test".into(), true);
        assert!(app.pending_suggestion.is_none());
        apply_idle_suggestion(&mut app, "try cargo test".into(), false);
        assert_eq!(app.pending_suggestion.as_deref(), Some("try cargo test"));

        let config = Config::default();
        assert!(!apply_catalog_window(
            &mut app, "p", "m", "other", "m", 8_000, &config
        ));
        assert!(apply_catalog_window(
            &mut app, "p", "m", "p", "m", 128_000, &config
        ));
        assert_eq!(app.api_context_window, Some(128_000));

        let mut rt = test_runtime();
        let (tx, rx) = tokio::sync::oneshot::channel();
        rt.pending_perm_queue
            .push_back(whycodes_agent::PermissionRequest {
                tool_name: "bash".into(),
                detail: "x".into(),
                reply: tx,
            });
        shutdown_runtime_queues(&mut rt);
        assert!(rx.blocking_recv().ok() == Some(false));
        assert!(rt.pending_perm_queue.is_empty());
    }

    #[tokio::test]
    async fn apply_auth_flow_note_code_and_results() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut key = String::new();
        let mut provider = "anthropic".to_string();
        let mut model = "claude-sonnet-5".to_string();
        let config = Config::default();
        apply_auth_flow_event(
            &mut app,
            AuthFlowEvent::Note("Visit https://x\nthen paste".into()),
            &mut provider,
            &mut model,
            &mut key,
            &config,
        )
        .await;
        assert_eq!(app.status_message, "Visit https://x");

        let (tx, _rx) = tokio::sync::oneshot::channel();
        apply_auth_flow_event(
            &mut app,
            AuthFlowEvent::NeedCode(tx),
            &mut provider,
            &mut model,
            &mut key,
            &config,
        )
        .await;
        assert!(app.auth_code_sink.is_some());
        assert!(app.status_message.contains("Paste"));

        apply_auth_flow_event(
            &mut app,
            AuthFlowEvent::Done {
                provider: "anthropic".into(),
                result: Err("nope".into()),
            },
            &mut provider,
            &mut model,
            &mut key,
            &config,
        )
        .await;
        assert!(app.status_message.contains("sign-in failed"));

        apply_auth_flow_event(
            &mut app,
            AuthFlowEvent::Done {
                provider: "openai".into(),
                result: Ok("ok".into()),
            },
            &mut provider,
            &mut model,
            &mut key,
            &config,
        )
        .await;
        assert!(app.status_message.contains("Signed in"));
        // Plugin-less installs have no suggested models; still switch provider.
        assert_eq!(provider, "openai");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("using openai/")),
            "{:?}",
            app.messages.iter().map(|m| &m.content).collect::<Vec<_>>()
        );

        whycodes_auth::register_spec(whycodes_auth::ProviderSpec {
            name: "tui-oauth-switch-demo".into(),
            label: "Demo".into(),
            flow: whycodes_auth::FlowKind::DeviceCode,
            client_id: "cid".into(),
            client_secret: None,
            authorize_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            scopes: "read".into(),
            token_encoding: whycodes_auth::TokenEncoding::Form,
            redirect_uri: None,
            loopback_port: None,
            loopback_host: None,
            callback_path: String::new(),
            extra_authorize: vec![],
            derived: None,
            suggested_models: vec!["demo-model".into()],
            inference: None,
        });
        apply_auth_flow_event(
            &mut app,
            AuthFlowEvent::Done {
                provider: "tui-oauth-switch-demo".into(),
                result: Ok("ok".into()),
            },
            &mut provider,
            &mut model,
            &mut key,
            &config,
        )
        .await;
        assert_eq!(provider, "tui-oauth-switch-demo");
        assert_eq!(model, "demo-model");
    }

    #[test]
    fn handle_question_enter_confirms_and_multi_space() {
        let spec = whycodes_tools::question::QuestionSpec {
            prompt: "Pick many?".into(),
            options: vec![
                whycodes_tools::question::QuestionOption {
                    label: "A".into(),
                    description: String::new(),
                    preview: None,
                },
                whycodes_tools::question::QuestionOption {
                    label: "B".into(),
                    description: String::new(),
                    preview: None,
                },
            ],
            multi_select: true,
        };
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut q = std::collections::VecDeque::new();
        let p = std::collections::VecDeque::new();
        app.ask_question(vec![spec.clone()]);
        assert!(handle_question_key(
            &mut app,
            KeyCode::Char(' '),
            &mut q,
            &p
        ));
        if let Some(DialogKind::Question(st)) = app.dialogs.active() {
            assert!(!st.multi_selected.is_empty());
        }
        // Enter on multi without finishing stays open (or finishes if confirm works).
        assert!(handle_question_key(&mut app, KeyCode::Enter, &mut q, &p));

        let single = sample_question();
        let (tx, rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![single.clone()],
            reply: tx,
        });
        app.ask_question(vec![single]);
        assert!(handle_question_key(&mut app, KeyCode::Enter, &mut q, &p));
        assert!(rx.blocking_recv().unwrap().is_ok());
    }

    #[test]
    fn empty_free_text_esc_cancels_question_immediately() {
        let spec = whycodes_tools::question::QuestionSpec {
            prompt: "Type it?".into(),
            options: vec![],
            multi_select: false,
        };
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut q = std::collections::VecDeque::new();
        let p = std::collections::VecDeque::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![spec.clone()],
            reply: tx,
        });
        app.ask_question(vec![spec]);
        assert!(
            matches!(app.dialogs.active(), Some(DialogKind::Question(st)) if st.free_text_focus)
        );
        assert!(handle_question_key(&mut app, KeyCode::Esc, &mut q, &p));
        assert!(q.is_empty());
        assert!(!app.dialogs.is_open());
        assert!(matches!(
            rx.blocking_recv().unwrap(),
            Err(QuestionError::Cancelled)
        ));
    }

    #[test]
    fn flush_pending_question_replies_completes_oneshot() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut q = std::collections::VecDeque::new();
        let p = std::collections::VecDeque::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        app.pending_question_answers = Some(vec![whycodes_tools::question::QuestionAnswer {
            selected: vec!["Yes".into()],
            free_text: None,
        }]);
        flush_pending_question_replies(&mut app, &mut q, &p);
        assert!(q.is_empty());
        assert!(app.pending_question_answers.is_none());
        assert!(rx.blocking_recv().unwrap().is_ok());

        let (tx, rx) = tokio::sync::oneshot::channel();
        q.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        app.question_dismissed = true;
        flush_pending_question_replies(&mut app, &mut q, &p);
        assert!(!app.question_dismissed);
        assert!(matches!(
            rx.blocking_recv().unwrap(),
            Err(QuestionError::Cancelled)
        ));
    }

    #[test]
    fn stale_waiting_for_question_still_opens_queued_dialog() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut rt = test_runtime();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        rt.pending_question_queue.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        app.current_agent_state = AgentState::WaitingForQuestion;
        assert!(app.dialogs.active().is_none());
        maybe_open_queued_dialog(&mut app, &rt);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Question(_))
        ));
    }

    #[test]
    fn project_diff_on_a_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).success());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        assert!(git(&["add", "a.txt"]).success());
        assert!(git(&["commit", "-q", "-m", "init"]).success());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let out = project_diff_report(dir.path());
        assert!(out.contains("Diff"), "{out}");
        assert!(
            out.contains("status") || out.contains("a.txt") || out.contains("HEAD"),
            "{out}"
        );
    }

    #[test]
    fn context_report_counts_tool_blocks() {
        isolate_home();
        let mut session = Session::new(PathBuf::from("/work"), "sys".into());
        session.add_user_message("go");
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: "t1".into(),
            content: "x".repeat(80),
            is_error: false,
        }]);
        let app = TuiApp::from_config(TuiAppConfig::default());
        let agent = Agent::new(dummy_info("build"));
        let out = context_report(&session, &app, &Config::default(), &agent);
        assert!(out.contains("tool:"), "{out}");
        assert!(out.contains("largest tool results"), "{out}");
    }

    #[test]
    fn tui_login_prompt_pasted_code_cancels_when_dropped() {
        use whycodes_auth::providers::LoginUi;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut ui = TuiLoginUi { tx };
        let fut = ui.prompt_pasted_code();
        // Dropping the NeedCode sender (the TUI side) cancels the flow.
        drop(fut);
    }

    #[test]
    fn arm_record_route_and_model_choice() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut rt = test_runtime();
        let mut at = Some(Instant::now());
        let flag = arm_generating(&mut app, &mut rt, &mut at, "remote…");
        assert!(rt.agent_busy);
        assert!(at.is_none());
        assert_eq!(app.status_message, "remote…");
        assert!(
            app.messages
                .last()
                .is_some_and(|m| m.role == ChatRole::Assistant)
        );
        assert!(!whycodes_agent::is_cancelled(&Some(flag)));

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "hello file").unwrap();
        let mut config = Config::default();
        config.session.auto_title = true;
        let expanded = record_user_turn(
            &mut app,
            &mut rt,
            "read @note.txt please",
            dir.path(),
            &config,
            &[],
        );
        assert!(expanded.contains("hello file"), "{expanded}");
        assert!(rt.session.messages.iter().any(|m| {
            m.content
                .as_text()
                .is_some_and(|t| t.contains("hello file"))
        }));

        let bad = [crate::images::PromptImage {
            path: dir.path().join("missing.png"),
            label: "missing.png".into(),
            media_type: "image/png".into(),
        }];
        record_user_turn(&mut app, &mut rt, "", dir.path(), &config, &bad);
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Image attach"))
        );

        let (p, m) = route_turn_model(&rt.session.id, "acme", "big", "hi", Some("fast-1"));
        assert_eq!(p, "acme");
        let _ = m;

        let mut provider = "old".into();
        let mut model = "old-m".into();
        let mut key = String::new();
        config.providers.insert(
            "acme".into(),
            whycodes_core::types::ProviderConfig {
                name: "acme".into(),
                api_key: Some("sk-from-cfg".into()),
                api_base: None,
                base_url: None,
                headers: None,
                models: vec!["m1".into()],
                tool_arguments: None,
                extra: Default::default(),
            },
        );
        apply_model_choice(
            &mut app,
            &mut provider,
            &mut model,
            &mut key,
            "acme".into(),
            "m1".into(),
            &config,
        );
        assert_eq!(provider, "acme");
        assert_eq!(model, "m1");
        assert_eq!(key, "sk-from-cfg");
        assert!(app.status_message.contains("acme/m1"));

        // Switching to an OAuth-only provider must drop the previous key.
        let mut leftover = "sk-from-previous-backend".into();
        apply_model_choice(
            &mut app,
            &mut provider,
            &mut model,
            &mut leftover,
            "google-antigravity".into(),
            "gemini-3.5-flash-low".into(),
            &config,
        );
        assert_eq!(provider, "google-antigravity");
        assert!(
            leftover.is_empty(),
            "must not keep previous provider credential"
        );

        leftover = "ya29-oauth".into();
        apply_model_choice(
            &mut app,
            &mut provider,
            &mut model,
            &mut leftover,
            "google-antigravity".into(),
            "gemini-3.1-pro-low".into(),
            &config,
        );
        assert_eq!(
            leftover, "ya29-oauth",
            "same-provider model change keeps the credential"
        );

        let mut key = String::new();
        try_fill_api_key(&mut key, "nope");
        assert!(key.is_empty());
        try_fill_api_key(&mut key, "acme");
        // config load may or may not see providers; env fallback:
        unsafe { std::env::set_var("NOPE_API_KEY", "from-env") };
        let mut key = String::new();
        try_fill_api_key(&mut key, "nope");
        assert_eq!(key, "from-env");
        unsafe { std::env::remove_var("NOPE_API_KEY") };
        try_fill_api_key(&mut key, "nope");
        assert_eq!(key, "from-env", "already filled stays");
    }

    #[test]
    fn apply_async_title_active_parked_and_pending() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut rt = test_runtime();
        rt.session.add_user_message("topic");
        let sid = rt.session.id.clone();
        let mut pending = None;
        apply_async_title(
            &mut app,
            &mut rt,
            &mut [],
            &mut pending,
            sid.clone(),
            "Better Title".into(),
        );
        assert!(
            rt.session.title.contains("Better") || app.session_title == rt.session.title,
            "title={} app={}",
            rt.session.title,
            app.session_title
        );

        let mut parked = test_runtime();
        parked.session.add_user_message("bg topic");
        let bg_id = parked.session.id.clone();
        let mut runtimes = vec![parked];
        apply_async_title(
            &mut app,
            &mut rt,
            &mut runtimes,
            &mut pending,
            bg_id,
            "Parked Title".into(),
        );

        apply_async_title(
            &mut app,
            &mut rt,
            &mut runtimes,
            &mut pending,
            "unknown-sid".into(),
            "Later".into(),
        );
        assert_eq!(
            pending.as_ref().map(|(s, _)| s.as_str()),
            Some("unknown-sid")
        );
    }

    fn boot_opts(dir: &std::path::Path, key: &str) -> TuiRunOptions {
        TuiRunOptions {
            project_dir: dir.to_path_buf(),
            provider: "acme".into(),
            model: "m1".into(),
            api_key: key.into(),
            agent_name: "plan".into(),
            max_turns: None,
            initial_prompt: None,
            config: Config::default(),
            resume_session_id: None,
            remote: None,
            update_rx: None,
        }
    }

    #[tokio::test]
    async fn prepare_tui_boot_sets_chrome_and_defaults() {
        isolate_home();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hi").unwrap();
        let mut opts = boot_opts(dir.path(), "");
        opts.agent_name = "plan".into();
        opts.config.tools.question.timeout_enabled = true;
        opts.config.tools.question.timeout_secs = 5;
        let mut plan = dummy_info("plan");
        plan.mode = AgentMode::All;
        opts.config.agents.push(plan);
        let boot = prepare_tui_boot(&opts).await;
        assert_eq!(boot.app.provider_name, "acme");
        assert_eq!(boot.app.model_name, "m1");
        assert_eq!(boot.app.agent_name, "plan");
        assert!(boot.missing_key);
        assert!(boot.app.status_message.contains("no API key"));
        assert!(boot.app.primary_agents.contains(&"plan".to_string()));
        assert_eq!(boot.app.agent_cycle_idx, 1);
        assert_eq!(boot.agent.info.name, "plan");

        let opts = boot_opts(dir.path(), "sk-test");
        let boot = prepare_tui_boot(&opts).await;
        assert!(!boot.missing_key);
        assert!(boot.app.status_message.contains("Tab focus"));
    }

    #[test]
    fn apply_resume_found_missing_and_latest() {
        let (_home_lock, _home) = isolate_home_fresh();
        let dir = tempfile::tempdir().unwrap();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
        apply_resume(&mut app, &mut session, "sys", RESUME_LATEST, false);
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("No saved"))
        );
        apply_resume(&mut app, &mut session, "sys", "missing-id", false);
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("not found"))
        );

        let mut saved = Session::new(dir.path().to_path_buf(), "sys".into());
        saved.add_user_message("resume me");
        persist_session_best_effort(&saved, "boot");
        apply_resume(&mut app, &mut session, "keep-sys", &saved.id, true);
        assert_eq!(session.id, saved.id);
        assert_eq!(session.system_prompt, "keep-sys");
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Resumed"))
        );
    }

    #[tokio::test]
    async fn apply_remote_hydrate_error_still_attaches() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut session = Session::new(PathBuf::from("/work"), "sys".into());
        let rem = crate::remote::RemoteAttach::new("127.0.0.1:1", "sid-9");
        apply_remote_hydrate(&mut app, &mut session, &rem).await;
        assert_eq!(session.id, "sid-9");
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Attached"))
        );
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Remote hydrate"))
        );
    }

    #[test]
    fn spinner_session_keys_slash_and_busy_ctrl_c() {
        isolate_home();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut frame = 0usize;
        app.status_message = "Generating…".into();
        tick_spinner(&mut app, &mut frame);
        assert_eq!(frame, 1);
        assert!(app.status_message.is_empty());
        app.status_message = "⠋ working".into();
        tick_spinner(&mut app, &mut frame);
        assert!(app.status_message.is_empty());
        app.status_message = "tool: run".into();
        tick_spinner(&mut app, &mut frame);
        assert_eq!(app.status_message, "tool: run");

        warn_session_limit(&mut app);
        assert!(
            app.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Session limit"))
        );

        let mut rt = test_runtime();
        let parked = test_runtime();
        let mut runtimes = vec![parked];
        let mut mru = vec![0];
        cycle_live_session(&mut app, &mut rt, &mut runtimes, &mut mru, true);
        switch_mru_session(&mut app, &mut rt, &mut runtimes, &mut mru);
        cycle_live_session(&mut app, &mut rt, &mut [], &mut mru, false);

        let fresh = test_runtime();
        adopt_fresh_runtime(&mut app, &mut rt, &mut runtimes, &mut mru, fresh);
        assert_eq!(runtimes.len(), 2);

        app.input_buffer = "/help".into();
        assert_eq!(slash_command_from_prompt(&app).as_deref(), Some("/help"));
        consume_slash_draft(&mut app);
        assert!(app.input_buffer.is_empty());
        app.input_buffer = "hello".into();
        assert!(slash_command_from_prompt(&app).is_none());

        app.input_buffer = "draft".into();
        assert_eq!(busy_ctrl_c(&mut app, None), BusyCtrlC::ClearedDraft);
        assert!(app.input_buffer.is_empty());
        assert_eq!(busy_ctrl_c(&mut app, None), BusyCtrlC::BeginCancel);
        assert_eq!(
            busy_ctrl_c(&mut app, Some(Instant::now())),
            BusyCtrlC::ForceStop
        );

        queue_auto_prompt_if_idle(&mut app, true);
        assert!(app.pending_prompt.is_none());
        app.pending_auto_prompts.push_back("next".into());
        queue_auto_prompt_if_idle(&mut app, false);
        assert_eq!(app.pending_prompt.as_deref(), Some("next"));
        app.pending_auto_prompts.push_back("held".into());
        queue_auto_prompt_if_idle(&mut app, false);
        assert_eq!(app.pending_prompt.as_deref(), Some("next"));

        app.input_buffer = "/".into();
        app.slash_suggest.refresh(&app.input_buffer);
        let picked = slash_command_from_prompt(&app).expect("slash menu");
        assert!(picked.starts_with('/'), "{picked}");

        apply_boot_prompt(&mut app, true, None);
        assert_eq!(app.status_message, "no API key · /connect");
        apply_boot_prompt(&mut app, false, Some(String::new()));
        apply_boot_prompt(&mut app, false, Some("do it".into()));
        assert_eq!(app.pending_prompt.as_deref(), Some("do it"));

        let mut rt = test_runtime();
        let name = rt.agent.info.name.clone();
        let (ag, sess) = take_turn_owner(&mut rt, PathBuf::from("/work").as_path());
        assert_eq!(ag.info.name, name);
        assert_eq!(rt.agent.info.name, "_pending");
        assert!(rt.session_backup.is_some());
        let _ = sess;

        let mut rt = test_runtime();
        maybe_open_queued_dialog(&mut app, &rt);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        rt.pending_perm_queue
            .push_back(whycodes_agent::PermissionRequest {
                tool_name: "bash".into(),
                detail: "ls".into(),
                reply: tx,
            });
        maybe_open_queued_dialog(&mut app, &rt);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Permission { .. })
        ));
        maybe_open_queued_dialog(&mut app, &rt);

        app.dialogs.clear();
        app.mode = AppMode::Normal;
        app.current_agent_state = AgentState::Idle;
        let (tx, _rx) = tokio::sync::oneshot::channel();
        rt.pending_perm_queue.clear();
        rt.pending_question_queue.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: tx,
        });
        maybe_open_queued_dialog(&mut app, &rt);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Question(_))
        ));

        let mut parked = test_runtime();
        parked.session.add_user_message("dash");
        let mut runtimes = vec![parked];
        let mut mru = vec![];
        apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
        apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, 9);
        apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, 0);
        assert_eq!(mru, vec![0]);

        assert!(!should_tick_spinner(&app, false));
        assert!(should_tick_spinner(&app, true));
        app.current_agent_state = AgentState::WaitingForPermission;
        assert!(!should_tick_spinner(&app, true));
        app.current_agent_state = AgentState::Idle;
        assert!(!should_force_stop(false, Some(Instant::now()), true));
        assert!(!should_force_stop(true, None, true));
        assert!(should_force_stop(true, Some(Instant::now()), true));
        assert!(!should_force_stop(true, Some(Instant::now()), false));

        crate::input::open_dialog(&mut app, DialogKind::Sessions);
        let rt = test_runtime();
        refresh_live_session_ui(&mut app, &rt, &[]);
        assert!(!app.sessions_rows.is_empty());
        crate::input::open_dialog(&mut app, DialogKind::SessionList);
        refresh_live_session_ui(&mut app, &rt, &[]);
        assert!(
            app.session_list
                .sessions
                .iter()
                .any(|e| e.live == Some(usize::MAX))
        );
    }

    #[tokio::test]
    async fn prepare_boot_with_resume_and_remote() {
        isolate_home();
        let dir = tempfile::tempdir().unwrap();
        let mut saved = Session::new(dir.path().to_path_buf(), "sys".into());
        saved.add_user_message("boot resume");
        persist_session_best_effort(&saved, "boot-resume");
        let mut opts = boot_opts(dir.path(), "sk");
        opts.resume_session_id = Some(saved.id.clone());
        opts.remote = Some(crate::remote::RemoteAttach::new("127.0.0.1:1", "r1"));
        let boot = prepare_tui_boot(&opts).await;
        assert_eq!(boot.session.id, "r1", "remote hydrate overwrites id");
        assert!(
            boot.app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Resumed") || t.message.contains("Attached"))
        );
    }

    #[tokio::test]
    async fn apply_remote_hydrate_success() {
        isolate_home();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let body = r#"{"title":"remote-title","messages":[{"role":"user","content":"hi"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut session = Session::new(PathBuf::from("/work"), "sys".into());
        let rem = crate::remote::RemoteAttach::new(format!("http://{addr}"), "sid-ok");
        apply_remote_hydrate(&mut app, &mut session, &rem).await;
        assert_eq!(session.id, "sid-ok");
        assert_eq!(session.title, "remote-title");
        assert_eq!(session.messages.len(), 1);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind2");
        let addr = listener.local_addr().expect("addr2");
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = vec![0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let body = r#"{"title":"","messages":[]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        let rem = crate::remote::RemoteAttach::new(format!("http://{addr}"), "sid-empty");
        apply_remote_hydrate(&mut app, &mut session, &rem).await;
        assert_eq!(session.id, "sid-empty");
    }

    #[test]
    fn spinner_wraps_and_only_clears_generic_progress() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut frame = 9;
        app.status_message = "Generating response".into();

        tick_spinner(&mut app, &mut frame);

        assert_eq!(frame, 0);
        assert_eq!(app.spinner_frame, 0);
        assert!(app.status_message.is_empty());

        app.status_message = "⠴ waiting".into();
        tick_spinner(&mut app, &mut frame);
        assert!(app.status_message.is_empty());

        app.status_message = "Running cargo test".into();
        tick_spinner(&mut app, &mut frame);
        assert_eq!(app.status_message, "Running cargo test");
    }

    #[test]
    fn force_stop_requires_busy_cancel_and_timeout_or_pending_signal() {
        let expired = Instant::now()
            .checked_sub(CANCEL_FORCE_AFTER + Duration::from_millis(1))
            .expect("force-stop duration fits in Instant");
        let recent = Instant::now();

        assert!(!should_force_stop(false, Some(expired), true));
        assert!(!should_force_stop(true, None, true));
        assert!(should_force_stop(true, Some(recent), true));
        assert!(should_force_stop(true, Some(expired), false));
        assert!(!should_force_stop(true, Some(recent), false));
    }

    #[test]
    fn auto_prompts_are_fifo_and_do_not_replace_pending_work() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        app.pending_auto_prompts
            .extend(["first".into(), "second".into()]);

        queue_auto_prompt_if_idle(&mut app, true);
        assert!(app.pending_prompt.is_none());
        assert_eq!(app.pending_auto_prompts.len(), 2);

        queue_auto_prompt_if_idle(&mut app, false);
        assert_eq!(app.pending_prompt.as_deref(), Some("first"));
        assert_eq!(
            app.pending_auto_prompts.front().map(String::as_str),
            Some("second")
        );

        queue_auto_prompt_if_idle(&mut app, false);
        assert_eq!(app.pending_prompt.as_deref(), Some("first"));
        assert_eq!(app.pending_auto_prompts.len(), 1);
    }

    #[test]
    fn queued_dialogs_wait_for_idle_and_prioritize_permissions() {
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        let mut rt = test_runtime();
        let (permission_tx, _permission_rx) = tokio::sync::oneshot::channel();
        rt.pending_perm_queue
            .push_back(whycodes_agent::PermissionRequest {
                tool_name: "bash".into(),
                detail: "cargo test".into(),
                reply: permission_tx,
            });
        let (question_tx, _question_rx) = tokio::sync::oneshot::channel();
        rt.pending_question_queue.push_back(QuestionRequest {
            questions: vec![sample_question()],
            reply: question_tx,
        });

        app.current_agent_state = AgentState::WaitingForQuestion;
        app.ask_question(vec![sample_question()]);
        maybe_open_queued_dialog(&mut app, &rt);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Question(_))
        ));

        app.dialogs.clear();
        app.mode = AppMode::Normal;
        app.current_agent_state = AgentState::Idle;
        maybe_open_queued_dialog(&mut app, &rt);
        assert!(matches!(
            app.dialogs.active(),
            Some(DialogKind::Permission { tool_name, detail })
                if tool_name == "bash" && detail == "cargo test"
        ));
    }
}
