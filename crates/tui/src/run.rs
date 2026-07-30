//! TUI event loop — streaming agent + permission dialogs (OpenCode-style).

use std::io::stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use whycode_agent::agent::Agent;
use whycode_agent::permission::ChannelPermissionPrompter;
use whycode_agent::{CancelFlag, TurnEvent, new_cancel_flag, request_cancel};
use whycode_core::config::Config;
use whycode_core::types::AgentMode;
use whycode_session::SessionHistory;
use whycode_session::session::Session;

use crate::app::{AgentState, AppMode, ChatRole, DialogKind, TuiApp};
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
    app.project_label = opts
        .project_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| opts.project_dir.display().to_string());

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
            "agent={}  {}/{}  — Tab agent  Esc cancel  ? help",
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

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<TurnEvent>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<TurnOutcome>();

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

    let result = async {
        loop {
            terminal.draw(|f| render::render(f, &app))?;

            // ── Stream events from agent ──────────────────────────────
            while let Ok(ev) = event_rx.try_recv() {
                apply_turn_event(&mut app, ev);
            }

            // Spinner while generating (generic status only)
            if agent_busy
                && !matches!(
                    app.current_agent_state,
                    AgentState::WaitingForPermission
                )
            {
                const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                spinner_frame = (spinner_frame + 1) % FRAMES.len();
                let generic = app.status_message.contains("Generating")
                    || app
                        .status_message
                        .chars()
                        .next()
                        .map(|c| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(c))
                        .unwrap_or(false);
                if generic {
                    app.status_message =
                        format!("{} Generating…  [Esc cancel]", FRAMES[spinner_frame]);
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
                                && last.role == ChatRole::Assistant && last.content.is_empty() {
                                    last.content = text;
                                }
                        app.current_agent_state = AgentState::Idle;
                        app.status_message = format!(
                            "Ready — agent={}  {}/{}",
                            agent.info.name, provider, model
                        );
                        if let Some(db) = open_db_quiet() {
                            let _ = session.save_to_db(&db);
                        }
                    }
                    TurnOutcome::Err {
                        error,
                        agent: a,
                        session: s,
                        cancelled,
                    } => {
                        agent = a;
                        session = s;
                        if cancelled {
                            app.current_agent_state = AgentState::Idle;
                            app.status_message = "Cancelled.".into();
                            app.add_message(ChatRole::System, "⏹ Generation cancelled (Esc).");
                        } else {
                            app.current_agent_state = AgentState::Error(error.clone());
                            app.status_message = format!("Error: {error}");
                            app.add_message(ChatRole::System, format!("Error: {error}"));
                        }
                    }
                }
            }

            // ── Start turn if needed ──────────────────────────────────
            if !agent_busy
                && let Some(prompt) = app.pending_prompt.take() {
                    // Lazy-load API key from env/config when user first chats
                    if api_key.is_empty() {
                        if let Ok(cfg) = Config::load()
                            && let Some(pc) = cfg.get_provider(&provider)
                                && let Some(k) = &pc.api_key
                                    && !k.is_empty() {
                                        api_key = k.clone();
                                    }
                        let env_name = format!("{}_API_KEY", provider.to_uppercase());
                        if api_key.is_empty()
                            && let Ok(k) = std::env::var(&env_name)
                                && !k.is_empty() {
                                    api_key = k;
                                }
                    }
                    if api_key.is_empty() {
                        app.add_message(
                            ChatRole::System,
                            format!(
                                "Cannot call LLM: no API key for `{provider}`.\n\
                                 Set env `{}_API_KEY` or: whycode provider add {provider} --api-key <key>\n\
                                 Then /connect and try again.",
                                provider.to_uppercase()
                            ),
                        );
                        app.status_message = format!("no API key for {provider} — /connect");
                        continue;
                    }

                    agent_busy = true;
                    let flag = new_cancel_flag();
                    cancel_flag = Some(Arc::clone(&flag));
                    app.current_agent_state = AgentState::Generating;
                    app.status_message = "⠋ Generating…  [Esc cancel]".into();
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
                    session.add_user_message(&expanded);

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
            if event::poll(Duration::from_millis(40))? {
                let ev = event::read()?;

                // Permission dialog keys handled specially
                if matches!(
                    app.dialogs.active(),
                    Some(DialogKind::Permission { .. })
                )
                    && let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press {
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

                // Tab: cycle agents (when idle)
                if let Event::Key(key) = &ev
                    && key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Tab
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
                        let text = app.input_buffer.trim().to_string();
                        if text.starts_with('/') {
                            app.input_buffer.clear();
                            app.input_cursor = 0;
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

                // Block input submission while busy (except quit/scroll/Esc cancel)
                if agent_busy
                    && let Event::Key(key) = &ev
                        && key.kind == KeyEventKind::Press {
                            match key.code {
                                KeyCode::Esc => {
                                    if let Some(ref flag) = cancel_flag {
                                        request_cancel(flag);
                                        app.status_message =
                                            "Cancelling…".into();
                                    }
                                }
                                KeyCode::Char('q')
                                    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                                {
                                    if let Some(ref flag) = cancel_flag {
                                        request_cancel(flag);
                                    }
                                    app.running = false;
                                }
                                KeyCode::Char('c')
                                    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                                {
                                    if let Some(ref flag) = cancel_flag {
                                        request_cancel(flag);
                                    }
                                    app.running = false;
                                }
                                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                                    let _ = input::handle_event(&mut app, ev);
                                }
                                _ => {}
                            }
                            continue;
                        }

                if !input::handle_event(&mut app, ev) {
                    break;
                }
            }

            if !app.running {
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

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn apply_turn_event(app: &mut TuiApp, ev: TurnEvent) {
    match ev {
        TurnEvent::TextDelta(t) => {
            app.current_agent_state = AgentState::Generating;
            app.append_to_last(&t);
        }
        TurnEvent::ThinkingDelta(t) => {
            app.current_agent_state = AgentState::Thinking;
            app.append_thinking(&t);
        }
        TurnEvent::ToolStart { id, name, input } => {
            app.status_message = format!("tool: {name}  [Esc cancel]");
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
        TurnEvent::Cancelled => {
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
            ctx.app.status_message = "New session".into();
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
            Err(e) => ctx.app.status_message = format!("Export failed: {e}"),
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
                ctx.app.status_message = format!("no key — set {env_name} or provider add");
                ctx.app.add_message(
                    ChatRole::System,
                    format!(
                        "No API key for `{}`.\n\
                         1. $env:{env_name} = \"…\"   (PowerShell)\n\
                         2. whycode provider add {} --api-key <key>\n\
                         3. /connect again",
                        ctx.provider, ctx.provider
                    ),
                );
            } else {
                ctx.app.status_message = format!("API key loaded for {}", ctx.provider);
                ctx.app.add_message(
                    ChatRole::System,
                    format!("✓ API key ready for `{}` / `{}`.", ctx.provider, ctx.model),
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
                ctx.app.status_message = format!("Unknown agent '{rest}'");
            }
        }
        "/models" => {
            if rest.is_empty() {
                ctx.app.status_message = format!("Model: {}/{}", ctx.provider, ctx.model);
            } else if let Some((p, m)) = rest.split_once('/') {
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
                ctx.app.status_message = format!("Model → {}/{}", ctx.provider, ctx.model);
            } else {
                *ctx.model = rest.to_string();
                ctx.app.model_name = rest.to_string();
                ctx.app.status_message = format!("Model → {}", ctx.model);
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
            ctx.app.status_message = format!(
                "msgs={} tokens≈{} agent={}",
                ctx.session.messages.len(),
                ctx.session.token_count(),
                ctx.agent.info.name
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
            ctx.app.status_message = format!("Unknown: {other} — /help");
        }
    }
}
