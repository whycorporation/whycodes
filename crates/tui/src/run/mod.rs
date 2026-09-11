//! TUI event loop — streaming agent + permission dialogs.

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::color::{ColorMode, QuantizingBackend, detect_color_mode, set_active_color_mode};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyCode, KeyEventKind, KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    size as term_size, supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
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

mod import;
mod persist;
mod slash;
#[cfg(test)]
mod tests;

pub(crate) use import::mark_import_declined;
use import::*;
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
        send_auth_event(&self.tx, event);
    }
}

fn send_auth_event(tx: &mpsc::UnboundedSender<AuthFlowEvent>, event: AuthFlowEvent) {
    seed_unbounded(tx, event, "auth-flow event");
}

fn seed_unbounded<T>(tx: &mpsc::UnboundedSender<T>, value: T, what: &'static str) {
    if tx.send(value).is_err() {
        tracing::debug!("{what} dropped: TUI event loop closed");
    }
}

fn headless_busy_key_ready(ev: Option<&Event>) -> bool {
    let Some(Event::Key(key)) = ev else {
        return true;
    };
    if key.kind != KeyEventKind::Press {
        return true;
    }
    if key
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        return true;
    }
    // Hold overlay answers until the dialog owns the keyboard.
    !matches!(
        key.code,
        KeyCode::Char('y' | 'Y' | 'n' | 'N' | 'a' | 'A' | 'd' | 'D') | KeyCode::Enter
    )
}

/// `WHYCODES_TEST_LLM` replaces the registry with a repeating
/// [`whycodes_llm::ScriptedProvider`] so a headless turn never hits the network.
/// Empty / missing env is a no-op (production never sets this).
fn inject_test_llm(agent: &mut Agent, provider: &str) {
    let Ok(text) = std::env::var("WHYCODES_TEST_LLM") else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let mut registry = whycodes_llm::ProviderRegistry::new();
    if text == "ASK" {
        registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
            provider.to_string(),
            [
                vec![whycodes_llm::ScriptedStep::ToolCall {
                    id: "q1".into(),
                    name: "question".into(),
                    input: serde_json::json!({
                        "questions": [{
                            "prompt": "Pick?",
                            "options": [
                                {"label": "Yes"},
                                {"label": "No"}
                            ]
                        }]
                    }),
                }],
                vec![whycodes_llm::ScriptedStep::Text("asked-ok".into())],
            ],
        )));
    } else if text == "SHELL" {
        // One tool call, then a text reply — repeating would loop tools forever.
        registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
            provider.to_string(),
            [
                vec![whycodes_llm::ScriptedStep::ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "echo hi"}),
                }],
                vec![whycodes_llm::ScriptedStep::Text("shell-done".into())],
            ],
        )));
    } else {
        let step = if text == "FAIL" {
            whycodes_llm::ScriptedStep::FailOpen("scripted-fail".into())
        } else if text == "HANG" {
            whycodes_llm::ScriptedStep::Hang(std::time::Duration::from_secs(30))
        } else {
            whycodes_llm::ScriptedStep::Text(text)
        };
        registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
            provider.to_string(),
            [step],
        )));
    }
    agent.set_provider_registry(registry);
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
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = whycodes_auth::error::Result<String>> + Send + '_>,
    > {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(AuthFlowEvent::NeedCode(tx));
        Box::pin(async move {
            rx.await.map_err(|_| {
                whycodes_auth::AuthError::FlowCancelled("sign-in dismissed".to_string())
            })
        })
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
        send_auth_done(&tx, p, result);
    });
}

fn send_auth_done(
    tx: &mpsc::UnboundedSender<AuthFlowEvent>,
    provider: String,
    result: Result<String, String>,
) {
    send_auth_event(tx, AuthFlowEvent::Done { provider, result });
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
    /// Load layered config after first paint (CLI TUI path). Tests / `connect`
    /// leave this false and pass a ready `config`.
    pub defer_config_load: bool,
    /// CLI `-P` was set; deferred config must not overwrite `provider`.
    pub provider_from_cli: bool,
    /// CLI `-m` was set; deferred config must not overwrite `model`.
    pub model_from_cli: bool,
    /// CLI `-a` was set; deferred config must not overwrite `agent_name`.
    pub agent_from_cli: bool,
    /// Background GitHub latest-release check. `None` skips the home popup.
    pub update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<UpdateOffer>>,
    /// Event-loop I/O injection. Production callers leave this default (real TTY).
    pub inject: LoopInject,
}

/// Test/harness injection for [`run`]. Production leaves every field default.
///
/// Headless tests fill `scripted_events` (TestBackend). Live-buffer tests set
/// `live_buf` and optionally `crossterm_events` so CrosstermBackend / poll
/// arms execute without a controlling terminal.
#[derive(Default)]
pub struct LoopInject {
    /// Scripted events → TestBackend (headless). `Some` even when empty.
    pub scripted_events: Option<VecDeque<Event>>,
    /// Memory-buffer CrosstermBackend (live path, no TTY).
    pub live_buf: bool,
    /// Events served by crossterm poll/read instead of the OS.
    pub crossterm_events: VecDeque<Event>,
    pub poll_err: bool,
    pub read_err: bool,
    pub draw_fail: bool,
    pub clear_fail: bool,
    /// Seed the catalog / suggestion / auth channels before the first poll.
    pub catalog: Option<(String, String, u32)>,
    pub suggest: Option<String>,
    pub auth: Option<AuthFlowEvent>,
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

/// Chrome for the first 80×24 home frame. Agent / Session / SQLite stay out
/// of this struct so `record_draw` is not waiting on them (issue #85).
struct TuiChrome {
    app: TuiApp,
    config: Config,
    missing_key: bool,
}

/// Agent + session after first paint. Built by [`prepare_tui_runtime`].
struct TuiRuntime {
    file_index: Arc<whycodes_index::WorkspaceIndex>,
    agent: Agent,
    session: Session,
    history: SessionHistory,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    perm_rx: mpsc::UnboundedReceiver<whycodes_agent::PermissionRequest>,
    question_rx: mpsc::UnboundedReceiver<QuestionRequest>,
    session_claims: whycodes_core::FileClaimRegistry,
}

/// Test helper: chrome then runtime (production `run` paints between them).
#[cfg(test)]
async fn prepare_tui_boot(opts: &TuiRunOptions) -> TuiRuntimeBoot {
    let mut chrome = prepare_tui_chrome(opts);
    let runtime = prepare_tui_runtime(opts, &mut chrome.app).await;
    TuiRuntimeBoot {
        app: chrome.app,
        missing_key: chrome.missing_key,
        agent: runtime.agent,
        session: runtime.session,
    }
}

/// Combined boot used by unit tests that still assert on agent + chrome.
#[cfg(test)]
struct TuiRuntimeBoot {
    app: TuiApp,
    missing_key: bool,
    agent: Agent,
    session: Session,
}

fn apply_resume(
    app: &mut TuiApp,
    session: &mut Session,
    system_prompt: &str,
    want: &str,
    auto_title: bool,
) {
    apply_resume_loaded(
        app,
        session,
        system_prompt,
        want,
        auto_title,
        try_load_session(want).map_err(|e| e.to_string()),
    );
}

fn apply_resume_loaded(
    app: &mut TuiApp,
    session: &mut Session,
    system_prompt: &str,
    want: &str,
    auto_title: bool,
    loaded: Result<Option<Session>, String>,
) {
    match loaded {
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
            app.toasts
                .push(crate::toast::ToastKind::Warning, resume_missing_toast(want));
        }
        Err(e) => {
            app.toasts.push(
                crate::toast::ToastKind::Error,
                format!("Resume failed: {e}"),
            );
        }
    }
}

fn resume_missing_toast(want: &str) -> String {
    if want == RESUME_LATEST {
        "No saved sessions to continue".into()
    } else {
        format!("Session not found: {want}")
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

fn prepare_tui_chrome(opts: &TuiRunOptions) -> TuiChrome {
    // Built-in theme only on the first frame. `from_core_config` walks
    // `~/.config/whycodes/themes/` — that belongs after `record_draw`.
    let tui_cfg = TuiAppConfig::default();
    let mut app = TuiApp::new(tui_cfg);

    app.provider_name = opts.provider.clone();
    app.model_name = opts.model.clone();
    app.reasoning_effort = opts.config.session.reasoning_effort.clone();
    app.approval_mode = opts.config.general.approval_mode.unwrap_or_default();
    app.agent_name = opts.agent_name.clone();
    // Do not canonicalize here: Windows NTFS + Defender on a temp dir is
    // tens of ms and the first-frame harness uses exactly that.
    app.project_dir = opts.project_dir.clone();
    app.project_label = app
        .project_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| whycodes_core::display_path(&app.project_dir));
    // `.git/HEAD` only. `git rev-parse` is deferred until after first paint
    // (empty harness dirs have no `.git`; spawning git was the Windows TTFF tax).
    app.refresh_git_branch_fast();
    // Catalog / API-key probe wait until after first paint (issue #85).
    let missing_key = opts.api_key.is_empty();
    app.status_message = format!(
        "agent={}  {}/{}  — Tab focus  Ctrl+T agent  Esc cancel  /help",
        opts.agent_name, opts.provider, opts.model
    );

    TuiChrome {
        app,
        config: Config::default(),
        missing_key,
    }
}

/// After first paint: load layered TOML and refresh chrome from it.
/// CLI `-P`/`-m`/`-a` already sit on `opts`; only fill blanks from config.
fn apply_deferred_config(
    opts: &mut TuiRunOptions,
    app: &mut TuiApp,
    config: &mut Config,
    provider: &mut String,
    model: &mut String,
) {
    let mut loaded = Config::load_layered(&opts.project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    loaded.general.project_path = Some(opts.project_dir.clone());
    if !opts.provider_from_cli
        && let Some(id) = loaded
            .default_model
            .as_ref()
            .map(|m| m.provider_id.clone())
            .filter(|id| !id.is_empty())
            .or_else(|| loaded.providers.keys().next().cloned())
    {
        *provider = id;
        opts.provider = provider.clone();
        app.provider_name = provider.clone();
    }
    if !opts.model_from_cli
        && let Some(id) = loaded
            .default_model
            .as_ref()
            .map(|m| m.model_id.clone())
            .filter(|id| !id.is_empty())
    {
        *model = id;
        opts.model = model.clone();
        app.model_name = model.clone();
    }
    if !opts.agent_from_cli && !loaded.default_agent.is_empty() {
        app.agent_name = loaded.default_agent.clone();
        opts.agent_name = app.agent_name.clone();
    }
    app.reasoning_effort = loaded.session.reasoning_effort.clone();
    app.approval_mode = loaded.general.approval_mode.unwrap_or_default();
    app.apply_context_window(
        provider,
        model,
        loaded.configured_context_window(provider, model),
        loaded.session.max_context_tokens as u64,
    );
    app.primary_agents = loaded
        .agents
        .iter()
        .filter(|a| a.mode == AgentMode::Primary || a.mode == AgentMode::All)
        .map(|a| a.name.clone())
        .collect();
    ensure_primary_agents(&mut app.primary_agents);
    if let Some(idx) = app.primary_agents.iter().position(|n| n == &app.agent_name) {
        app.agent_cycle_idx = idx;
    }
    app.model_selection.models = configured_models(&loaded);
    app.model_selection.selected = app
        .model_selection
        .models
        .iter()
        .position(|(p, m)| p == provider && m == model)
        .unwrap_or(0);
    let color_mode = app.config.color_mode;
    app.config = TuiAppConfig::from_core_config(&loaded.tui);
    app.config.color_mode = color_mode;
    app.config.extra.quantize_for(color_mode);
    *config = loaded.clone();
    opts.config = loaded;
}

async fn prepare_tui_runtime(opts: &TuiRunOptions, app: &mut TuiApp) -> TuiRuntime {
    let mut config = opts.config.clone();
    // Empty index until hydrate: `@` stays inert for frame 0 without
    // `canonicalize` + scan (issue #49). Started here so Agent can hold it.
    let file_index = whycodes_index::WorkspaceIndex::start(Vec::new());
    app.set_file_index(file_index.clone());

    let agent_info = config
        .get_agent(&opts.agent_name)
        .cloned()
        .unwrap_or_else(|| default_agent_info(&opts.agent_name));

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
    let question_prompter =
        question_prompter.with_notify(whycodes_agent::notify::handle_from_config(&config.notify));
    let question_prompter: Arc<ChannelQuestionPrompter> = Arc::new(question_prompter);

    config.general.project_path = Some(opts.project_dir.clone());
    let session_claims = whycodes_core::FileClaimRegistry::new();
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index.clone())
        .with_session_claims(session_claims.clone())
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
        )
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);
    agent.set_approval_mode(app.approval_mode);
    inject_test_llm(&mut agent, &opts.provider);

    let mut session = Session::new(opts.project_dir.clone(), system_prompt.clone());
    app.session_title = session.title.clone();
    app.session_id = session.id.clone();
    let history = SessionHistory::new();

    if let Some(ref want) = opts.resume_session_id {
        apply_resume(
            app,
            &mut session,
            &system_prompt,
            want,
            opts.config.session.auto_title,
        );
    }

    if let Some(ref rem) = opts.remote {
        apply_remote_hydrate(app, &mut session, rem).await;
    }

    TuiRuntime {
        file_index,
        agent,
        session,
        history,
        perm_prompter,
        question_prompter,
        perm_rx,
        question_rx,
        session_claims,
    }
}

/// True when a full-screen TUI can attach to a real terminal.
///
/// Prefer the controlling terminal (`/dev/tty` on Unix, `CONOUT$` on
/// Windows) so IDEs/wrappers that capture stdout (`stdout_tty=false`) still
/// get a normal TUI. Falls back to stdout when it is itself a TTY.
pub fn tui_available() -> bool {
    // stdout already a TTY: do not open CONOUT$ / `/dev/tty` just to probe.
    // `cmd_run` used to pay that extra handle before first paint (Windows).
    io::stdout().is_terminal() || open_controlling_console().is_some()
}

/// Concrete writer for ratatui/crossterm (`execute!` needs `Sized`).
enum TuiWriter {
    Console(std::fs::File),
    Stdout(io::Stdout),
    /// In-memory sink so `LoopTerm::live` can be unit-tested without a TTY.
    Buf(Vec<u8>),
    /// Always-failing sink so `Terminal::new` / alt-screen `execute!` error
    /// arms run without a real TTY.
    #[allow(dead_code)]
    Fail,
}

impl Write for TuiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Console(f) => f.write(buf),
            Self::Stdout(s) => s.write(buf),
            Self::Buf(b) => b.write(buf),
            Self::Fail => Err(io::Error::other("tui writer fail")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Console(f) => f.flush(),
            Self::Stdout(s) => s.flush(),
            Self::Buf(b) => b.flush(),
            Self::Fail => Err(io::Error::other("tui writer fail")),
        }
    }
}

struct LoopIo {
    scripted: Option<VecDeque<Event>>,
    crossterm: VecDeque<Event>,
    poll_err: bool,
    read_err: bool,
    force_zero_poll: bool,
}

impl LoopIo {
    fn from_inject(inject: &mut LoopInject) -> Self {
        let scripted = inject.scripted_events.take();
        let crossterm = std::mem::take(&mut inject.crossterm_events);
        Self {
            // Stub / live-buf / scripted runs never have a TTY. After the
            // injected queue drains, skip OS poll (ENOENT/EAGAIN on CI).
            force_zero_poll: inject.live_buf || scripted.is_some() || !crossterm.is_empty(),
            scripted,
            crossterm,
            poll_err: inject.poll_err,
            read_err: inject.read_err,
        }
    }

    fn is_headless(&self) -> bool {
        self.scripted.is_some()
    }

    fn crossterm_empty(&self) -> bool {
        self.crossterm.is_empty()
    }

    fn peek(&self) -> Option<&Event> {
        self.scripted.as_ref().and_then(|q| q.front())
    }

    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if let Some(q) = &self.scripted {
            return Ok(!q.is_empty());
        }
        self.poll_crossterm(timeout)
    }

    fn read_batch(&mut self) -> io::Result<Vec<Event>> {
        if let Some(q) = &mut self.scripted {
            // One event per poll so a scripted Enter can start a turn before
            // later keys (Esc, :q) are applied.
            match q.pop_front() {
                Some(ev) => Ok(vec![ev]),
                None => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "scripted TUI events exhausted",
                )),
            }
        } else {
            self.read_event_batch()
        }
    }

    fn poll_crossterm(&mut self, timeout: Duration) -> io::Result<bool> {
        if self.poll_err {
            self.poll_err = false;
            return Err(io::Error::other("crossterm stub poll failed"));
        }
        if !self.crossterm.is_empty() {
            return Ok(true);
        }
        if self.force_zero_poll {
            return Ok(false);
        }
        live_poll_crossterm(timeout)
    }

    fn read_crossterm(&mut self) -> io::Result<Event> {
        if self.read_err {
            self.read_err = false;
            return Err(io::Error::other("crossterm stub read failed"));
        }
        if let Some(ev) = self.crossterm.pop_front() {
            return Ok(ev);
        }
        if self.force_zero_poll {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "crossterm stub empty",
            ));
        }
        live_read_crossterm()
    }

    fn read_event_batch(&mut self) -> io::Result<Vec<Event>> {
        const MAX_BATCH: usize = 256;
        let mut batch = Vec::with_capacity(8);
        batch.push(self.read_crossterm()?);
        while batch.len() < MAX_BATCH {
            match self.poll_crossterm(Duration::ZERO) {
                Ok(true) => batch.push(self.read_crossterm()?),
                Ok(false) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(batch)
    }
}

enum LoopTerm {
    Live(Terminal<QuantizingBackend<CrosstermBackend<TuiWriter>>>),
    Headless(Terminal<QuantizingBackend<TestBackend>>),
}

impl LoopTerm {
    fn live(out: TuiWriter, color_mode: ColorMode) -> anyhow::Result<Self> {
        let backend = QuantizingBackend::with_size_fallback(
            CrosstermBackend::new(out),
            color_mode,
            ratatui::layout::Size {
                width: 80,
                height: 24,
            },
        );
        Ok(Self::Live(
            Terminal::new(backend).inspect_err(on_terminal_new_failed)?,
        ))
    }

    fn headless(color_mode: ColorMode) -> anyhow::Result<Self> {
        let backend = QuantizingBackend::new(TestBackend::new(80, 24), color_mode);
        Ok(Self::Headless(Terminal::new(backend)?))
    }

    fn resize(&mut self, area: Rect) {
        match self {
            Self::Live(t) => {
                if let Err(e) = t.resize(area) {
                    log_resize_failed("live", e);
                }
            }
            Self::Headless(t) => {
                if let Err(e) = t.resize(area) {
                    log_resize_failed("headless", e);
                }
            }
        }
    }

    fn clear(&mut self, fail: &mut bool) -> anyhow::Result<()> {
        if *fail {
            *fail = false;
            return Err(anyhow::anyhow!("tui clear failed"));
        }
        match self {
            Self::Live(t) => t.clear().map_err(Into::into),
            Self::Headless(t) => t.clear().map_err(Into::into),
        }
    }

    fn draw_app(
        &mut self,
        app: &mut TuiApp,
        fail: &mut bool,
    ) -> anyhow::Result<(ratatui::layout::Rect, Option<crate::cell_grid::CellGrid>)> {
        if *fail {
            *fail = false;
            return Err(anyhow::anyhow!("tui draw failed"));
        }
        match self {
            Self::Live(t) => draw_into(t, app),
            Self::Headless(t) => draw_into(t, app),
        }
    }

    fn restore(self, keyboard_enhanced: bool) {
        match self {
            Self::Live(mut terminal) => {
                restore_live_backend(terminal.backend_mut(), keyboard_enhanced);
                let _ = terminal.show_cursor();
            }
            Self::Headless(mut terminal) => {
                let _ = terminal.show_cursor();
            }
        }
    }
}

fn on_terminal_new_failed(e: &impl std::fmt::Display) {
    let _ = disable_raw_mode();
    whycodes_core::logging::emit(
        "whycodes_tui",
        "error",
        "tui.terminal_new_failed",
        Some(serde_json::json!({ "error": e.to_string() })),
    );
}

fn log_resize_failed(kind: &str, e: impl std::fmt::Display) {
    tracing::debug!(error = %e, "{kind} terminal resize failed");
}

fn draw_into<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp,
) -> anyhow::Result<(ratatui::layout::Rect, Option<crate::cell_grid::CellGrid>)>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let completed = terminal
        .draw(|f| render::render(f, app))
        .map_err(anyhow::Error::from)?;
    let area = completed.area;
    let cells = if app.mouse_sel.is_some() {
        Some(crate::cell_grid::CellGrid::from_buffer(completed.buffer))
    } else {
        None
    };
    Ok((area, cells))
}

/// Writer for alt-screen / draw / mouse: controlling console first, else stdout if TTY.
fn open_tui_writer() -> io::Result<TuiWriter> {
    choose_tui_writer(open_controlling_console(), io::stdout().is_terminal())
}

fn choose_tui_writer(console: Option<std::fs::File>, stdout_is_tty: bool) -> io::Result<TuiWriter> {
    if let Some(console) = console {
        return Ok(TuiWriter::Console(console));
    }
    if stdout_is_tty {
        return Ok(TuiWriter::Stdout(io::stdout()));
    }
    Err(io::Error::new(
        io::ErrorKind::NotConnected,
        "no interactive terminal (stdout is not a TTY and the controlling console is unavailable)",
    ))
}

/// Open flags for `/dev/tty` (Unix). Always compiled so tests can drive the
/// builder without a Unix TTY.
#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn unix_tty_open_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true);
    o
}

/// Open flags for `CONOUT$` (Windows). Always compiled so tests can drive the
/// builder without a live console.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn windows_console_open_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    o
}

/// Open `/dev/tty` with Unix flags. Always compiled so tests can drive the
/// builder + path without a Unix host.
#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn try_open_unix_tty() -> io::Result<std::fs::File> {
    unix_tty_open_options().open("/dev/tty")
}

/// Open `CONOUT$` with Windows flags. Always compiled so tests can drive it
/// without a live console.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn try_open_windows_console() -> io::Result<std::fs::File> {
    windows_console_open_options().open("CONOUT$")
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn open_unix_controlling_console() -> Option<std::fs::File> {
    console_open_result(try_open_unix_tty(), "open /dev/tty failed, trying stdout")
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn open_windows_controlling_console() -> Option<std::fs::File> {
    console_open_result(
        try_open_windows_console(),
        "open CONOUT$ failed, trying stdout",
    )
}

/// `/dev/tty` on Unix, `CONOUT$` on Windows. `None` if this process has no console.
fn open_controlling_console() -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        open_unix_controlling_console()
    }
    #[cfg(windows)]
    {
        open_windows_controlling_console()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

fn console_open_result(
    result: io::Result<std::fs::File>,
    msg: &'static str,
) -> Option<std::fs::File> {
    match result {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::debug!(error = %e, "{msg}");
            None
        }
    }
}

fn ensure_primary_agents(agents: &mut Vec<String>) {
    if agents.is_empty() {
        *agents = vec!["build".into(), "plan".into(), "ask".into()];
    }
}

fn default_agent_info(name: &str) -> whycodes_core::types::AgentInfo {
    whycodes_core::types::AgentInfo {
        name: name.to_string(),
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
    }
}

/// Leave alt-screen / raw mode from the panic hook (and from tests).
fn restore_terminal_on_panic() {
    restore_terminal_with(open_tui_writer)
}

fn restore_terminal_with(open: impl FnOnce() -> io::Result<TuiWriter>) {
    if let Ok(mut out) = open() {
        restore_terminal_on(&mut out);
    } else {
        let _ = disable_raw_mode();
    }
}

fn install_panic_terminal_restore() {
    whycodes_core::logging::set_panic_cleanup(restore_terminal_on_panic);
}

/// After [`IDLE_TRIM_AFTER`] of quiet idle, return retained heap pages.
fn maybe_idle_heap_trim(agent_busy: bool, idle_for: Duration, armed: &mut bool) {
    if !agent_busy && idle_for >= crate::heap::IDLE_TRIM_AFTER && *armed {
        crate::heap::release_retained_heap_debounced("client_idle", crate::heap::IDLE_TRIM_AFTER);
        *armed = false;
    }
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
fn enable_keyboard_enhancement(out: &mut impl Write, size: Option<(u16, u16)>) -> bool {
    if !should_query_keyboard_enhancement(
        std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty()),
        size,
    ) {
        return false;
    }
    push_keyboard_flags(out, keyboard_enhancement_supported())
}

fn keyboard_enhancement_supported() -> bool {
    matches!(supports_keyboard_enhancement(), Ok(true))
}

fn push_keyboard_flags(out: &mut impl Write, supported: bool) -> bool {
    if !supported {
        return false;
    }
    execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

fn attach_live(
    color_mode: ColorMode,
    open: impl FnOnce() -> io::Result<TuiWriter>,
    enable_raw: impl FnOnce() -> io::Result<()>,
    size: impl FnOnce() -> io::Result<(u16, u16)>,
) -> anyhow::Result<(LoopTerm, bool, u16, u16)> {
    let mut tui_out = open().map_err(|e| {
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
    enter_raw_and_alt(&mut tui_out, enable_raw)?;
    let (tw, th) = size().unwrap_or((0, 0));
    let keyboard_enhanced = enable_keyboard_enhancement(&mut tui_out, Some((tw, th)));
    let mut terminal = LoopTerm::live(tui_out, color_mode)?;
    if tw == 0 || th == 0 {
        terminal.resize(Rect::new(0, 0, 80, 24));
        whycodes_core::logging::emit(
            "whycodes_tui",
            "warn",
            "tui.size_fallback",
            Some(serde_json::json!({ "reported_w": tw, "reported_h": th, "using": "80x24" })),
        );
    }
    Ok((terminal, keyboard_enhanced, tw, th))
}

fn enter_raw_and_alt(
    out: &mut impl Write,
    enable_raw: impl FnOnce() -> io::Result<()>,
) -> anyhow::Result<()> {
    enable_raw().map_err(|e| {
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
    execute!(out, EnterAlternateScreen).map_err(|e| {
        let _ = disable_raw_mode();
        whycodes_core::logging::emit(
            "whycodes_tui",
            "error",
            "tui.alt_screen_failed",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        anyhow::anyhow!("failed to enter alternate screen ({e})")
    })?;
    Ok(())
}

fn enable_mouse_paste_cursor(out: &mut impl Write) {
    if let Err(e) = execute!(
        out,
        EnableMouseCapture,
        EnableBracketedPaste,
        SetCursorStyle::BlinkingBar
    ) {
        tracing::debug!(error = %e, "enable mouse/paste/cursor after first paint failed");
    }
}

fn restore_live_backend(out: &mut impl Write, keyboard_enhanced: bool) {
    let _ = disable_raw_mode();
    if keyboard_enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        SetCursorStyle::DefaultUserShape
    );
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

/// Sync wrapper so `whycodes run -d` can paint before clap's Tokio pool exists.
pub fn run_sync(opts: TuiRunOptions) -> anyhow::Result<TuiExit> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    rt.block_on(run(opts))
}

/// Run the full-screen TUI until the user quits.
pub async fn run(opts: TuiRunOptions) -> anyhow::Result<TuiExit> {
    let mut opts = opts;
    let mut loop_io = LoopIo::from_inject(&mut opts.inject);
    let headless = loop_io.is_headless();
    let live_buf = opts.inject.live_buf;
    let mut draw_fail = opts.inject.draw_fail;
    let mut clear_fail = opts.inject.clear_fail;
    let seed_catalog = opts.inject.catalog.take();
    let seed_suggest = opts.inject.suggest.take();
    let seed_auth = opts.inject.auth.take();

    // Unit tests drive CLI `cmd_run` / `cmd_connect` through this entry
    // without opening a terminal. `WHYCODES_TEST_TUI=upgrade` asks the CLI
    // to install after restore; anything else is a clean quit.
    // Headless scripted runs (this crate's tests) skip the stub so the loop
    // still executes against TestBackend.
    if !headless && let Ok(kind) = std::env::var("WHYCODES_TEST_TUI") {
        let _opts = opts;
        return Ok(if kind == "upgrade" {
            TuiExit::Upgrade
        } else {
            TuiExit::Quit
        });
    }

    // Wall clock for the Cline-style exit summary (process open → quit).
    let session_started = Instant::now();

    // Chrome only — Agent / Session / SQLite wait until after first paint (#85).
    let chrome = prepare_tui_chrome(&opts);
    let mut app = chrome.app;
    let mut config = chrome.config;
    let missing_key = chrome.missing_key;
    let remote = opts.remote.clone();

    let mut provider = opts.provider.clone();
    let mut model = opts.model.clone();
    let mut api_key = opts.api_key.clone();
    let max_turns = opts.max_turns;
    let project_dir = opts.project_dir.clone();

    // On panic, leave alt-screen / raw mode so the shell is usable and the
    // crash report (written by whycodes_core::logging) is readable.
    if !headless {
        install_panic_terminal_restore();
    }

    let bench_clock = std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty());
    if !bench_clock {
        whycodes_core::logging::emit(
            "whycodes_tui",
            "info",
            "tui.starting",
            Some(serde_json::json!({
                "provider": provider,
                "model": model,
                "stdout_tty": io::stdout().is_terminal(),
                "stdin_tty": io::stdin().is_terminal(),
                "headless": headless,
            })),
        );
    }

    let color_mode = detect_color_mode();
    set_active_color_mode(color_mode);
    app.config.color_mode = color_mode;
    app.config.extra.quantize_for(color_mode);

    // `live_buf` runs the production `!headless` arms (panic restore,
    // first-frame hydrate, crossterm poll) into a memory buffer.
    let (mut terminal, keyboard_enhanced, tw, th) = if headless && !live_buf {
        (LoopTerm::headless(color_mode)?, false, 80u16, 24u16)
    } else {
        attach_for_loop(color_mode, live_buf)?
    };

    if !bench_clock {
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
    }

    apply_boot_prompt(&mut app, missing_key, opts.initial_prompt.clone());

    // Inert unless WHYCODES_BENCH is set; see crate::bench.
    let bench = crate::bench::config_from_env();
    // Channels / mouse / paste wait until after first paint.

    // First paint before Agent / Session / SQLite (issue #85). `--idle-ms 0`
    // exits here and never pays ToolExecutor or the session DB.
    if app.pending_full_clears > 0 {
        if let Err(e) = terminal.clear(&mut clear_fail) {
            whycodes_core::logging::emit(
                "whycodes_tui",
                "warn",
                "tui.full_clear_failed",
                Some(serde_json::json!({ "error": e.to_string() })),
            );
        }
        app.pending_full_clears = app.pending_full_clears.saturating_sub(1);
    }
    let (draw_area, snapshot) = match terminal.draw_app(&mut app, &mut draw_fail) {
        Ok(v) => v,
        Err(e) => {
            whycodes_core::logging::emit(
                "whycodes_tui",
                "error",
                "tui.draw_failed",
                Some(serde_json::json!({ "error": e.to_string() })),
            );
            terminal.restore(keyboard_enhanced);
            if !headless {
                whycodes_core::logging::clear_panic_cleanup();
            }
            return Err(e);
        }
    };
    crate::bench::record_draw();
    if !bench_clock {
        whycodes_core::logging::emit(
            "whycodes_tui",
            "info",
            "tui.first_frame",
            Some(serde_json::json!({
                "w": draw_area.width,
                "h": draw_area.height,
            })),
        );
    }
    let _ = after_draw_frame(&mut app, snapshot, false, None);
    if let Some(ref b) = bench {
        // Idle harness (`--idle-ms N`) measures redraws on a settled home
        // screen. Hydrate (index / MCP / session DB) would dirty chrome and
        // inflate draws/s — skip it, wait out the window, then exit.
        while !crate::bench::should_stop(b) {
            std::thread::sleep(Duration::from_millis(50));
        }
        terminal.restore(keyboard_enhanced);
        if !headless {
            whycodes_core::logging::clear_panic_cleanup();
        }
        crate::bench::write_results(b);
        return Ok(TuiExit::Quit);
    }

    if let LoopTerm::Live(ref mut term) = terminal {
        enable_mouse_paste_cursor(term.backend_mut());
    }

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
    if let Some(win) = seed_catalog {
        seed_unbounded(&catalog_tx, win, "seed catalog");
    }
    if let Some(s) = seed_suggest {
        seed_unbounded(&suggest_tx, s, "seed suggestion");
    }
    if let Some(ev) = seed_auth {
        send_auth_event(&auth_tx, ev);
    }

    if opts.defer_config_load {
        apply_deferred_config(&mut opts, &mut app, &mut config, &mut provider, &mut model);
    }

    let runtime = prepare_tui_runtime(&opts, &mut app).await;
    let mut file_index = runtime.file_index;
    let mut agent = runtime.agent;
    let session = runtime.session;
    let history = runtime.history;
    let perm_prompter = runtime.perm_prompter;
    let question_prompter = runtime.question_prompter;
    let perm_rx = runtime.perm_rx;
    let question_rx = runtime.question_rx;
    let session_claims = runtime.session_claims;

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

    hydrate_after_first_frame(
        &mut app,
        &mut rt,
        &mut file_index,
        &mut api_key,
        &provider,
        &model,
        &mut config,
        &project_dir,
        false,
    )
    .await;
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
            maybe_offer_import(&mut app);
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
            if app.needs_redraw || animate {
                if app.pending_full_clears > 0 {
                    if let Err(e) = terminal.clear(&mut clear_fail) {
                        whycodes_core::logging::emit(
                            "whycodes_tui",
                            "warn",
                            "tui.full_clear_failed",
                            Some(serde_json::json!({ "error": e.to_string() })),
                        );
                    }
                    app.pending_full_clears = app.pending_full_clears.saturating_sub(1);
                }
                let (_draw_area, snapshot) = match terminal.draw_app(&mut app, &mut draw_fail) {
                    Ok(v) => v,
                    Err(e) => {
                        whycodes_core::logging::emit(
                            "whycodes_tui",
                            "error",
                            "tui.draw_failed",
                            Some(serde_json::json!({ "error": e.to_string() })),
                        );
                        return Err(e);
                    }
                };
                crate::bench::record_draw();
                // Cell snapshot is only for mouse text selection → clipboard.
                // Skip the ~4k String allocs/frame when nothing is selected.
                if after_draw_frame(&mut app, snapshot, animate, bench.as_ref()) {
                    break;
                }
            }

            if bench_should_break(bench.as_ref()) {
                break;
            }

            // ── Stream events from rt.agent (coalesce text/thinking deltas) ──
            after_turn_events_drain(&mut app, &mut rt.event_rx, &config, &file_index);

            if should_tick_spinner(&app, rt.agent_busy) {
                tick_spinner(&mut app, &mut spinner_frame);
            }

            // ── Permission / question requests (queued; one at a time) ─
            drain_prompter_queues(&mut app, &mut rt);

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
            maybe_force_stop_in_loop(
                &mut app,
                &mut rt,
                &mut cancel_requested_at,
                &config,
                &project_dir,
                &file_index,
            );

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
                apply_pending_agent(&mut app, &mut rt, &config, &project_dir, &name).await;
            }

            apply_pending_picker_choices(
                &mut app,
                &mut rt,
                &mut config,
                &mut provider,
                &mut model,
                &mut api_key,
                &mut catalog_fetch_pending,
                catalog_tx.clone(),
                &auth_tx,
            )
            .await;

            if app.pending_import && !rt.agent_busy {
                app.pending_import = false;
                apply_pending_import(
                    &mut app,
                    &mut rt.agent,
                    &mut config,
                    &project_dir,
                    &file_index,
                )
                .await;
            }

            // Deferred / idle catalog: never race the first (or any) user turn.
            if should_spawn_idle_catalog(
                catalog_fetch_pending,
                rt.agent_busy,
                app.pending_prompt.is_some(),
                missing_key,
            ) {
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
            apply_pending_cancel(
                &mut app,
                &mut rt,
                &mut cancel_requested_at,
                &config,
                &project_dir,
                &file_index,
            );

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
                    spawn_remote_turn(
                        &mut app,
                        &mut rt,
                        &mut cancel_requested_at,
                        rem.clone(),
                        &prompt,
                        &project_dir,
                    );
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

                spawn_local_turn(
                    &mut app,
                    &mut rt,
                    &mut cancel_requested_at,
                    &prompt,
                    &submit_images,
                    &project_dir,
                    &config,
                    &provider,
                    &model,
                    &api_key,
                    max_turns,
                    title_tx.clone(),
                );
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
            maybe_idle_heap_trim(
                rt.agent_busy,
                last_user_input.elapsed(),
                &mut idle_trim_armed,
            );

            let overlay_owns_keys = dialog_overlay_owns_keys(app.dialogs.active());
            // Headless: while a turn is in flight, hold non-cancel keys until a
            // permission/question overlay opens (so `y` is not typed into the prompt).
            if headless
                && rt.agent_busy
                && !overlay_owns_keys
                && !headless_busy_key_ready(loop_io.peek())
            {
                tokio::task::yield_now().await;
                continue;
            }
            let has_ev = match loop_io.poll(poll_for) {
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
            // Scripted tests drain the event queue, then wait for an in-flight
            // turn (or compact) to finish instead of aborting it on shutdown.
            // `#[tokio::test]` is current-thread: yield so spawned turns run.
            if headless {
                if headless_should_quit(has_ev, rt.agent_busy, rt.turn_join.is_some()) {
                    app.running = false;
                } else if rt.agent_busy || rt.turn_join.is_some() {
                    tokio::task::yield_now().await;
                }
            }
            // Live-buffer runs drive crossterm via `LoopInject`. When the
            // queue is empty the loop would wait forever (no TTY).
            if live_buf && !headless {
                if rt.agent_busy || rt.turn_join.is_some() {
                    tokio::task::yield_now().await;
                } else if live_buf_should_quit(has_ev, loop_io.crossterm_empty()) {
                    app.running = false;
                }
            }
            if has_ev {
                // Drain the whole pending queue before the next paint. A
                // trackpad flick is dozens of wheel events; handling one per
                // draw made the chat look frozen (each frame re-laid the
                // transcript). Moves alone do not force a redraw — hover
                // chrome still calls mark_dirty when the hit set changes.
                let batch = match loop_io.read_batch() {
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
                    terminal.resize(Rect::new(0, 0, *w, *h));
                }
                if batch.iter().any(event_forces_redraw) {
                    app.mark_dirty();
                }
                apply_batch_full_clears(&mut app, &batch);

                for ev in batch {
                    if apply_permission_overlay_event(&mut app, &mut rt, &ev) {
                        continue;
                    }
                    if apply_question_overlay_event(&mut app, &mut rt, &ev) {
                        continue;
                    }

                    // Ctrl+T / Ctrl+N / Ctrl+Page / Ctrl+O / Ctrl+Tab / slash Enter.
                    if let Event::Key(key) = &ev
                        && let Some(action) = idle_loop_key_action(
                            key,
                            app.mode,
                            rt.agent_busy,
                            !runtimes.is_empty(),
                            runtimes.len() + 1 >= MAX_LIVE_SESSIONS,
                        )
                        && apply_idle_loop_key(
                            action,
                            &mut app,
                            &mut rt,
                            &mut runtimes,
                            &mut mru,
                            &mut config,
                            &project_dir,
                            &file_index,
                            &session_claims,
                            &mut provider,
                            &mut model,
                            &mut api_key,
                            &auth_tx,
                        )
                        .await
                    {
                        continue;
                    }

                    // While busy: Esc cancels (draft preserved — Grok). Typing, scroll,
                    // and focus still work so the user can queue thoughts.
                    // Permission / question overlays own Esc/Enter — do not steal them.
                    let overlay_owns_keys = dialog_overlay_owns_keys(app.dialogs.active());
                    if rt.agent_busy
                        && !overlay_owns_keys
                        && let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press
                    {
                        apply_busy_key(
                            busy_key_action(key),
                            &ev,
                            &mut app,
                            &mut rt,
                            &mut cancel_requested_at,
                            &config,
                            &project_dir,
                            &file_index,
                        );
                        continue;
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
    terminal.restore(keyboard_enhanced);
    // Normal exit — panic hook no longer needs to touch the terminal.
    if !headless {
        whycodes_core::logging::clear_panic_cleanup();
    }

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

/// Real crossterm poll. Tests call this with `Duration::ZERO` so the
/// production line runs without a TTY wait.
fn production_term_size() -> io::Result<(u16, u16)> {
    term_size().or(Ok((0, 0)))
}

fn live_buf_open() -> io::Result<TuiWriter> {
    Ok(TuiWriter::Buf(Vec::new()))
}

fn live_buf_raw() -> io::Result<()> {
    Ok(())
}

fn live_buf_size() -> io::Result<(u16, u16)> {
    Ok((0, 0))
}

type OpenTui = fn() -> io::Result<TuiWriter>;
type EnableRaw = fn() -> io::Result<()>;
type TermSize = fn() -> io::Result<(u16, u16)>;

/// Live-buffer tests swap in memory writers; production uses the controlling
/// console, raw mode, and the real terminal size.
fn loop_attach_io(live_buf: bool) -> (OpenTui, EnableRaw, TermSize) {
    if live_buf {
        (live_buf_open, live_buf_raw, live_buf_size)
    } else {
        (open_tui_writer, enable_raw_mode, production_term_size)
    }
}

fn attach_for_loop(
    color_mode: ColorMode,
    live_buf: bool,
) -> anyhow::Result<(LoopTerm, bool, u16, u16)> {
    let (open, raw, size) = loop_attach_io(live_buf);
    attach_live(color_mode, open, raw, size)
}

/// Overlay keys that answer a permission prompt (allow / deny). Other keys
/// fall through to the normal dialog handler.
fn permission_overlay_reply(code: KeyCode) -> Option<bool> {
    match code {
        KeyCode::Char('y' | 'Y' | 'a' | 'A') | KeyCode::Enter => Some(true),
        KeyCode::Char('n' | 'N' | 'd' | 'D') | KeyCode::Esc => Some(false),
        _ => None,
    }
}

/// Permission overlay Y/N/A/D/Enter/Esc. Returns true when the event is consumed.
fn apply_permission_overlay_event(app: &mut TuiApp, rt: &mut SessionRuntime, ev: &Event) -> bool {
    if !matches!(app.dialogs.active(), Some(DialogKind::Permission { .. })) {
        return false;
    }
    let Event::Key(key) = ev else {
        return false;
    };
    if key.kind != KeyEventKind::Press {
        return false;
    }
    let Some(allow) = permission_overlay_reply(key.code) else {
        return false;
    };
    reply_permission(app, &mut rt.pending_perm_queue, allow);
    true
}

/// Question overlay keys (Press and Repeat). Returns true when consumed.
fn apply_question_overlay_event(app: &mut TuiApp, rt: &mut SessionRuntime, ev: &Event) -> bool {
    if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        return false;
    }
    let Event::Key(key) = ev else {
        return false;
    };
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return false;
    }
    let handled = handle_question_key(
        app,
        key.code,
        &mut rt.pending_question_queue,
        &rt.pending_perm_queue,
    );
    if handled {
        app.mark_dirty();
    }
    handled
}

fn live_poll_crossterm(timeout: Duration) -> io::Result<bool> {
    poll_crossterm_with(timeout, crossterm::event::poll)
}

/// Injected poll so tests can drive the live-poll wrapper without a TTY.
fn poll_crossterm_with(
    timeout: Duration,
    poll: impl FnOnce(Duration) -> io::Result<bool>,
) -> io::Result<bool> {
    poll(timeout)
}

/// Real crossterm read. Tests only call this after a zero-timeout poll
/// returns true (otherwise it would block on a missing TTY).
fn live_read_crossterm() -> io::Result<Event> {
    read_crossterm_with(crossterm::event::read)
}

fn read_crossterm_with(read: impl FnOnce() -> io::Result<Event>) -> io::Result<Event> {
    read()
}

/// Tests force a zero timeout so an empty stub cannot block on a missing TTY.
#[cfg(test)]
fn poll_timeout(requested: Duration, force_zero: bool) -> Duration {
    if force_zero {
        Duration::ZERO
    } else {
        requested
    }
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
/// The TUI draws on the controlling console; after restore, prefer that same
/// device so the lines land in the real terminal scrollback even when stdout
/// is captured. Fall back to stdout, then stderr.
fn print_session_summary(summary: &str) {
    if let Some(mut tty) = open_controlling_console()
        && writeln!(tty, "{summary}").is_ok()
        && tty.flush().is_ok()
    {
        return;
    }
    let mut out = io::stdout();
    if writeln!(out, "{summary}").is_ok() && out.flush().is_ok() {
        return;
    }
    let mut err = io::stderr();
    let _ = writeln!(err, "{summary}");
    let _ = err.flush();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleLoopKey {
    CycleAgent,
    NewSession,
    SessionLimit,
    CycleSession { next: bool },
    Dashboard,
    MruSwitch,
    SlashEnter,
}

fn idle_loop_key_action(
    key: &crossterm::event::KeyEvent,
    mode: AppMode,
    agent_busy: bool,
    has_parked: bool,
    at_session_limit: bool,
) -> Option<IdleLoopKey> {
    if key.kind != KeyEventKind::Press || mode != AppMode::Normal {
        return None;
    }
    let ctrl = key
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('t') if ctrl && !agent_busy => Some(IdleLoopKey::CycleAgent),
        KeyCode::Char('n') if ctrl => Some(if at_session_limit {
            IdleLoopKey::SessionLimit
        } else {
            IdleLoopKey::NewSession
        }),
        KeyCode::PageDown if ctrl && has_parked => Some(IdleLoopKey::CycleSession { next: true }),
        KeyCode::PageUp if ctrl && has_parked => Some(IdleLoopKey::CycleSession { next: false }),
        KeyCode::Char('o') if ctrl => Some(IdleLoopKey::Dashboard),
        KeyCode::Tab if ctrl && has_parked => Some(IdleLoopKey::MruSwitch),
        KeyCode::Enter if !agent_busy => Some(IdleLoopKey::SlashEnter),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyKey {
    Esc,
    Quit,
    CtrlC,
    WaitEnter,
    PassThrough,
}

fn busy_key_action(key: &crossterm::event::KeyEvent) -> BusyKey {
    match key.code {
        KeyCode::Esc => BusyKey::Esc,
        KeyCode::Char('q')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            BusyKey::Quit
        }
        KeyCode::Char('c')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            BusyKey::CtrlC
        }
        KeyCode::Enter => BusyKey::WaitEnter,
        _ => BusyKey::PassThrough,
    }
}

/// Idle Ctrl+T/N/Page/O/Tab and slash-Enter. Returns true when the event is consumed.
#[allow(clippy::too_many_arguments)]
async fn apply_idle_loop_key(
    action: IdleLoopKey,
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut Vec<SessionRuntime>,
    mru: &mut Vec<usize>,
    config: &mut Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
    session_claims: &whycodes_core::FileClaimRegistry,
    provider: &mut String,
    model: &mut String,
    api_key: &mut String,
    auth_tx: &mpsc::UnboundedSender<AuthFlowEvent>,
) -> bool {
    match action {
        IdleLoopKey::CycleAgent => {
            cycle_agent(
                app,
                &mut rt.agent,
                &mut rt.session,
                config,
                project_dir,
                Arc::clone(&rt.perm_prompter),
                Arc::clone(&rt.question_prompter),
                &rt.event_tx,
            )
            .await;
            true
        }
        IdleLoopKey::NewSession => {
            let fresh = spawn_new_session_runtime(
                &app.agent_name,
                config,
                project_dir,
                file_index,
                session_claims.clone(),
            )
            .await;
            adopt_fresh_runtime(app, rt, runtimes, mru, fresh);
            true
        }
        IdleLoopKey::SessionLimit => {
            warn_session_limit(app);
            true
        }
        IdleLoopKey::CycleSession { next } => {
            cycle_live_session(app, rt, runtimes, mru, next);
            true
        }
        IdleLoopKey::Dashboard => {
            open_sessions_dashboard(app, rt, runtimes);
            true
        }
        IdleLoopKey::MruSwitch => {
            switch_mru_session(app, rt, runtimes, mru);
            true
        }
        IdleLoopKey::SlashEnter => {
            if let Some(text) = slash_command_from_prompt(app) {
                consume_slash_draft(app);
                handle_slash(
                    &text,
                    &mut SlashContext {
                        app,
                        session: &mut rt.session,
                        history: &mut rt.history,
                        agent: &mut rt.agent,
                        config,
                        project_dir,
                        provider,
                        model,
                        api_key,
                        perm_prompter: Arc::clone(&rt.perm_prompter),
                        question_prompter: Arc::clone(&rt.question_prompter),
                        auth_tx: auth_tx.clone(),
                        pending_compact: &mut rt.pending_compact,
                    },
                )
                .await;
                true
            } else {
                false
            }
        }
    }
}

/// Busy Esc / Ctrl+Q / Ctrl+C / Enter / passthrough. Always consumes the key.
#[allow(clippy::too_many_arguments)]
fn apply_busy_key(
    action: BusyKey,
    ev: &Event,
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    match action {
        BusyKey::Esc => {
            // First Esc: cooperative cancel. Second: force-stop now.
            if cancel_requested_at.is_some() {
                force_stop_turn(
                    app,
                    rt,
                    cancel_requested_at,
                    config,
                    project_dir,
                    file_index,
                );
            } else {
                begin_cancel(
                    app,
                    &rt.cancel_flag,
                    cancel_requested_at,
                    &mut rt.pending_question_queue,
                    &mut rt.pending_perm_queue,
                );
            }
            app.esc_armed_at = None;
        }
        BusyKey::Quit => {
            // Quit: always force-stop so we never hang on exit.
            if rt.agent_busy {
                force_stop_turn(
                    app,
                    rt,
                    cancel_requested_at,
                    config,
                    project_dir,
                    file_index,
                );
            }
            app.running = false;
        }
        BusyKey::CtrlC => match busy_ctrl_c(app, *cancel_requested_at) {
            BusyCtrlC::ClearedDraft => {}
            BusyCtrlC::BeginCancel => begin_cancel(
                app,
                &rt.cancel_flag,
                cancel_requested_at,
                &mut rt.pending_question_queue,
                &mut rt.pending_perm_queue,
            ),
            BusyCtrlC::ForceStop => force_stop_turn(
                app,
                rt,
                cancel_requested_at,
                config,
                project_dir,
                file_index,
            ),
        },
        BusyKey::WaitEnter => {
            toast_wait_for_turn(app);
        }
        BusyKey::PassThrough => {
            // Typing, scroll, focus toggle — all allowed mid-turn.
            let _ = input::handle_event(app, ev.clone());
        }
    }
}

fn toast_wait_for_turn(app: &mut TuiApp) {
    app.toasts.push(
        crate::toast::ToastKind::Info,
        "Wait for turn or Esc to cancel",
    );
}

fn dialog_overlay_owns_keys(active: Option<&DialogKind>) -> bool {
    matches!(
        active,
        Some(DialogKind::Permission { .. } | DialogKind::Question(_))
    )
}

fn headless_should_quit(has_ev: bool, agent_busy: bool, turn_join: bool) -> bool {
    !has_ev && !agent_busy && !turn_join
}

fn live_buf_should_quit(has_ev: bool, crossterm_empty: bool) -> bool {
    !has_ev && crossterm_empty
}

/// Idle catalog: only after a deferred fetch, with no in-flight turn or prompt.
fn should_spawn_idle_catalog(
    catalog_fetch_pending: bool,
    agent_busy: bool,
    pending_prompt: bool,
    missing_key: bool,
) -> bool {
    catalog_fetch_pending && !agent_busy && !pending_prompt && !missing_key
}

/// Queue a catalog fetch when a turn is in flight; otherwise spawn it now.
fn defer_or_spawn_catalog(
    agent_busy: bool,
    catalog_fetch_pending: &mut bool,
    config: &Config,
    provider: &str,
    model: &str,
    api_key: &str,
    catalog_tx: mpsc::UnboundedSender<(String, String, u32)>,
) {
    if agent_busy {
        *catalog_fetch_pending = true;
        return;
    }
    *catalog_fetch_pending = false;
    spawn_model_context_fetch(config, provider, model, api_key, catalog_tx);
}

/// Agent picker selection: refuse while a turn is in flight, otherwise switch.
async fn apply_pending_agent(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    config: &Config,
    project_dir: &std::path::Path,
    name: &str,
) {
    if rt.agent_busy {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            "Can't switch agent while a turn is running",
        );
        return;
    }
    switch_to_agent(
        app,
        &mut rt.agent,
        &mut rt.session,
        config,
        project_dir,
        Arc::clone(&rt.perm_prompter),
        Arc::clone(&rt.question_prompter),
        &rt.event_tx,
        name,
        false,
    )
    .await;
}

/// Arm cooperative cancel: set the flag, unblock permission/question waits,
/// and start the force-stop timer.
/// Mouse `[stop]` / UI cancel: first click begins cooperative cancel, a
/// second click while already cancelling force-stops the turn.
fn apply_pending_cancel(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    if !rt.agent_busy || !app.pending_cancel {
        return;
    }
    app.pending_cancel = false;
    if cancel_requested_at.is_some() {
        force_stop_turn(
            app,
            rt,
            cancel_requested_at,
            config,
            project_dir,
            file_index,
        );
    } else {
        begin_cancel(
            app,
            &rt.cancel_flag,
            cancel_requested_at,
            &mut rt.pending_question_queue,
            &mut rt.pending_perm_queue,
        );
    }
}

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
    let question_prompter =
        question_prompter.with_notify(whycodes_agent::notify::handle_from_config(&config.notify));
    let question_prompter: Arc<ChannelQuestionPrompter> = Arc::new(question_prompter);

    let mut agent = Agent::new(agent_info)
        .with_config(config)
        .with_file_index(file_index.clone())
        .with_session_claims(session_claims)
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
        )
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);
    agent.set_approval_mode(config.general.approval_mode.unwrap_or_default());
    agent = agent.with_mcp(config).await;

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

/// Apply model / effort / approval / login / catalog flags set by dialogs.
#[allow(clippy::too_many_arguments)]
async fn apply_pending_picker_choices(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    config: &mut Config,
    provider: &mut String,
    model: &mut String,
    api_key: &mut String,
    catalog_fetch_pending: &mut bool,
    catalog_tx: mpsc::UnboundedSender<(String, String, u32)>,
    auth_tx: &mpsc::UnboundedSender<AuthFlowEvent>,
) {
    if let Some((p, m)) = app.pending_model.take() {
        apply_model_choice(app, provider, model, api_key, p, m, config);
        fill_oauth_credential(api_key, provider).await;
        defer_or_spawn_catalog(
            rt.agent_busy,
            catalog_fetch_pending,
            config,
            provider,
            model,
            api_key,
            catalog_tx.clone(),
        );
    }
    if let Some(effort) = app.pending_effort.take() {
        apply_reasoning_effort(app, &mut rt.agent, config, &effort);
    }
    if let Some(mode) = app.pending_approval_mode.take() {
        apply_approval_mode(app, &mut rt.agent, config, mode);
    }
    if let Some(p) = app.pending_login_provider.take()
        && let Ok(dir) = Config::data_dir()
    {
        spawn_oauth_login(app, auth_tx, dir, &p);
    }
    if app.pending_catalog_refresh {
        app.pending_catalog_refresh = false;
        app.clear_api_context_window();
        defer_or_spawn_catalog(
            rt.agent_busy,
            catalog_fetch_pending,
            config,
            provider,
            model,
            api_key,
            catalog_tx,
        );
    }
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

/// In-loop hard stop: abort a wedged turn after [`CANCEL_FORCE_AFTER`] or a
/// second `[stop]` while already cancelling.
fn maybe_force_stop_in_loop(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    if !should_force_stop(rt.agent_busy, *cancel_requested_at, app.pending_cancel) {
        return;
    }
    app.pending_cancel = false;
    force_stop_turn(
        app,
        rt,
        cancel_requested_at,
        config,
        project_dir,
        file_index,
    );
}

/// Local in-process agent turn. Records the user message, routes the model,
/// and delivers [`TurnOutcome::Ok`] / [`TurnOutcome::Err`].
#[allow(clippy::too_many_arguments)]
fn spawn_local_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    prompt: &str,
    submit_images: &[crate::images::PromptImage],
    project_dir: &std::path::Path,
    config: &Config,
    provider: &str,
    model: &str,
    api_key: &str,
    max_turns: Option<usize>,
    title_tx: mpsc::UnboundedSender<(String, String)>,
) {
    let flag = arm_generating(app, rt, cancel_requested_at, "");
    let expanded = record_user_turn(app, rt, prompt, project_dir, config, submit_images);
    let (route_provider, route_model) = route_turn_model(
        rt.session.id.as_str(),
        provider,
        model,
        &expanded,
        rt.agent
            .model_fast()
            .or(config.session.model_fast.as_deref()),
    );

    let provider2 = route_provider;
    let model2 = route_model;
    let api_key2 = api_key.to_string();
    let event_tx2 = rt.event_tx.clone();
    let done_tx2 = rt.done_tx.clone();
    let cancel2 = Some(flag);
    let auto_title = config.session.auto_title;
    let title_model = config.session.title_model.clone();
    let title_tx2 = title_tx;
    let title_provider = provider.to_string();
    let title_session_model = model.to_string();

    let (ag, sess) = take_turn_owner(rt, project_dir);

    rt.turn_join = Some(tokio::spawn(async move {
        let agent = ag;
        let mut session = sess;
        // Time only the agent loop. Title refine runs async *after*
        // we release rt.agent_busy so the user can type immediately.
        let work_t0 = Instant::now();
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

/// Remote `whycodes serve` turn: stream over HTTP, deliver [`TurnOutcome::Remote`].
fn spawn_remote_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    rem: crate::remote::RemoteAttach,
    prompt: &str,
    project_dir: &std::path::Path,
) {
    let expanded = expand_at_files(prompt, project_dir);
    rt.session.add_user_message(&expanded);
    let flag = arm_generating(app, rt, cancel_requested_at, "remote…");
    let event_tx2 = rt.event_tx.clone();
    let done_tx2 = rt.done_tx.clone();
    rt.turn_join = Some(tokio::spawn(async move {
        let t0 = Instant::now();
        let result = crate::remote::stream_chat(&rem, &expanded, event_tx2, Some(flag)).await;
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

/// Move newly arrived prompter requests onto the runtime queues, then open one overlay.
fn drain_prompter_queues(app: &mut TuiApp, rt: &mut SessionRuntime) {
    while let Ok(req) = rt.perm_rx.try_recv() {
        rt.pending_perm_queue.push_back(req);
    }
    while let Ok(req) = rt.question_rx.try_recv() {
        rt.pending_question_queue.push_back(req);
    }
    maybe_open_queued_dialog(app, rt);
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
            app.status_message = format!("tool: {}", shown_tool_name(&name));
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
            apply_background_event(app, &id, &status, &summary);
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

/// Paste / focus / resize echo leftover glyphs onto the PTY; clear twice.
fn apply_batch_full_clears(app: &mut TuiApp, batch: &[Event]) {
    if batch
        .iter()
        .any(crate::redraw_schedule::event_needs_full_clear)
    {
        // Two frames: some emulators echo the paste *after*
        // Event::Paste, so one clear is overwritten by the ghost.
        app.request_full_clear(2);
    }
    if crate::redraw_schedule::batch_looks_like_unbracketed_paste(batch) {
        app.request_full_clear(2);
    }
}

/// Store the mouse-selection cell snapshot, or drop it when nothing is selected.
fn apply_draw_snapshot(app: &mut TuiApp, snapshot: Option<crate::cell_grid::CellGrid>) {
    if let Some(cells) = snapshot {
        app.screen_cells = cells;
    } else if !app.screen_cells.is_empty() {
        app.screen_cells.clear();
    }
}

/// True once a `WHYCODES_BENCH` run has drawn its first frame and outstayed its duration.
fn bench_should_break(bench: Option<&crate::bench::BenchConfig>) -> bool {
    matches!(bench, Some(b) if crate::bench::should_stop(b))
}

/// Snapshot + deferred heap trim + dirty flag. Returns true when a bench run should exit.
fn after_draw_frame(
    app: &mut TuiApp,
    snapshot: Option<crate::cell_grid::CellGrid>,
    animate: bool,
    bench: Option<&crate::bench::BenchConfig>,
) -> bool {
    apply_draw_snapshot(app, snapshot);
    crate::heap::run_deferred_release();
    app.needs_redraw = animate || app.pending_full_clears > 0;
    bench_should_break(bench)
}

fn refresh_sidebar_if_visible(
    app: &mut TuiApp,
    config: &whycodes_config::Config,
    file_index: &std::sync::Arc<whycodes_index::WorkspaceIndex>,
) {
    if app.sidebar.visible {
        refresh_sidebar(app, config, file_index);
    }
}

/// Drain agent stream events; refresh the sidebar only when it is on screen.
fn after_turn_events_drain(
    app: &mut TuiApp,
    event_rx: &mut mpsc::UnboundedReceiver<TurnEvent>,
    config: &whycodes_config::Config,
    file_index: &std::sync::Arc<whycodes_index::WorkspaceIndex>,
) -> bool {
    if drain_turn_events(app, event_rx) {
        refresh_sidebar_if_visible(app, config, file_index);
        app.mark_dirty();
        true
    } else {
        false
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

/// Paint, then hydrate. Deferred boot work the first 80×24 home frame does
/// not need (issue #49).
#[allow(clippy::too_many_arguments)]
async fn hydrate_after_first_frame(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    file_index: &mut Arc<whycodes_index::WorkspaceIndex>,
    api_key: &mut String,
    provider: &str,
    model: &str,
    config: &mut Config,
    project_dir: &std::path::Path,
    animate: bool,
) {
    let hydrate_before = capture_first_frame_hydrate_chrome(app);
    app.config.theme.apply_syntax_theme();
    if app.project_dir.as_os_str() != project_dir.as_os_str() {
        app.project_dir = project_dir.to_path_buf();
    }
    if let Ok(canon) = project_dir.canonicalize() {
        app.project_dir = canon;
        app.project_label = app
            .project_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| whycodes_core::display_path(&app.project_dir));
    }
    config.load_command_files(project_dir);
    app.refresh_git_branch();
    hydrate_auth_plugins(project_dir);
    let real = start_workspace_file_index(project_dir);
    app.set_file_index(real.clone());
    rt.agent.set_file_index(real.clone());
    *file_index = real;
    rt.agent.hydrate_plugins(Some(project_dir));
    hydrate_full_system_prompt(rt, project_dir, config);
    hydrate_session_picker(app);
    hydrate_deferred_api_key(api_key, provider, model, config, app);
    rt.agent.load_mcp(config).await;
    maybe_session_auto_index(project_dir, config, app);
    refresh_sidebar(app, config, file_index);
    load_app_todos(app);
    settle_first_frame_hydrate(app, &hydrate_before, animate);
}

fn hydrate_auth_plugins(project_dir: &std::path::Path) -> usize {
    let mut dirs = Vec::new();
    if let Ok(p) = whycodes_config::Config::default_path()
        && let Some(parent) = p.parent()
    {
        dirs.push(parent.join("plugins"));
    }
    dirs.push(whycodes_core::project_dir(project_dir).join("plugins"));
    let loaded = whycodes_auth::plugin::load_from_dirs(&dirs);
    if loaded > 0 {
        tracing::debug!(count = loaded, "hydrated auth plugins after first frame");
    }
    loaded
}

fn start_workspace_file_index(
    project_dir: &std::path::Path,
) -> Arc<whycodes_index::WorkspaceIndex> {
    whycodes_index::WorkspaceIndex::start(whycodes_index::WorkspaceIndex::project_roots(
        project_dir,
    ))
}

fn hydrate_full_system_prompt(
    rt: &mut SessionRuntime,
    project_dir: &std::path::Path,
    config: &Config,
) {
    let base = rt.agent.system_prompt();
    let with_agents = whycodes_agent::agent::Agent::with_agents_md(&base, project_dir);
    let full = with_project_memory(&with_agents, project_dir, config, None);
    rt.session.set_system_prompt(&full);
}

fn hydrate_session_picker(app: &mut TuiApp) {
    if app.session_list.sessions.is_empty() {
        let entries = load_session_entries();
        if !entries.is_empty() {
            app.session_list.sessions = entries;
        }
    }
}

fn hydrate_deferred_api_key(
    api_key: &mut String,
    provider: &str,
    model: &str,
    config: &Config,
    app: &mut TuiApp,
) {
    if !api_key.is_empty() {
        return;
    }
    let env_var = format!("{}_API_KEY", provider.to_uppercase());
    let mut fetched: Option<String> = None;
    if let Ok(v) = std::env::var(&env_var)
        && !v.is_empty()
    {
        fetched = Some(v);
    }
    if fetched.is_none()
        && let Some(pc) = config.get_provider(provider)
        && let Some(k) = &pc.api_key
        && !k.is_empty()
    {
        fetched = Some(k.clone());
    }
    if let Some(k) = fetched {
        *api_key = k;
        app.status_message = format!(
            "agent={}  {}/{}  — Tab focus  Ctrl+T agent  Esc cancel  /help",
            app.agent_name, provider, model
        );
    }
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

fn shown_tool_name(name: &str) -> &str {
    match name {
        "bash" | "shell" | "run_terminal_command" => "run",
        "read_file" => "read",
        "search_code" | "rg" => "grep",
        other => other,
    }
}

fn apply_background_event(app: &mut TuiApp, id: &str, status: &str, summary: &str) {
    match status {
        "running" => {
            app.bg_running_count = app.bg_running_count.saturating_add(1);
            app.upsert_bg_job(id, "running", summary);
            app.status_message = format!("bg {id} started");
            app.toasts.push(
                crate::toast::ToastKind::Info,
                truncate_toast(&format!("bg {id}: {summary}"), 56),
            );
        }
        "done" => {
            app.bg_running_count = app.bg_running_count.saturating_sub(1);
            app.upsert_bg_job(id, "done", summary);
            app.toasts.push(
                crate::toast::ToastKind::Success,
                truncate_toast(&format!("bg {id} done · {summary}"), 56),
            );
        }
        "failed" => {
            app.bg_running_count = app.bg_running_count.saturating_sub(1);
            app.upsert_bg_job(id, "failed", summary);
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                truncate_toast(&format!("bg {id} failed · {summary}"), 64),
            );
        }
        "killed" => {
            app.bg_running_count = app.bg_running_count.saturating_sub(1);
            app.upsert_bg_job(id, "killed", summary);
            app.toasts.push(
                crate::toast::ToastKind::Info,
                truncate_toast(&format!("bg {id} killed"), 40),
            );
        }
        _ => {
            app.upsert_bg_job(id, status, summary);
            app.status_message = format!("bg {id} {status}");
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
    // Opt-out for debugging hang/crash suspicions: WHYCODES_NO_MODEL_CATALOG=1
    if skip_model_catalog() {
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

fn skip_model_catalog() -> bool {
    std::env::var_os("WHYCODES_NO_MODEL_CATALOG").is_some()
}
