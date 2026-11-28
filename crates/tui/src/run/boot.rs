//! Start-up: building the app and runtime, resuming a session, and the work
//! deferred until after the first frame.
use super::*;

/// Test helper: chrome then runtime (production `run` paints between them).
#[cfg(test)]
pub(super) async fn prepare_tui_boot(opts: &TuiRunOptions) -> TuiRuntimeBoot {
    let mut chrome = prepare_tui_chrome(opts);
    let runtime = prepare_tui_runtime(opts, &mut chrome.app).await;
    TuiRuntimeBoot {
        app: chrome.app,
        missing_key: chrome.missing_key,
        agent: runtime.agent,
        session: runtime.session,
    }
}

pub(super) fn apply_resume(
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

pub(super) fn apply_resume_loaded(
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
            crate::heap::request_release_after_draw("history_loaded");
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

pub(super) fn resume_missing_toast(want: &str) -> String {
    if want == RESUME_LATEST {
        "No saved sessions to continue".into()
    } else {
        format!("Session not found: {want}")
    }
}

pub(super) async fn apply_remote_hydrate(
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
                crate::heap::request_release_after_draw("remote_history_loaded");
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

pub(super) fn prepare_tui_chrome(opts: &TuiRunOptions) -> TuiChrome {
    // Built-in theme only on the first frame. `from_core_config` walks
    // `~/.config/whycodes/themes/` — that belongs after `record_draw`.
    let tui_cfg = TuiAppConfig::default();
    let mut app = TuiApp::new(tui_cfg);
    // Events per wheel notch depend on the host terminal; `new` stays hermetic.
    app.wheel = crate::wheel::WheelScroll::from_env();

    app.provider_name = opts.provider.clone();
    app.model_name = opts.model.clone();
    app.reasoning_effort = opts.config.session.reasoning_effort.clone();
    app.approval_mode = opts.config.general.approval_mode.unwrap_or_default();
    app.agent_name = opts.agent_name.clone();
    crate::desktop_notify::configure(&opts.config.notify);
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
        config: opts.config.clone(),
        missing_key,
    }
}

/// After first paint: load layered TOML and refresh chrome from it.
/// CLI `-P`/`-m`/`-a` already sit on `opts`; only fill blanks from config.
pub(super) fn apply_deferred_config(
    opts: &mut TuiRunOptions,
    app: &mut TuiApp,
    config: &mut Config,
    provider: &mut String,
    model: &mut String,
) {
    let mut loaded = Config::load_layered(&opts.project_dir)
        .or_else(|_| Config::load_or_create())
        .unwrap_or_default();
    loaded.general.project_path = Some(opts.project_dir.clone());
    if let Some(mode) = opts.approval_mode_from_cli {
        loaded.general.approval_mode = Some(mode);
    }
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

pub(super) async fn prepare_tui_runtime(opts: &TuiRunOptions, app: &mut TuiApp) -> TuiRuntime {
    let mut config = opts.config.clone();
    // Empty index until hydrate: `@` stays inert for frame 0 without
    // `canonicalize` + scan (issue #49). Started here so Agent can hold it.
    let file_index = whycodes_index::WorkspaceIndex::start(Vec::new());
    app.set_file_index(file_index.clone());

    let agent_info = config
        .get_agent(&opts.agent_name)
        .cloned()
        .unwrap_or_else(|| default_agent_info(&opts.agent_name));

    // Deferred: AGENTS.md + memory + plugins each touch disk/SQLite. Home has
    // no fenced code and needs no tool plugins, so the first frame uses a
    // minimal prompt; the full prompt is hydrated after paint.

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
    agent.set_route(&opts.provider, &opts.model);
    agent.set_approval_mode(app.approval_mode);
    inject_test_llm(&mut agent, &opts.provider);
    let system_prompt = agent.system_prompt();

    let mut session = Session::new(opts.project_dir.clone(), system_prompt.clone());
    app.session_title = session.title.clone();
    app.session_id = session.id.clone();
    let history = SessionHistory::with_depth(opts.config.session.undo_depth);

    if let Some(ref want) = opts.resume_session_id {
        let fresh_id = session.id.clone();
        apply_resume(
            app,
            &mut session,
            &system_prompt,
            want,
            opts.config.session.auto_title,
        );
        if opts.fork_session && session.id != fresh_id {
            fork_session_in_place(app, &mut session, "");
        }
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

pub(super) fn apply_boot_prompt(
    app: &mut TuiApp,
    missing_key: bool,
    initial_prompt: Option<String>,
) {
    if missing_key {
        app.status_message = "no API key · /connect".to_string();
    }
    if let Some(p) = initial_prompt
        && !p.is_empty()
    {
        app.add_message(ChatRole::User, &p);
        app.enqueue_prompt_text(p);
    }
}

pub(super) fn load_app_todos(app: &mut TuiApp) {
    app.replace_todos(whycodes_core::todo::load_todos(
        &app.project_dir,
        if app.session_id.is_empty() {
            None
        } else {
            Some(app.session_id.as_str())
        },
    ));
}

/// Load deferred chrome before the first *visible* interactive paint.
///
/// Interactive `run` no longer paints a black splash then the themed home
/// (two full-screen dumps). Bench still uses splash and never reaches here.
/// MCP servers keep connecting after it returns; see [`mcp_defer`].
#[allow(clippy::too_many_arguments)]
pub(super) async fn hydrate_after_first_frame(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    file_index: &mut Arc<whycodes_index::WorkspaceIndex>,
    api_key: &mut String,
    provider: &str,
    model: &str,
    config: &mut Config,
    project_dir: &std::path::Path,
    animate: bool,
) -> PendingMcp {
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
    hydrate_full_system_prompt(rt, project_dir, config, provider, model);
    hydrate_session_picker(app);
    hydrate_deferred_api_key(api_key, provider, model, config, app);
    let mcp = PendingMcp::start(rt, config).await;
    maybe_session_auto_index(project_dir, config);
    refresh_sidebar(app, config, file_index);
    load_app_todos(app);
    settle_first_frame_hydrate(app, &hydrate_before, animate);
    mcp
}

pub(super) fn hydrate_auth_plugins(project_dir: &std::path::Path) -> usize {
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

pub(super) fn start_workspace_file_index(
    project_dir: &std::path::Path,
) -> Arc<whycodes_index::WorkspaceIndex> {
    whycodes_index::WorkspaceIndex::start(whycodes_index::WorkspaceIndex::project_roots(
        project_dir,
    ))
}

pub(super) fn hydrate_full_system_prompt(
    rt: &mut SessionRuntime,
    project_dir: &std::path::Path,
    config: &Config,
    provider: &str,
    model: &str,
) {
    rt.agent
        .set_system_prompt_overlays(config.system_prompt_overlays.clone());
    rt.agent.set_route(provider, model);
    let with_agents = whycodes_agent::agent::Agent::with_agents_md(
        &rt.agent.system_prompt_for_route(provider, model),
        project_dir,
    );
    let full = with_project_memory(&with_agents, project_dir, config, None);
    rt.session.set_system_prompt(&full);
}

pub(super) fn hydrate_session_picker(app: &mut TuiApp) {
    if app.session_list.sessions.is_empty() {
        let entries = load_session_entries();
        if !entries.is_empty() {
            app.session_list.sessions = entries;
        }
    }
}

pub(super) fn hydrate_deferred_api_key(
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
