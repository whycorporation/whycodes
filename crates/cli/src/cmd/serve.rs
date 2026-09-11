//! Connect, serve, and web commands.
use super::helpers::*;
use crate::Cli;
use colored::*;
use std::path::PathBuf;
use whycodes_agent::agent::Agent;
use whycodes_config::Config;
use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};

pub(crate) async fn cmd_connect(
    cli: &Cli,
    addr: &str,
    session: Option<&str>,
) -> anyhow::Result<()> {
    use whycodes_tui::remote;

    let base = remote::normalize_base(addr);
    match remote::health(&base).await {
        Ok(h) => {
            println!(
                "{}",
                attach_health_line(
                    &base,
                    h.get("project").and_then(|p| p.as_str()).unwrap_or("?"),
                    h.get("uptime_secs").and_then(|u| u.as_u64()).unwrap_or(0),
                )
            );
        }
        Err(e) => {
            let mut msg = connect_unreachable_msg(&base, &e.to_string(), addr);
            let project_dir = resolve_dir(cli);
            if let Some(hint) = super::lockfile::connect_hint(&project_dir) {
                msg.push_str("\n\n");
                msg.push_str(&hint);
            }
            anyhow::bail!(msg);
        }
    }

    println!(
        "{}",
        "Note: the remote agent auto-approves tool prompts.".yellow()
    );

    let session_id = if let Some(id) = session.filter(|s| !s.is_empty()) {
        id.to_string()
    } else {
        remote::create_session(&base).await?
    };
    println!("{}", connect_session_line(&session_id));

    let project_dir = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    config.load_command_files(&project_dir);
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);
    let api_key = get_api_key(&provider, &config).await.unwrap_or_default();

    if !whycodes_tui::tui_available()
        && !(cfg!(test) && std::env::var_os("WHYCODES_TEST_TUI").is_some())
    {
        anyhow::bail!("connect needs a real TUI terminal (not --plain)");
    }

    whycodes_tui::run(whycodes_tui::TuiRunOptions {
        project_dir,
        provider,
        model,
        api_key,
        agent_name,
        max_turns: None,
        initial_prompt: None,
        config,
        resume_session_id: None,
        remote: Some(whycodes_tui::RemoteAttach::new(base, session_id)),
        defer_config_load: false,
        provider_from_cli: true,
        model_from_cli: true,
        agent_from_cli: true,
        update_rx: None,
        inject: Default::default(),
    })
    .await
    .map(|_| ())
}

/// `serve` — Warm multi-session API + local share server.
///
/// Loads config, MCP, plugins, and a workspace file index once so clients
/// reconnect without cold startup cost (jcode/OpenCode daemon spirit).
#[cfg(feature = "server")]
pub(crate) async fn cmd_serve(port: u16, no_takeover: bool) -> anyhow::Result<()> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use whycodes_agent::{PermissionPrompter, QuestionPrompter};
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (lock_guard, takeover) = super::lockfile::acquire_lock(&project_dir, port, no_takeover)?;
    if let Some(holder) = takeover {
        takeover_holder(&holder).await?;
    }
    println!("{}", serve_start_line(port));
    println!("  project: {}", project_dir.display());

    let mut config = Config::load()?;
    config.load_command_files(&project_dir);
    let agent_info = config
        .default_agent()
        .cloned()
        .unwrap_or_else(|| AgentInfo {
            name: "build".to_string(),
            description: "Default".to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        });

    // Permissions: `/v1` can prompt the SDK client; `/api` chat wraps
    // auto-approve so TUI `connect` stays unattended.
    let perm = whycodes_server::perm::PermHub::new();
    let file_index = whycodes_index::WorkspaceIndex::start(vec![project_dir.clone()]);
    let mut question_prompter =
        whycodes_server::perm::ServeQuestionPrompter::new(Arc::clone(&perm));
    question_prompter.timeout = if config.tools.question.timeout_enabled {
        Some(Duration::from_secs(
            config.tools.question.timeout_secs.max(1),
        ))
    } else {
        None
    };
    question_prompter.notify = Some(whycodes_agent::notify::handle_from_config(&config.notify));
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_permission_prompter(Arc::new(whycodes_server::perm::ServePrompter {
            hub: Arc::clone(&perm),
        }) as Arc<dyn PermissionPrompter>)
        .with_question_prompter(Arc::new(question_prompter) as Arc<dyn QuestionPrompter>)
        .with_file_index(file_index)
        .with_plugins(Some(&project_dir))
        .with_mcp(&config)
        .await;
    let (provider, model) = whycodes_server::routes::default_provider_model(&config);
    agent.set_route(&provider, &model);

    let state = whycodes_server::AppState {
        agent: Arc::new(agent),
        config: Arc::new(config),
        project_dir: project_dir.clone(),
        sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        max_turns: None,
        mcp_warm: true,
        index_warm: true,
        started_at: std::time::Instant::now(),
        cancel_flags: Arc::new(std::sync::Mutex::new(HashMap::new())),
        perm,
        session_route: Arc::new(std::sync::Mutex::new(HashMap::new())),
    };

    let router = whycodes_server::create_router(state);
    // Loopback only — this is a local warm daemon, not a public API.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    for line in serve_endpoint_lines(port, &addr.to_string()) {
        println!("{line}");
    }

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            let hint = super::lockfile::connect_hint(&project_dir)
                .unwrap_or_else(|| {
                    format!(
                        "port {port} is already in use (not a WhyCodes serve lock). Try another port or `whycodes connect`."
                    )
                });
            anyhow::bail!("{hint}");
        }
        Err(err) => return Err(err.into()),
    };
    if let Err(err) = super::lockfile::commit_lock(&lock_guard, port) {
        tracing::debug!(error = %err, "serve lock write after bind failed");
    }
    axum::serve(listener, router).await?;
    drop(lock_guard);
    Ok(())
}

#[cfg(feature = "server")]
pub(crate) async fn takeover_holder(holder: &super::lockfile::ServeLock) -> anyhow::Result<()> {
    use std::time::Duration;
    println!(
        "{} Taking over pid {} on port {}",
        "•".bold(),
        holder.pid,
        holder.port
    );
    if let Err(err) = super::lockfile::signal_term(holder.pid) {
        anyhow::bail!(
            "cannot signal pid {} ({err}). Treat as alive; not taking over.",
            holder.pid
        );
    }
    #[cfg(test)]
    let wait = Duration::from_millis(80);
    #[cfg(not(test))]
    let wait = Duration::from_secs(8);
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if !matches!(
            super::lockfile::pid_alive(holder.pid),
            super::lockfile::PidProbe::Alive | super::lockfile::PidProbe::Denied
        ) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            if let Err(err) = super::lockfile::signal_kill(holder.pid) {
                anyhow::bail!("takeover timed out signalling pid {}: {err}", holder.pid);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            if matches!(
                super::lockfile::pid_alive(holder.pid),
                super::lockfile::PidProbe::Alive | super::lockfile::PidProbe::Denied
            ) {
                anyhow::bail!(
                    "takeover timed out: pid {} still running after SIGKILL",
                    holder.pid
                );
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// `web` — Open web UI
pub(crate) fn attach_health_line(base: &str, project: &str, uptime_secs: u64) -> String {
    format!(
        "{} Attached to {base} (project {project}, uptime {uptime_secs}s)",
        "•".bold()
    )
}

pub(crate) fn connect_unreachable_msg(base: &str, err: &str, addr: &str) -> String {
    format!(
        "cannot reach {base}: {err}\n\nStart the daemon first:\n  whycodes serve\nthen:\n  whycodes connect {addr}"
    )
}

pub(crate) fn connect_session_line(session_id: &str) -> String {
    format!("{} session {}", "•".bold(), session_id.cyan())
}

pub(crate) fn serve_start_line(port: u16) -> String {
    format!(
        "{} Starting WhyCodes warm server on http://localhost:{}",
        "•".bold(),
        port.to_string().cyan()
    )
}

pub(crate) fn serve_endpoint_lines(port: u16, addr: &str) -> Vec<String> {
    vec![
        "  Endpoints:".into(),
        "    GET  /v1/health              (protocol handshake)".into(),
        "    GET  /v1/sessions".into(),
        "    POST /v1/sessions".into(),
        "    GET  /v1/sessions/:id".into(),
        "    POST /v1/sessions/:id/run    (SSE v1 event stream)".into(),
        "    POST /v1/sessions/:id/cancel".into(),
        "    POST /v1/sessions/:id/permission".into(),
        "    POST /v1/sessions/:id/question".into(),
        "    GET  /v1/sessions/:id/messages".into(),
        "    GET  /v1/models".into(),
        "    POST /v1/sessions/:id/model".into(),
        "    POST /v1/sessions/:id/rename".into(),
        "    POST /v1/sessions/:id/rewind".into(),
        "    POST /v1/sessions/:id/compact".into(),
        "    GET  /api/health             (TUI attach, legacy)".into(),
        "    GET  /api/tools".into(),
        "    GET  /api/models".into(),
        "    GET  /api/sessions".into(),
        "    POST /api/session/new".into(),
        "    GET  /api/session/:id".into(),
        "    POST /api/session/:id/chat   (SSE, TUI attach)".into(),
        "    GET  /api/shares".into(),
        "    GET  /s/:id[.json|.md]".into(),
        String::new(),
        format!(
            "  Share tip: in TUI run {} then open {}",
            "/share".cyan(),
            format!("http://localhost:{port}/s/<session-id>").cyan()
        ),
        format!("  Bind: {addr} (loopback only). Ctrl+C to stop."),
        String::new(),
    ]
}

pub(crate) fn web_stub_lines() -> Vec<String> {
    vec![
        format!("{} Web UI — not yet implemented.", "🌐".cyan()),
        "Start the server with: whycodes serve".into(),
        "Then open http://localhost:3030 in your browser.".into(),
    ]
}

pub(crate) async fn cmd_web() -> anyhow::Result<()> {
    for line in web_stub_lines() {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
