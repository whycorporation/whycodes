//! TUI event loop — streaming agent + permission dialogs.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::color::{QuantizingBackend, detect_color_mode, set_active_color_mode};
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
use whycodes_core::types::{AgentMode, ApprovalMode};
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

mod persist;
mod slash;
#[cfg(test)]
mod tests;

pub use persist::resolve_and_load_session;
use persist::*;
pub use slash::SlashContext;
use slash::*;

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

    // Paint, then hydrate. Anything the first 80×24 home frame does not need
    // is deferred until after the first `terminal.draw` → `record_draw()`.
    // See issue #49: this pile used to run serially before the first frame.
    let file_index = whycodes_index::WorkspaceIndex::start(Vec::new());
    // Real workspace roots are hydrated after first paint; the empty index
    // keeps `@` picker inert for frame 0 without paying `canonicalize` + scan.
    app.set_file_index(file_index.clone());

    app.provider_name = opts.provider.clone();
    app.model_name = opts.model.clone();
    app.reasoning_effort = opts.config.session.reasoning_effort.clone();
    app.approval_mode = opts.config.general.approval_mode.unwrap_or_default();
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
    // Git branch is read via `.git/HEAD` fast path (no `git` spawn). That's
    // cheap enough to keep before paint; full `git` fallback is deferred.
    app.refresh_git_branch();
    app.apply_context_window(
        &opts.provider,
        &opts.model,
        opts.config
            .configured_context_window(&opts.provider, &opts.model),
        opts.config.session.max_context_tokens as u64,
    );

    let mut config = opts.config.clone();

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
    // Deferred: AGENTS.md + memory + plugins each touch disk/SQLite. Home has
    // no fenced code and needs no tool plugins, so the first frame uses a
    // minimal prompt; the full prompt is hydrated after paint.
    let system_prompt = Agent::with_runtime_context(&base);

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
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);

    let mut session = Session::new(opts.project_dir.clone(), system_prompt.clone());
    app.session_title = session.title.clone();
    app.session_id = session.id.clone();
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
    // Unit tests drive CLI `cmd_run` / `cmd_connect` through this entry
    // without opening a terminal. `WHYCODES_TEST_TUI=upgrade` asks the CLI
    // to install after restore; anything else is a clean quit.
    // CLI unit tests set this so `cmd_run` / `cmd_connect` can call `run`
    // without opening a terminal. Never set in production.
    if let Ok(kind) = std::env::var("WHYCODES_TEST_TUI") {
        let _opts = opts;
        return Ok(if kind == "upgrade" {
            TuiExit::Upgrade
        } else {
            TuiExit::Quit
        });
    }

    // Wall clock for the Cline-style exit summary (process open → quit).
    let session_started = Instant::now();

    let boot = prepare_tui_boot(&opts).await;
    let mut app = boot.app;
    let mut config = boot.config;
    let mut file_index = boot.file_index;
    let mut agent = boot.agent;
    let session = boot.session;
    let history = boot.history;
    let perm_prompter = boot.perm_prompter;
    let question_prompter = boot.question_prompter;
    let perm_rx = boot.perm_rx;
    let question_rx = boot.question_rx;
    let session_claims = boot.session_claims;
    let mut missing_key = boot.missing_key;
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
    let color_mode = detect_color_mode();
    set_active_color_mode(color_mode);
    app.config.color_mode = color_mode;
    app.config.extra.quantize_for(color_mode);
    let backend = QuantizingBackend::new(CrosstermBackend::new(tui_out), color_mode);
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
        Some(serde_json::json!({
            "term_w": tw,
            "term_h": th,
            "color_mode": color_mode.as_str(),
            "term_program": std::env::var("TERM_PROGRAM").ok(),
            "term": std::env::var("TERM").ok(),
        })),
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
                    // Paint, then hydrate. Deferred boot work that is not needed
                    // for the first 80×24 home frame (issue #49).
                    // Syntax theme was skipped in `TuiApp::new` (syntect cache
                    // is ~2 ms cold).
                    let hydrate_before = capture_first_frame_hydrate_chrome(&app);
                    app.config.theme.apply_syntax_theme();
                    // Auth plugin dir walk was deferred from `async_main`.
                    {
                        let mut dirs = Vec::new();
                        if let Ok(p) = whycodes_config::Config::default_path()
                            && let Some(parent) = p.parent()
                        {
                            dirs.push(parent.join("plugins"));
                        }
                        dirs.push(whycodes_core::project_dir(&project_dir).join("plugins"));
                        let loaded = whycodes_auth::plugin::load_from_dirs(&dirs);
                        if loaded > 0 {
                            tracing::debug!(
                                count = loaded,
                                "hydrated auth plugins after first frame"
                            );
                        }
                    }
                    // Real workspace file index (canonicalize + scan) — empty
                    // index was used for the first frame so `@` picker does not
                    // block TTFF.
                    {
                        let real = whycodes_index::WorkspaceIndex::start(
                            whycodes_index::WorkspaceIndex::project_roots(&project_dir),
                        );
                        app.set_file_index(real.clone());
                        rt.agent.set_file_index(real.clone());
                        file_index = real;
                    }
                    // Shell plugins (plugins.toml + plugin.json discovery).
                    rt.agent.hydrate_plugins(Some(project_dir.as_path()));
                    // Full system prompt: AGENTS.md + sibling files + memory.
                    // Boot used only runtime context; hydrate the rest now.
                    {
                        let base = rt.agent.system_prompt();
                        let with_agents =
                            whycodes_agent::agent::Agent::with_agents_md(&base, &project_dir);
                        let full = with_project_memory(&with_agents, &project_dir, &config, None);
                        rt.session.set_system_prompt(&full);
                    }
                    // Session picker — home paints with empty list then fills.
                    if app.session_list.sessions.is_empty() {
                        let entries = load_session_entries();
                        if !entries.is_empty() {
                            app.session_list.sessions = entries;
                        }
                    }
                    // API key was deferred before `whycodes_tui::run` to avoid
                    // blocking `auth.json` I/O before first paint. Fetch the
                    // env/config key synchronously now; OAuth is async and
                    // stays lazy until the first turn (`ensure_api_key`).
                    if api_key.is_empty() {
                        let env_var = format!("{}_API_KEY", provider.to_uppercase());
                        let mut fetched: Option<String> = None;
                        if let Ok(v) = std::env::var(&env_var)
                            && !v.is_empty()
                        {
                            fetched = Some(v);
                        }
                        if fetched.is_none()
                            && let Some(pc) = config.get_provider(&provider)
                            && let Some(k) = &pc.api_key
                            && !k.is_empty()
                        {
                            fetched = Some(k.clone());
                        }
                        if let Some(k) = fetched {
                            api_key = k;
                            missing_key = api_key.is_empty()
                                && whycodes_llm::provider_requires_api_key(
                                    &provider,
                                    Some(&config),
                                );
                            app.status_message = if missing_key {
                                format!(
                                    "agent={}  {}/{}  — no API key · /connect  /help",
                                    app.agent_name, provider, model
                                )
                            } else {
                                format!(
                                    "agent={}  {}/{}  — Tab focus  Ctrl+T agent  Esc cancel  /help",
                                    app.agent_name, provider, model
                                )
                            };
                        }
                    }

                    // After first paint: MCP connect + code RAG auto-index.
                    // Both can block; doing them here keeps startup feel snappy.
                    rt.agent.load_mcp(&config).await;
                    maybe_session_auto_index(&project_dir, &config, &mut app);
                    refresh_sidebar(&mut app, &config, &file_index);
                    load_app_todos(&mut app);
                    settle_first_frame_hydrate(&mut app, &hydrate_before, animate);
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
                    &mut rt,
                    &mut cancel_requested_at,
                    &config,
                    &project_dir,
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
            if let Some(mode) = app.pending_approval_mode.take() {
                apply_approval_mode(&mut app, &mut rt.agent, &mut config, mode);
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
                        &mut rt,
                        &mut cancel_requested_at,
                        &config,
                        &project_dir,
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
                                        &mut rt,
                                        &mut cancel_requested_at,
                                        &config,
                                        &project_dir,
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
                                        &mut rt,
                                        &mut cancel_requested_at,
                                        &config,
                                        &project_dir,
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
                                        &mut rt,
                                        &mut cancel_requested_at,
                                        &config,
                                        &project_dir,
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
fn force_stop_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    // Always re-signal cancel in case the task is still cooperative.
    if let Some(flag) = rt.cancel_flag.as_ref() {
        request_cancel(flag);
    }
    while let Some(req) = rt.pending_question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
    }
    while let Some(req) = rt.pending_perm_queue.pop_front() {
        let _ = req.reply.send(false);
    }

    if let Some(h) = rt.turn_join.take() {
        h.abort();
    }

    // If the task finished in the race window, prefer its restored agent/session.
    let mut got_outcome = false;
    while let Ok(outcome) = rt.done_rx.try_recv() {
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
                rt.agent = a;
                rt.session = s;
            }
            TurnOutcome::Remote { .. } => {}
        }
    }

    if !got_outcome {
        // Task dropped without returning — rebuild agent; restore session snapshot.
        if let Some(backup) = rt.session_backup.take() {
            rt.session = backup;
        }
        let preferred = if app.agent_name.is_empty() {
            rt.agent.info.name.clone()
        } else {
            app.agent_name.clone()
        };
        rebuild_agent_after_force_stop(
            &mut rt.agent,
            &mut rt.session,
            config,
            project_dir,
            &preferred,
            rt.event_tx.clone(),
            Arc::clone(&rt.perm_prompter),
            Arc::clone(&rt.question_prompter),
            file_index,
        );
    } else {
        rt.session_backup.take();
    }

    rt.agent_busy = false;
    rt.cancel_flag = None;
    *cancel_requested_at = None;

    app.finish_open_thinking();
    app.current_agent_state = AgentState::Idle;
    app.status_message = format_turn_done_status(
        app,
        rt.agent.info.name.as_str(),
        &app.provider_name,
        &app.model_name,
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
    persist_session_best_effort(&rt.session, "force_cancelled");
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
        app.mode = AppMode::Dialog;
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
                        let display = whycodes_llm::format_turn_error(&whycodes_core::Error::llm(
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
                    whycodes_llm::format_turn_error(&whycodes_core::Error::llm(error.clone()));
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

fn apply_approval_mode_raw(app: &mut TuiApp, agent: &mut Agent, config: &mut Config, raw: &str) {
    let Some(mode) = ApprovalMode::parse(raw) else {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            format!("Unknown mode '{raw}' (auto, important, manual)"),
        );
        return;
    };
    apply_approval_mode(app, agent, config, mode);
}

fn apply_approval_mode(
    app: &mut TuiApp,
    agent: &mut Agent,
    config: &mut Config,
    mode: ApprovalMode,
) {
    app.approval_mode = mode;
    config.general.approval_mode = Some(mode);
    agent.set_approval_mode(mode);
    if let Err(e) = persist_general_approval_mode(mode) {
        tracing::warn!(error = %e, "failed to persist general.approval_mode");
    }
    app.status_message = format!("Approval mode → {}", mode.label());
    app.mark_dirty();
}

fn persist_general_approval_mode(mode: ApprovalMode) -> anyhow::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    let mut disk = Config::load()?;
    disk.general.approval_mode = Some(mode);
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

/// Chrome captured just after the first paint, before deferred hydrate.
pub(super) struct FirstFrameHydrateChrome {
    sessions: usize,
    status: String,
    file_tree: Vec<String>,
    mcp_status: Vec<String>,
}

pub(super) fn capture_first_frame_hydrate_chrome(app: &TuiApp) -> FirstFrameHydrateChrome {
    FirstFrameHydrateChrome {
        sessions: app.session_list.sessions.len(),
        status: app.status_message.clone(),
        file_tree: app.sidebar.file_tree.clone(),
        mcp_status: app.sidebar.mcp_status.clone(),
    }
}

/// True when first-frame hydrate changed chrome the user can already see.
///
/// Empty-project idle home (no recents, same status, hidden sidebar, no
/// toasts) must not schedule a second paint. MCP / index / plugins still run;
/// only the unconditional follow-up draw is gated.
pub(super) fn first_frame_hydrate_needs_paint(
    before: &FirstFrameHydrateChrome,
    app: &TuiApp,
) -> bool {
    if app.session_list.sessions.len() != before.sessions {
        return true;
    }
    if app.status_message != before.status {
        return true;
    }
    if app.sidebar.visible
        && (app.sidebar.file_tree != before.file_tree
            || app.sidebar.mcp_status != before.mcp_status)
    {
        return true;
    }
    !app.toasts.is_empty()
}

/// After first-frame hydrate: dirty only if visible chrome changed.
///
/// `replace_todos` / similar can `mark_dirty` even when the helper is false
/// (empty == empty is a no-op, but other hydrate work may have set the flag).
/// Empty-project idle must not keep that leftover paint, or the harness
/// counts one extra draw over 3s (~0.3/s). Animation still stays live.
pub(super) fn settle_first_frame_hydrate(
    app: &mut TuiApp,
    before: &FirstFrameHydrateChrome,
    animate: bool,
) {
    if first_frame_hydrate_needs_paint(before, app) {
        app.mark_dirty();
        return;
    }
    if !animate {
        app.needs_redraw = false;
        app.pending_full_clears = 0;
    }
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
