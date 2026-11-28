//! TUI event loop — streaming agent + permission dialogs.

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::color::{ColorMode, QuantizingBackend, detect_color_mode, set_active_color_mode};
use crossterm::cursor::{Hide, SetCursorStyle};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    SetTitle, disable_raw_mode, enable_raw_mode, size as term_size, supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use whycodes_agent::agent::Agent;
use whycodes_agent::permission::ChannelPermissionPrompter;
use whycodes_agent::steer::SteerMode;
use whycodes_agent::{
    CancelFlag, ChannelQuestionPrompter, PermissionReply, QuestionError, QuestionPrompter,
    QuestionRequest, TurnEvent, TurnOpts, new_cancel_flag, request_cancel,
};
use whycodes_config::Config;
use whycodes_core::types::{AgentMode, ApprovalMode};
use whycodes_session::session::Session;
use whycodes_session::{SessionHistory, TreeTarget};

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

mod boot;
mod goal_run;
mod import;
mod keys;
mod mcp_defer;
mod models;
mod persist;
mod prompts;
mod sessions;
mod slash;
#[cfg(test)]
mod submit_probe_tests;
mod terminal;
#[cfg(test)]
mod tests;
mod turn;
mod windows_console;

use boot::*;
pub(crate) use goal_run::GoalCheck;
use goal_run::*;
pub(crate) use import::mark_import_declined;
use import::*;
use keys::*;
use mcp_defer::PendingMcp;
use models::*;
pub use persist::resolve_and_load_session;
use persist::*;
use prompts::*;
use sessions::*;
pub use slash::SlashContext;
use slash::*;
use terminal::*;
use turn::*;

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
            // Headless tests wait for the in-flight turn before quitting, and
            // they hold ENV_LOCK, so this runs once per busy test, in series
            // (30 s each made the tui suite ~4 min). 3 s still outlasts the
            // queued keys and CANCEL_FORCE_AFTER (1.2 s).
            whycodes_llm::ScriptedStep::Hang(std::time::Duration::from_secs(3))
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
    /// Headless-only cap. Interactive TUI passes `None` (no default turn limit).
    pub max_turns: Option<usize>,
    pub initial_prompt: Option<String>,
    pub config: Config,
    /// When set, load this session (or the latest if `"__latest__"`) before first paint.
    ///
    /// Used by CLI `--continue` / `--resume <id>`. In-session resume uses
    /// `pending_session_id` on the app instead.
    pub resume_session_id: Option<String>,
    /// CLI `--fork`: continue a resumed session in a copy with a new id.
    pub fork_session: bool,
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
    /// CLI `--approval-mode`: wins over the deferred config's
    /// `[general] approval_mode` (and its env overlay) for this run.
    pub approval_mode_from_cli: Option<ApprovalMode>,
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

/// Combined boot used by unit tests that still assert on agent + chrome.
#[cfg(test)]
struct TuiRuntimeBoot {
    app: TuiApp,
    missing_key: bool,
    agent: Agent,
    session: Session,
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

/// Holds a full truecolor 200×60 frame (12k cells × ~20 B) without a mid-frame flush.
const CONSOLE_WRITE_BUFFER: usize = 256 * 1024;

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

/// After [`IDLE_TRIM_AFTER`] of quiet idle, return retained heap pages; then
/// again every [`IDLE_RETRIM_EVERY`] while the client stays idle.
///
/// [`IDLE_TRIM_AFTER`]: crate::heap::IDLE_TRIM_AFTER
/// [`IDLE_RETRIM_EVERY`]: crate::heap::IDLE_RETRIM_EVERY
fn maybe_idle_heap_trim(agent_busy: bool, idle_for: Duration, armed: &mut bool) {
    if agent_busy || idle_for < crate::heap::IDLE_TRIM_AFTER {
        return;
    }
    if *armed {
        crate::heap::release_retained_heap_debounced("client_idle", crate::heap::IDLE_TRIM_AFTER);
        *armed = false;
    } else {
        crate::heap::release_retained_heap_debounced(
            "client_idle_retrim",
            crate::heap::IDLE_RETRIM_EVERY,
        );
    }
}

/// One-time status-line notice when `[tui.key_bindings]` conflicts with
/// this terminal (debounced by a hash of the conflict set in `dir`).
fn key_conflict_notice(app: &mut TuiApp, dir: &std::path::Path) {
    let env = crate::terminal_setup::TerminalEnv::detect(app.keyboard_enhanced);
    let issues = crate::key_audit::configured_issues(&app.config.key_bindings, &env);
    if let Some(notice) = crate::key_audit::startup_notice(&issues, dir) {
        app.status_message = notice;
        app.mark_dirty();
    }
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
///
/// Test/helper sync wrapper. Production `run -d` uses a 2-worker pool in the CLI
/// so `event::poll` cannot starve turns on Linux, macOS, or Windows.
pub fn run_sync(opts: TuiRunOptions) -> anyhow::Result<TuiExit> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    rt.block_on(run(opts))
}

/// Alt-screen CSI + one line. No Agent, Config, Tokio, or ratatui.
///
/// Returns `Some(Quit)` when `WHYCODES_BENCH` is set (harness done). `None`
/// means the caller should keep going with the full TUI.
pub fn paint_first_frame_sync() -> anyhow::Result<Option<TuiExit>> {
    let bench = crate::bench::config_from_env();
    let mut out = open_tui_writer().map_err(|e| {
        anyhow::anyhow!(
            "failed to open terminal for TUI ({e}). \
             Run inside a real terminal, or use `whycodes --plain`."
        )
    })?;
    windows_console::prepare_windows_console(&out);
    if let Err(e) = write_splash_csi(&mut out) {
        restore_splash_csi(&mut out);
        return Err(e.into());
    }
    crate::bench::record_draw();
    if let Some(ref b) = bench {
        while !crate::bench::should_stop(b) {
            std::thread::sleep(Duration::from_millis(50));
        }
        restore_splash_csi(&mut out);
        crate::bench::write_results(b);
        return Ok(Some(TuiExit::Quit));
    }
    restore_splash_csi(&mut out);
    Ok(None)
}

/// Run the full-screen TUI until the user quits.
pub async fn run(opts: TuiRunOptions) -> anyhow::Result<TuiExit> {
    let mut opts = opts;
    let mut loop_io = LoopIo::from_inject(&mut opts.inject);
    let headless = loop_io.is_headless();
    let live_buf = opts.inject.live_buf;
    let mut draw_fail = opts.inject.draw_fail;
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

    // Wall clock for the exit summary (process open → quit).
    let session_started = Instant::now();

    if !headless {
        install_panic_terminal_restore();
    }

    let color_mode = detect_color_mode();
    set_active_color_mode(color_mode);
    // Attach before TuiApp / Config. Interactive first paint is the themed
    // home — a black splash then home is two full-screen dumps (Windows
    // flicker). `WHYCODES_BENCH` still uses splash so TTFF does not wait
    // on chrome.
    let (mut terminal, keyboard_enhanced, tw, th) = if headless && !live_buf {
        (LoopTerm::headless(color_mode)?, false, 80u16, 24u16)
    } else {
        attach_for_loop(color_mode, live_buf)?
    };

    let restore_attach = |terminal: LoopTerm| {
        terminal.restore(keyboard_enhanced);
        if !headless {
            whycodes_core::logging::clear_panic_cleanup();
        }
    };

    if draw_fail {
        restore_attach(terminal);
        return Err(anyhow::anyhow!("tui draw failed"));
    }
    let bench_clock = std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty());
    let bench = crate::bench::config_from_env();
    if let Some(ref b) = bench {
        if let Err(e) = terminal.draw_splash() {
            restore_attach(terminal);
            return Err(e);
        }
        crate::bench::record_draw();
        while !crate::bench::should_stop(b) {
            std::thread::sleep(Duration::from_millis(50));
        }
        restore_attach(terminal);
        crate::bench::write_results(b);
        return Ok(TuiExit::Quit);
    }

    let chrome = prepare_tui_chrome(&opts);
    let mut app = chrome.app;
    let mut config = chrome.config;
    let missing_key = chrome.missing_key;
    let remote = opts.remote.clone();
    app.attached = remote.is_some();
    let mut provider = opts.provider.clone();
    let mut model = opts.model.clone();
    let mut api_key = opts.api_key.clone();
    let max_turns = opts.max_turns;
    let project_dir = opts.project_dir.clone();
    app.config.color_mode = color_mode;
    app.config.extra.quantize_for(color_mode);
    apply_boot_prompt(&mut app, missing_key, opts.initial_prompt.clone());

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
    // Loopback model-server probe, spawned only when the provider dialog opens.
    let (discover_tx, mut discover_rx) =
        mpsc::unbounded_channel::<Vec<whycodes_llm::LocalServer>>();
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
    // The session that timer belongs to: it is reset on a switch, or an Esc
    // in one session would force-stop the turn of the next.
    let mut cancel_owner = app.session_id.clone();
    let mut spinner_frame: usize = 0;
    let mut last_spinner_tick = Instant::now();
    // Title may arrive before TurnOutcome restores the real rt.session; hold it.
    let mut pending_async_title: Option<(String, String)> = None;
    let mut update_rx = opts.update_rx;

    let mut mcp = hydrate_after_first_frame(
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

    if let LoopTerm::Live(ref mut term) = terminal {
        enable_mouse_paste_cursor(term.backend_mut());
    }
    let mut first_draw_fail = false;
    match terminal.draw_app(&mut app, &mut first_draw_fail) {
        Ok(_) => crate::bench::record_draw(),
        Err(e) => {
            restore_attach(terminal);
            return Err(e);
        }
    }
    app.needs_redraw = false;
    app.pending_full_clears = 0;
    // Live Windows console only: conhost / PowerShell paste is an
    // unbracketed key flood, so a mid-paste Enter must not submit half the
    // clipboard. Scripted/headless queues deliver one event per batch and
    // must keep same-turn Enter submits.
    app.paste_enter_guard = paste_enter_guard_for(cfg!(windows), headless);
    app.keyboard_enhanced = keyboard_enhanced;
    if !headless && let Ok(dir) = Config::data_dir() {
        key_conflict_notice(&mut app, &dir);
    }

    if !bench_clock {
        whycodes_core::logging::emit(
            "whycodes_tui",
            "info",
            "tui.first_frame",
            Some(serde_json::json!({ "w": tw, "h": th })),
        );
    }

    // Paste / resize / focus can echo glyphs onto the PTY outside ratatui's
    // diff. Force a full themed redraw on the next paint so leftover text
    // cannot sit in the unpainted rows beside the prompt. Ordinary
    // Backspace/Delete must not bump `pending_full_clears` — home gutters
    // already fill_blank.
    // Deep-idle + malloc_trim clocks (redraw_schedule / idle_heap).
    let mut last_user_input = Instant::now();
    let mut idle_trim_armed = true;
    let mut retention = crate::heap::RetentionTicker::new(
        crate::heap::CLIENT_RETENTION_TRIM_BYTES,
        crate::heap::RETENTION_CHECK_EVERY,
        Instant::now(),
    );
    let result = async {
        'main: loop {
            if app.session_id != cancel_owner {
                cancel_owner.clone_from(&app.session_id);
                cancel_requested_at = None;
            }
            // ── Drain background sessions into their own view snapshots ──
            // Events on inactive runtimes never touch `app`; they update the
            // runtime's snapshot + state and set `unread` so the dashboard
            // and cycle keys can surface activity.
            for bg in runtimes.iter_mut() {
                drain_background_runtime(bg);
                drain_background_goal(bg);
                // Parked sessions keep their cache warm too (opt-in).
                if remote.is_none() {
                    maybe_warm_cache(
                        bg,
                        config.session.cache_warm_minutes,
                        &provider,
                        &model,
                        &api_key,
                    );
                }
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
            maybe_offer_openrouter_key(&mut app, &config, &provider, &api_key);

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

            // ── Stream events from rt.agent (coalesce text/thinking deltas) ──
            // Before the draw, so tokens that arrived during the last poll
            // paint this iteration instead of one poll (≤40 ms) later.
            after_turn_events_drain(&mut app, &mut rt.event_rx, &config, &file_index);

            // Paced wheel/trackpad backlog (scroll clock): one capped
            // chunk per 16 ms instead of the whole flick in one frame.
            app.tick_wheel();

            // Paint only when something changed. Busy turns used to paint
            // every iteration; deltas and spinner ticks already mark dirty,
            // and static toasts never did (~180 wasted frames each).
            if app.needs_redraw {
                if app.pending_full_clears > 0 {
                    // Full *themed* redraw, not CSI erase: Windows paints
                    // ClearType::All with the profile default background
                    // (white flash on every submit / paste / session
                    // switch), and the erase flushes outside the
                    // synchronized-update dump. `render` fill_blanks the
                    // whole frame, so resetting the previous buffer makes
                    // the next draw rewrite every cell — paste echo is
                    // overwritten without an intermediate blank frame.
                    terminal.reset_prev_for_full_redraw();
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
                if after_draw_frame(&mut app, snapshot, bench.as_ref()) {
                    break;
                }
            }

            if bench_should_break(bench.as_ref()) {
                break;
            }

            if should_tick_spinner(&app, rt.agent_busy)
                && last_spinner_tick.elapsed() >= crate::ui::spinner::TICK_INTERVAL
            {
                last_spinner_tick = Instant::now();
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
            if let Some(target) = app.pending_tree_target.take() {
                apply_tree_target(&mut app, &mut rt, target);
            }

            if app.pending_local_discovery {
                app.pending_local_discovery = false;
                spawn_local_discovery(discover_tx.clone());
            }
            if let Some(server) = app.pending_local_server.take() {
                apply_local_server_choice(&mut app, &mut rt, &mut config, &server);
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

            if app.pending_terminal_setup {
                app.pending_terminal_setup = false;
                apply_terminal_setup(&mut app, whycodes_import::discover::home_dir().as_deref());
            }

            if app.pending_import && !rt.agent_busy {
                app.pending_import = false;
                let wrote = apply_pending_import(
                    &mut app,
                    &mut rt.agent,
                    &mut config,
                    &project_dir,
                    &file_index,
                );
                if wrote {
                    mcp = PendingMcp::start(&mut rt, &config).await;
                }
            }

            // Deferred / idle catalog: never race the first (or any) user turn.
            if should_spawn_idle_catalog(
                catalog_fetch_pending,
                rt.agent_busy,
                app.has_pending_turn(),
                missing_key,
            ) {
                catalog_fetch_pending = false;
                spawn_model_context_fetch(&config, &provider, &model, &api_key, catalog_tx.clone());
            }

            // A7 suggestion results
            while let Ok(suggestion) = suggest_rx.try_recv() {
                apply_idle_suggestion(&mut app, suggestion, rt.agent_busy);
            }

            // `/goal` verdicts for the visible session.
            while let Ok(check) = rt.goal_rx.try_recv() {
                apply_goal_check(&mut app, &mut rt, check, config.session.goal_max_turns);
            }

            // Local model servers found by the provider-dialog probe.
            while let Ok(found) = discover_rx.try_recv() {
                apply_local_discovery(&mut app, &config, found);
            }

            // OAuth login flow progress (`/connect` spawned task).
            while let Ok(ev) = auth_rx.try_recv() {
                apply_auth_flow_event(
                    &mut app,
                    &mut rt,
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

            // Agent hub: cancel a worker now; a re-run waits for an idle session.
            crate::agent_hub::send_pending_cancel(&mut app, &rt.worker_cancels);
            crate::agent_hub::send_pending_rerun(&mut app, rt.agent_busy);

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

            // ── Prompt-cache keep-alive while idle (opt-in) ─────────────
            if remote.is_none() {
                maybe_warm_cache(
                    &mut rt,
                    config.session.cache_warm_minutes,
                    &provider,
                    &model,
                    &api_key,
                );
            }

            // ── Steer a running turn / reclaim unread messages ─────────
            match remote {
                Some(ref rem) => steer_remote(&mut app, &rt, rem, &project_dir),
                None => steer_or_reclaim(&mut app, &mut rt, &project_dir),
            }

            mcp.poll(&mut app, &mut rt, &mut runtimes);
            // A Ctrl+V image read finishes off the UI thread.
            crate::input::poll_clipboard_paste(&mut app);

            // ── Start turn if needed ──────────────────────────────────
            if !rt.agent_busy
                && let Some(turn) = app.take_pending_turn()
            {
                let prompt = turn.text;
                let submit_images = turn.images;

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

                mcp.wait(&mut app, &mut rt, &mut runtimes, mcp_defer::TURN_WAIT)
                    .await;
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
            // `redraw_schedule` (toasts ≠ animation; 30s → 5s deep idle).
            let awaiting_matches = app.file_suggest.awaiting_matches();
            let poll_for =
                crate::redraw_schedule::poll_interval(&crate::redraw_schedule::RedrawNeed {
                    agent_busy: rt.agent_busy,
                    running_subagents: app.running_task_count() > 0,
                    awaiting_matches,
                    needs_redraw: app.needs_redraw,
                    wheel_scrolling: app.wheel.is_active(),
                    toasts_visible: !app.toasts.is_empty(),
                    since_user_input: last_user_input.elapsed(),
                });
            maybe_idle_heap_trim(
                rt.agent_busy,
                last_user_input.elapsed(),
                &mut idle_trim_armed,
            );
            retention.tick(Instant::now());

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
                // One id per batch: the paste Enter guard compares the
                // batch of the last prompt insert with the batch of the
                // Enter to spot one-key-per-poll paste floods.
                app.input_batch_seq = app.input_batch_seq.wrapping_add(1);
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
                crate::desktop_notify::note_focus_events(&batch);
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
                if batch
                    .iter()
                    .any(crate::redraw_schedule::event_needs_focus_redraw)
                {
                    terminal.reset_prev_for_full_redraw();
                }

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
                    {
                        if matches!(action, IdleLoopKey::SlashEnter)
                            && slash_command_from_prompt(&app)
                                .is_some_and(|cmd| mcp_defer::reads_mcp_servers(&cmd))
                        {
                            mcp.wait(&mut app, &mut rt, &mut runtimes, mcp_defer::TURN_WAIT)
                                .await;
                        }
                        if apply_idle_loop_key(
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
                    }

                    // While busy: Esc cancels (draft preserved). Typing, scroll,
                    // and focus still work so the user can queue thoughts.
                    // Permission / question overlays own Esc/Enter — do not steal them.
                    let overlay_owns_keys = dialog_overlay_owns_keys(app.dialogs.active())
                        || hub_owns_key(&app, &ev)
                        || picker_owns_key(&app, &ev);
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
                    if !rt.agent_busy && std::mem::take(&mut app.clear_session_requested) {
                        apply_idle_loop_key(
                            IdleLoopKey::ClearSession,
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
                        .await;
                    }
                    // Mouse / [✗] set flags inside handle_event — complete the
                    // oneshot on this same event, not the next tick.
                    flush_pending_question_replies(
                        &mut app,
                        &mut rt.pending_question_queue,
                        &rt.pending_perm_queue,
                    );
                } // for ev in batch
                if should_preconnect(
                    &app.input_buffer,
                    config.session.preconnect,
                    remote.is_some(),
                ) {
                    rt.agent.preconnect(&provider);
                }
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

    // Deny waiters, abort in-flight turns (including the active session),
    // and kill background jobs so Tokio drop is not blocked on HTTP/MCP.
    shutdown_runtime_queues(&mut rt);
    for mut bg in runtimes.drain(..) {
        shutdown_runtime_queues(&mut bg);
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

    // Final flush + exit summary on the normal terminal (scrollback).
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

/// `/tree` selection: refuse while a turn is in flight, otherwise make the
/// chosen turn the active leaf. Undo history belongs to the old path, so it
/// starts over.
fn apply_tree_target(app: &mut TuiApp, rt: &mut SessionRuntime, target: TreeTarget) {
    if rt.agent_busy {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            "Can't switch branches while a turn is running",
        );
        return;
    }
    match rt.session.switch_branch(target) {
        Ok(true) => {
            rt.history = SessionHistory::new();
            app.load_messages_from_session(&rt.session);
            app.sync_context_estimate(&rt.session);
            persist_session_best_effort(&rt.session, "branch_switch");
            app.toasts
                .push(crate::toast::ToastKind::Success, "Switched branch");
        }
        Ok(false) => app.status_message = "Already on that turn".into(),
        Err(e) => app.status_message = format!("/tree: {e}"),
    }
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

/// Apply `f` to a detached copy of `view` without cloning the transcript or
/// touching the process-wide syntax theme.
fn with_view_scratch(view: &mut crate::session_runtime::ViewSnapshot, f: impl FnOnce(&mut TuiApp)) {
    let mut scratch = TuiApp::from_config(crate::config::TuiAppConfig::default());
    scratch.adopt_view(view);
    f(&mut scratch);
    scratch.yield_view(view);
}

/// What one drain applied, ordered so the strongest kind wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Drained {
    Nothing,
    /// Only text / thinking deltas: the transcript grew, nothing else moved.
    Deltas,
    /// Tool calls, results, status… — anything that can touch the workspace.
    Events,
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

fn note_stash_join(joined: Result<whycodes_session::TreeSnapshot, tokio::task::JoinError>) {
    if let Err(e) = joined {
        tracing::debug!(error = %e, "undo stash wait failed");
    }
}

fn tick_spinner(app: &mut TuiApp, spinner_frame: &mut usize) {
    *spinner_frame = (*spinner_frame + 1) % crate::ui::spinner::frame_count();
    app.spinner_frame = *spinner_frame;
    app.mark_dirty();
    let generic = app.status_message.contains("Generating")
        || app
            .status_message
            .chars()
            .next()
            .map(crate::ui::spinner::is_spinner_char)
            .unwrap_or(false);
    if generic {
        app.status_message.clear();
    }
}

fn goal_chip(goal: Option<&crate::goal::Goal>) -> Option<&'static str> {
    goal.map(|g| {
        if g.paused.is_some() {
            "◎ goal paused"
        } else {
            "◎ goal"
        }
    })
}

fn should_tick_spinner(app: &TuiApp, agent_busy: bool) -> bool {
    (agent_busy || app.running_task_count() > 0)
        && !matches!(
            app.current_agent_state,
            AgentState::WaitingForPermission | AgentState::WaitingForQuestion
        )
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

/// Record a finished keep-alive, then send the next one when it is due.
fn maybe_warm_cache(
    rt: &mut SessionRuntime,
    window_minutes: u64,
    provider: &str,
    model: &str,
    api_key: &str,
) {
    use futures::FutureExt;
    if let Some(job) = rt.warm_job.take_if(|j| j.is_finished())
        && let Some(Ok(Some(usage))) = job.now_or_never()
    {
        rt.agent
            .record_cache_warm(&mut rt.session, provider, model, &usage);
    }
    let Some(ended) = rt.last_turn_end else {
        return;
    };
    let window = Duration::from_secs(window_minutes.saturating_mul(60));
    let now = Instant::now();
    if rt.agent_busy
        || rt.warm_job.is_some()
        || !whycodes_agent::agent::cache_warm::warm_due(now, ended, rt.last_warm, window)
    {
        return;
    }
    rt.last_warm = Some(now);
    rt.warm_job = rt
        .agent
        .cache_warm_job(&rt.session, provider, model, api_key)
        .map(tokio::spawn);
}

/// While a local turn runs, queued prompts (with `@file` expansion and
/// images) go to its inbox and are read at the next step. Once it ends,
/// whatever it did not read is queued again first, exactly as typed.
fn steer_or_reclaim(app: &mut TuiApp, rt: &mut SessionRuntime, project_dir: &std::path::Path) {
    if !rt.agent_busy {
        let unread = rt.steer.drain(&rt.steer_session).len();
        let read = rt.steered.len().saturating_sub(unread);
        let left: Vec<crate::app::PendingTurn> = rt.steered.drain(..).skip(read).collect();
        if !left.is_empty() {
            app.reclaim_steering(left);
            app.mark_dirty();
        }
        return;
    }
    let turns = app.take_steerable_turns(false);
    if turns.is_empty() {
        return;
    }
    for turn in turns {
        let content = steer_content(app, &turn, project_dir);
        rt.steer.push_mode(&rt.steer_session, turn.mode, content);
        rt.steered.push_back(turn);
    }
    app.status_message = "sent to the running turn".into();
    app.mark_dirty();
}

/// What the agent receives for a steered prompt: `@file` mentions expanded,
/// images attached; a failed image falls back to the text, as on submit.
fn steer_content(
    app: &mut TuiApp,
    turn: &crate::app::PendingTurn,
    project_dir: &std::path::Path,
) -> whycodes_core::types::MessageContent {
    use whycodes_core::types::MessageContent;
    let expanded = expand_at_files(&turn.text, project_dir);
    if turn.images.is_empty() {
        return MessageContent::Text(expanded);
    }
    match crate::images::build_user_blocks(&expanded, &turn.images) {
        Ok(blocks) => MessageContent::Blocks(blocks),
        Err(e) => {
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                format!("Image attach failed: {e}"),
            );
            MessageContent::Text(expanded)
        }
    }
}

/// While a daemon turn runs, queued text prompts go to the daemon, which
/// replays any it did not read as `enqueue_prompt` events. Images stay
/// queued: the daemon chat API is text only.
fn steer_remote(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    rem: &crate::remote::RemoteAttach,
    project_dir: &std::path::Path,
) {
    if !rt.agent_busy {
        return;
    }
    let turns = app.take_steerable_turns(true);
    if turns.is_empty() {
        return;
    }
    for turn in turns {
        let text = expand_at_files(&turn.text, project_dir);
        let (rem, events, mode) = (rem.clone(), rt.event_tx.clone(), turn.mode);
        tokio::spawn(async move {
            if let Err(e) = crate::remote::steer(&rem, &text, mode).await {
                tracing::debug!(error = %e, "steer failed; queued instead");
                if let Err(e) = events.send(TurnEvent::EnqueuePrompt { text }) {
                    tracing::debug!(error = %e, "steer fallback dropped");
                }
            }
        });
    }
    app.status_message = "sent to the running turn".into();
    app.mark_dirty();
}

/// Esc or a failed turn ends a `/loop`: the queued repeats would otherwise
/// start right away (and repeat the same error) after the user asked to stop.
fn stop_loop_after_error(app: &mut TuiApp, why: &str) {
    let dropped = app.pending_auto_prompts.len();
    if dropped == 0 {
        return;
    }
    app.pending_auto_prompts.clear();
    app.add_message(
        ChatRole::System,
        format!(
            "↻ loop stopped: {why}; {dropped} queued prompt{} dropped.",
            if dropped == 1 { "" } else { "s" }
        ),
    );
}

fn queue_auto_prompt_if_idle(app: &mut TuiApp, agent_busy: bool) {
    if agent_busy || app.has_pending_turn() {
        return;
    }
    if let Some(next) = app.pending_auto_prompts.pop_front() {
        app.enqueue_prompt_text(next);
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
    bench: Option<&crate::bench::BenchConfig>,
) -> bool {
    apply_draw_snapshot(app, snapshot);
    crate::heap::run_deferred_release();
    app.needs_redraw = app.pending_full_clears > 0;
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
    let drained = drain_turn_events(app, event_rx);
    if drained == Drained::Nothing {
        return false;
    }
    // Deltas only grow the transcript. Re-listing every indexed file per
    // token batch cost more than the frame; the tree moves with tool events.
    if drained == Drained::Events {
        refresh_sidebar_if_visible(app, config, file_index);
    }
    app.mark_dirty();
    true
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

fn shown_tool_name(name: &str) -> &str {
    match name {
        "bash" | "shell" | "run_terminal_command" => "run",
        "read_file" => "read",
        "search_code" | "rg" => "grep",
        other => other,
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

/// Open the provider connection once a prompt is being typed. Not for a slash
/// command, and not when attached to a daemon, which makes the requests.
fn should_preconnect(input: &str, enabled: bool, remote: bool) -> bool {
    let input = input.trim_start();
    enabled && !remote && !input.is_empty() && !input.starts_with('/')
}

const EMPTY_ANSWER_NOTE: &str = "The model ended the turn without an answer.";
