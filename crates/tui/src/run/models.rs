//! Model, provider, credential and agent choices made from the TUI.
use super::*;

/// Agent picker selection: refuse while a turn is in flight, otherwise switch.
pub(super) async fn apply_pending_agent(
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

pub(super) fn warn_missing_api_key(app: &mut TuiApp, provider: &str) {
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
pub(super) async fn apply_pending_picker_choices(
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
    // Mid-turn `rt.agent` is a placeholder the finished turn replaces: a
    // route, effort or mode set on it now would be lost. Hold the choice.
    let agent_choice = app.pending_model.is_some()
        || app.pending_effort.is_some()
        || app.pending_approval_mode.is_some();
    if rt.agent_busy && agent_choice {
        if !app.picker_wait_noted {
            app.picker_wait_noted = true;
            app.toasts
                .push(crate::toast::ToastKind::Info, "Applies when this turn ends");
        }
        return;
    }
    app.picker_wait_noted = false;
    if let Some((p, m)) = app.pending_model.take() {
        apply_model_choice(app, provider, model, api_key, p, m, config);
        rt.agent.set_route(provider, model);
        refresh_session_memory(&mut rt.session, &rt.agent, &app.project_dir, config, None);
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
    apply_pending_openrouter_key(app, config, provider, model, api_key);
}

/// Probe loopback for local model servers; the result lands in the
/// provider dialog through `tx`. Runs only after the user opened the dialog.
pub(super) fn spawn_local_discovery(tx: mpsc::UnboundedSender<Vec<whycodes_llm::LocalServer>>) {
    tokio::spawn(async move {
        let found =
            whycodes_llm::discover_local_servers(whycodes_llm::DEFAULT_DISCOVERY_TIMEOUT).await;
        seed_unbounded(&tx, found, "local server discovery");
    });
}

/// Probe finished: list the servers in the provider dialog. A server that a
/// configured provider already points at is listed once, as that provider.
pub(super) fn apply_local_discovery(
    app: &mut TuiApp,
    config: &Config,
    found: Vec<whycodes_llm::LocalServer>,
) {
    app.provider_dialog.discovering = false;
    app.provider_dialog.local_servers = found
        .into_iter()
        .filter(|s| {
            !matches!(
                s.plan_save(config.get_provider(&s.provider)),
                whycodes_llm::LocalSave::AlreadyConfigured
            )
        })
        .collect();
    app.mark_dirty();
}

/// Discovered server picked in the provider dialog: persist it as
/// `[providers.<id>]` (api_base, no key), teach the live agent about it and
/// queue a switch to its first model. A configured block is never
/// overwritten: one pointing elsewhere makes this a no-op with a warning.
pub(super) fn apply_local_server_choice(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    config: &mut Config,
    server: &whycodes_llm::LocalServer,
) {
    if rt.agent_busy {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            "Can't switch provider while a turn is running",
        );
        return;
    }
    let block = match server.plan_save(config.get_provider(&server.provider)) {
        whycodes_llm::LocalSave::New(pc) | whycodes_llm::LocalSave::FillApiBase(pc) => pc,
        whycodes_llm::LocalSave::AlreadyConfigured => {
            queue_local_server_model(app, server);
            return;
        }
        whycodes_llm::LocalSave::Conflict { configured } => {
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                format!(
                    "`{}` is already configured with a different api_base ({configured}); skipped",
                    server.provider
                ),
            );
            return;
        }
    };
    config
        .providers
        .insert(server.provider.clone(), block.clone());
    // Only the new block goes to disk; `config` also holds project and env values.
    if let Err(e) = Config::update_global(&mut |global| {
        global
            .providers
            .insert(server.provider.clone(), block.clone());
    }) {
        tracing::warn!("could not persist provider '{}': {e}", server.provider);
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            format!("{} not saved to config.toml: {e}", server.provider),
        );
    }
    // The registry was built at startup; the new id must exist before the next turn.
    rt.agent.apply_config(config);
    app.add_message(
        ChatRole::System,
        format!(
            "✓ `{}` saved as `[providers.{}]` → {} (no API key needed)",
            server.label, server.provider, server.api_base
        ),
    );
    queue_local_server_model(app, server);
}

/// Switch to the server's first model, or say none is loaded.
pub(super) fn queue_local_server_model(app: &mut TuiApp, server: &whycodes_llm::LocalServer) {
    match server.models.first() {
        Some(model) => app.pending_model = Some((server.provider.clone(), model.clone())),
        None => app.toasts.push(
            crate::toast::ToastKind::Info,
            format!(
                "{} has no model loaded · load one, then /models",
                server.label
            ),
        ),
    }
}

pub(super) fn apply_pending_openrouter_key(
    app: &mut TuiApp,
    config: &mut Config,
    provider: &mut String,
    model: &mut String,
    api_key: &mut String,
) {
    let Some(key) = app.pending_openrouter_key.take() else {
        return;
    };
    if key.is_empty() {
        if let Err(e) = config.mark_openrouter_key_prompt_skipped() {
            tracing::warn!("could not persist OpenRouter skip flag: {e}");
            config.tui.skip_openrouter_key_prompt = true;
        }
        app.openrouter_key_prompted = true;
        app.status_message = "skipped OpenRouter key · /connect".into();
        return;
    }
    match config.set_openrouter_api_key(&key) {
        Ok(()) => {
            *api_key = key;
            *provider = whycodes_config::DEFAULT_PROVIDER.to_string();
            if model.is_empty() || *model == "claude-sonnet-4-20250514" || *model == "default" {
                *model = whycodes_config::DEFAULT_MODEL_ID.to_string();
            }
            app.provider_name = provider.clone();
            app.model_name = model.clone();
            app.status_message = format!(
                "agent={}  {}/{}  — Tab focus  Ctrl+T agent  Esc cancel  /help",
                app.agent_name, provider, model
            );
            app.toasts
                .push(crate::toast::ToastKind::Success, "OpenRouter API key saved");
        }
        Err(e) => {
            app.toasts.push(
                crate::toast::ToastKind::Error,
                format!("Could not save API key: {e}"),
            );
        }
    }
    app.openrouter_key_prompted = true;
}

/// First-launch OpenRouter paste modal. After first paint, and only when the
/// active provider is OpenRouter with no key (env / config / OAuth).
pub(super) fn maybe_offer_openrouter_key(
    app: &mut TuiApp,
    config: &Config,
    provider: &str,
    api_key: &str,
) {
    if app.openrouter_key_prompted || app.dialogs.is_open() {
        return;
    }
    if config.tui.skip_openrouter_key_prompt {
        app.openrouter_key_prompted = true;
        return;
    }
    if !api_key.is_empty() {
        app.openrouter_key_prompted = true;
        return;
    }
    if !provider.is_empty() && provider != whycodes_config::DEFAULT_PROVIDER {
        app.openrouter_key_prompted = true;
        return;
    }
    if std::env::var("OPENROUTER_API_KEY")
        .ok()
        .is_some_and(|v| !v.is_empty())
    {
        app.openrouter_key_prompted = true;
        return;
    }
    app.openrouter_key_prompted = true;
    app.openrouter_key_input.clear();
    app.mode = crate::app::AppMode::Dialog;
    app.key_context = crate::keymap::KeymapContext::Dialog;
    app.dialogs.push(DialogKind::OpenRouterKey);
}

pub(super) fn apply_catalog_window(
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

pub(super) async fn apply_auth_flow_event(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
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
                    rt.agent.set_route(provider, model);
                    refresh_session_memory(
                        &mut rt.session,
                        &rt.agent,
                        &app.project_dir,
                        config,
                        None,
                    );
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

pub(super) fn explicit_provider_key(config: &Config, provider: &str) -> Option<String> {
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

pub(super) fn try_fill_api_key(api_key: &mut String, provider: &str) {
    if !api_key.is_empty() {
        return;
    }
    let cfg = Config::load().unwrap_or_default();
    if let Some(k) = explicit_provider_key(&cfg, provider) {
        *api_key = k;
        whycodes_llm::oauth_refresh::unregister(provider);
    }
}

pub(super) async fn fill_oauth_credential(api_key: &mut String, provider: &str) {
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

pub(super) fn apply_model_choice(
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

pub(super) fn apply_reasoning_effort(
    app: &mut TuiApp,
    agent: &mut Agent,
    config: &mut Config,
    raw: &str,
) {
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

pub(super) fn persist_session_reasoning_effort(value: &str) -> anyhow::Result<()> {
    Config::update_global(&mut |global| {
        global.session.reasoning_effort = Some(value.to_string());
    })?;
    Ok(())
}

pub(super) fn apply_approval_mode_raw(
    app: &mut TuiApp,
    agent: &mut Agent,
    config: &mut Config,
    raw: &str,
) {
    let Some(mode) = ApprovalMode::parse(raw) else {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            format!("Unknown mode '{raw}' (auto, important, manual)"),
        );
        return;
    };
    apply_approval_mode(app, agent, config, mode);
}

pub(super) fn apply_approval_mode(
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

pub(super) fn persist_general_approval_mode(mode: ApprovalMode) -> anyhow::Result<()> {
    Config::update_global(&mut |global| global.general.approval_mode = Some(mode))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn cycle_agent(
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
pub(super) async fn switch_to_agent(
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
        let (route_provider, route_model) = agent.route();
        let route_provider = route_provider.to_string();
        let route_model = route_model.to_string();
        let bg = agent.background_registry().clone();
        let claims = agent.session_claims();
        // Keep the loaded MCP/plugin tools, file index and approval mode: a
        // fresh agent would otherwise start without them.
        let mut next = Agent::new(info)
            .with_config(config)
            .with_background_registry(bg)
            .with_shared_tools(agent.shared_tools())
            .with_permission_prompter(
                Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
            )
            .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);
        next.set_route(&route_provider, &route_model);
        next.set_approval_mode(agent.approval_mode());
        if let Some(c) = claims {
            next = next.with_session_claims(c);
        }
        if let Some(index) = agent.file_index() {
            next.set_file_index(index);
        }
        *agent = next;
        agent.wire_event_sink(event_tx.clone());
        let prompt = with_project_memory(
            &Agent::with_agents_md(
                &agent.system_prompt_for_route(&route_provider, &route_model),
                project_dir,
            ),
            project_dir,
            config,
            None,
        );
        session.set_system_prompt(&prompt);
    }
}

/// Recompute footer context max for the active provider/model.
pub(super) fn refresh_context_window(
    app: &mut TuiApp,
    config: &Config,
    provider: &str,
    model: &str,
) {
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
pub(super) fn spawn_model_context_fetch(
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

pub(super) fn skip_model_catalog() -> bool {
    std::env::var_os("WHYCODES_NO_MODEL_CATALOG").is_some()
}
