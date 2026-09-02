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
                "{} Attached to {base} (project {}, uptime {}s)",
                "•".bold(),
                h.get("project").and_then(|p| p.as_str()).unwrap_or("?"),
                h.get("uptime_secs").and_then(|u| u.as_u64()).unwrap_or(0)
            );
        }
        Err(e) => {
            anyhow::bail!(
                "cannot reach {base}: {e}\n\nStart the daemon first:\n  whycodes serve\nthen:\n  whycodes connect {addr}"
            );
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
    println!("{} session {}", "•".bold(), session_id.cyan());

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
        update_rx: None,
    })
    .await
    .map(|_| ())
}

/// `serve` — Warm multi-session API + local share server.
///
/// Loads config, MCP, plugins, and a workspace file index once so clients
/// reconnect without cold startup cost (jcode/OpenCode daemon spirit).
#[cfg(feature = "server")]
pub(crate) async fn cmd_serve(port: u16) -> anyhow::Result<()> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use whycodes_agent::{PermissionPrompter, QuestionPrompter};

    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!(
        "{} Starting WhyCodes warm server on http://localhost:{}",
        "•".bold(),
        port.to_string().cyan()
    );
    println!("  project: {}", project_dir.display());

    let config = Config::load()?;
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
    let agent = Agent::new(agent_info)
        .with_config(&config)
        .with_permission_prompter(Arc::new(whycodes_server::perm::ServePrompter {
            hub: Arc::clone(&perm),
        }) as Arc<dyn PermissionPrompter>)
        .with_question_prompter(Arc::new(whycodes_server::perm::ServeQuestionPrompter {
            hub: Arc::clone(&perm),
        }) as Arc<dyn QuestionPrompter>)
        .with_file_index(file_index)
        .with_plugins(Some(&project_dir))
        .with_mcp(&config)
        .await;

    let state = whycodes_server::AppState {
        agent: Arc::new(agent),
        config: Arc::new(config),
        project_dir,
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
    println!("  Endpoints:");
    println!("    GET  /v1/health              (protocol handshake)");
    println!("    GET  /v1/sessions");
    println!("    POST /v1/sessions");
    println!("    GET  /v1/sessions/:id");
    println!("    POST /v1/sessions/:id/run    (SSE v1 event stream)");
    println!("    POST /v1/sessions/:id/cancel");
    println!("    POST /v1/sessions/:id/permission");
    println!("    POST /v1/sessions/:id/question");
    println!("    GET  /v1/sessions/:id/messages");
    println!("    GET  /v1/models");
    println!("    POST /v1/sessions/:id/model");
    println!("    POST /v1/sessions/:id/rename");
    println!("    POST /v1/sessions/:id/rewind");
    println!("    POST /v1/sessions/:id/compact");
    println!("    GET  /api/health             (TUI attach, legacy)");
    println!("    GET  /api/tools");
    println!("    GET  /api/models");
    println!("    GET  /api/sessions");
    println!("    POST /api/session/new");
    println!("    GET  /api/session/:id");
    println!("    POST /api/session/:id/chat   (SSE, TUI attach)");
    println!("    GET  /api/shares");
    println!("    GET  /s/:id[.json|.md]");
    println!();
    println!(
        "  Share tip: in TUI run {} then open {}",
        "/share".cyan(),
        format!("http://localhost:{port}/s/<session-id>").cyan()
    );
    println!("  Bind: {addr} (loopback only). Ctrl+C to stop.");
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// `web` — Open web UI
pub(crate) async fn cmd_web() -> anyhow::Result<()> {
    println!("{} Web UI — not yet implemented.", "🌐".cyan());
    println!("Start the server with: whycodes serve");
    println!("Then open http://localhost:3030 in your browser.");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn serve_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
