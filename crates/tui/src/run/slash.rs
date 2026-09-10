//! Slash-command handling for the TUI event loop.
use super::*;

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

pub(super) const LOOP_USAGE: &str = "Usage: /loop N prompt…  |  /loop stop";

pub(super) fn oauth_unavailable_hint(names: Vec<String>) -> String {
    if names.is_empty() {
        "install an auth plugin".to_string()
    } else {
        names.join(", ")
    }
}

pub(super) enum LoopSlash {
    Stop,
    Queue { n: usize, prompt: String },
    Usage,
}

/// `/loop stop|clear` or `/loop N prompt…` (N defaults to 3, clamped 1..=20).
pub(super) fn parse_loop_slash(rest: &str) -> LoopSlash {
    let rest = rest.trim();
    if rest == "stop" || rest == "clear" {
        return LoopSlash::Stop;
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("").trim();
    let rest_prompt = parts.next().unwrap_or("").trim();
    let (n, prompt) = if let Ok(count) = first.parse::<usize>() {
        (count, rest_prompt.to_string())
    } else if !rest.is_empty() {
        (3usize, rest.to_string())
    } else {
        return LoopSlash::Usage;
    };
    if prompt.is_empty() {
        return LoopSlash::Usage;
    }
    LoopSlash::Queue {
        n: n.clamp(1, 20),
        prompt,
    }
}

/// Slash line to run, if the prompt currently holds a `/command`.
pub(super) fn slash_command_from_prompt(app: &TuiApp) -> Option<String> {
    let mut text = app.input_buffer.trim().to_string();
    if app.slash_suggest.active
        && let Some(cmd) = app.slash_suggest.current()
    {
        text = cmd.name.to_string();
    }
    text.starts_with('/').then_some(text)
}

pub(super) fn consume_slash_draft(app: &mut TuiApp) {
    app.input_buffer.clear();
    app.input_cursor = 0;
    app.pending_pastes.clear();
    app.slash_suggest.dismiss();
}

/// Max chars inlined per `@file` (speculative context without blowing prefill).
pub(super) const AT_FILE_MAX_CHARS: usize = 24_000;

pub(super) fn expand_at_files(input: &str, project_dir: &std::path::Path) -> String {
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
            Err(_missing) => {
                result.push('@');
                result.push_str(path_str);
            }
        }
        rest = &after[end..];
    }
    result.push_str(rest);
    result
}

pub(super) async fn handle_slash(text: &str, ctx: &mut SlashContext<'_>) {
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
                    ctx.app.add_message(
                        ChatRole::System,
                        format_bg_jobs(ctx.agent.background_registry().running_count(), &jobs),
                    );
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
        "/loop" => match parse_loop_slash(rest) {
            LoopSlash::Stop => {
                let n = ctx.app.pending_auto_prompts.len();
                ctx.app.pending_auto_prompts.clear();
                ctx.app.toasts.push(
                    crate::toast::ToastKind::Info,
                    format!("Cleared {n} queued loop prompt(s)"),
                );
            }
            LoopSlash::Queue { n, prompt } => {
                ctx.app.add_message(ChatRole::User, &prompt);
                ctx.app.pending_prompt = Some(prompt.clone());
                for _ in 1..n {
                    ctx.app.pending_auto_prompts.push_back(prompt.clone());
                }
                ctx.app
                    .toasts
                    .push(crate::toast::ToastKind::Info, format!("Loop ×{n} queued"));
            }
            LoopSlash::Usage => {
                ctx.app.status_message = LOOP_USAGE.into();
            }
        },
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
                        Err(e) => memory_err_toast(ctx.app, e),
                    },
                    Err(e) => memory_err_toast(ctx.app, e),
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
            Err(e) => memory_err_toast(ctx.app, e),
        },
        "/share" | "/export" => match ctx.session.export_share() {
            Ok(p) => apply_share_ok(ctx.app, &p, &ctx.session.id, share_server_up),
            Err(e) => export_failed_toast(ctx.app, e),
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
                ctx.app.status_message = format!(
                    "OAuth login not available for `{arg}` ({})",
                    oauth_unavailable_hint(whycodes_auth::oauth_providers())
                );
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
        "/mode" if rest.is_empty() => {
            crate::input::open_mode_dialog(ctx.app);
        }
        "/mode" => apply_approval_mode_raw(ctx.app, ctx.agent, ctx.config, rest),
        "/models" => {
            if let Some((p, m)) = rest.split_once('/') {
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
        "/import" => {
            handle_import_slash(ctx.app, rest);
        }
        other => {
            ctx.app.toasts.push(
                crate::toast::ToastKind::Warning,
                format!("Unknown command {other} — try /help"),
            );
        }
    }
}

pub(super) fn format_bg_jobs(running: usize, jobs: &[whycodes_agent::JobSnapshot]) -> String {
    let mut lines = vec![format!("Background jobs ({running} running)")];
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
    lines.join("\n")
}

pub(super) fn memory_err_toast(app: &mut TuiApp, e: impl std::fmt::Display) {
    app.toasts
        .push(crate::toast::ToastKind::Error, format!("Memory: {e}"));
}

pub(super) fn export_failed_toast(app: &mut TuiApp, e: impl std::fmt::Display) {
    app.toasts.push(
        crate::toast::ToastKind::Error,
        format!("Export failed: {e}"),
    );
}

fn apply_share_ok(app: &mut TuiApp, path: &str, session_id: &str, live: impl FnOnce(u16) -> bool) {
    let md = path.replace(".json", ".md");
    let port = std::env::var("WHYCODES_SHARE_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3030u16);
    let url = format!("http://127.0.0.1:{port}/s/{session_id}");
    let live = live(port);
    app.status_message = if live {
        format!("Share: {url}")
    } else {
        format!("Exported — run `whycodes serve` then open {url}")
    };
    app.add_message(
        ChatRole::System,
        format!(
            "Session shared locally:\n\
             - {path}\n\
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

/// Spawn a tiny follow-up suggestion when `tui.prompt_suggestions = "idle"`.
pub(super) fn maybe_spawn_prompt_suggestion(
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
        let request = suggestion_llm_request(&last_user, &last_asst);
        if let Ok(resp) = suggestion_transport()
            .complete(prov, &request, &api_key, &m)
            .await
        {
            send_suggestion_text(&resp.content, &suggest_tx);
        }
    });
}

pub(super) fn suggestion_prompt_body(last_user: &str, last_asst: &str) -> String {
    format!(
        "User last said:\n{}\n\nAssistant replied (excerpt):\n{}\n\n\
         Suggest ONE short next user message (≤12 words) to continue the coding task. \
         Reply with only that message, no quotes.",
        last_user.chars().take(500).collect::<String>(),
        last_asst.chars().take(400).collect::<String>()
    )
}

pub(super) fn suggestion_llm_request(
    last_user: &str,
    last_asst: &str,
) -> whycodes_core::types::LlmRequest {
    use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};
    LlmRequest {
        system: "You propose a single follow-up user prompt for a coding agent.".into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text(suggestion_prompt_body(last_user, last_asst)),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(40),
        temperature: Some(0.4),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

pub(super) fn suggestion_transport() -> whycodes_llm::LlmTransport {
    whycodes_llm::LlmTransport {
        complete_timeout: Some(std::time::Duration::from_secs(8)),
        retry: whycodes_llm::RetryPolicy {
            max_retries: 0,
            initial_backoff: std::time::Duration::from_millis(100),
            max_backoff: std::time::Duration::from_secs(1),
            max_elapsed: std::time::Duration::from_secs(8),
            full_jitter: true,
        },
    }
}

pub(super) fn send_suggestion_text(
    content: &[whycodes_core::types::ContentBlock],
    tx: &mpsc::UnboundedSender<String>,
) {
    let text = suggestion_text_from_blocks(content);
    if !text.is_empty() && tx.send(text).is_err() {
        tracing::debug!("idle suggestion dropped: TUI event loop closed");
    }
}

pub(super) fn suggestion_text_from_blocks(
    content: &[whycodes_core::types::ContentBlock],
) -> String {
    content
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
        .to_string()
}

#[cfg(test)]
#[path = "slash_tests.rs"]
mod tests;
