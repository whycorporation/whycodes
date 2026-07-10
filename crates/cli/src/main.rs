use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;

use whycode_agent::agent::Agent;
use whycode_core::config::Config;
use whycode_core::types::{AgentInfo, AgentMode, ModelConfig, PermissionSet, ProviderConfig};

/// Whycode — An AI coding agent built in Rust
#[derive(Parser, Debug)]
#[command(name = "whycode", version, about = "AI-powered coding agent", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Provider to use
    #[arg(short = 'P', long, global = true)]
    pub provider: Option<String>,

    /// Model to use
    #[arg(short = 'm', long, global = true)]
    pub model: Option<String>,

    /// Agent name to use
    #[arg(short = 'a', long = "agent", global = true)]
    pub agent_flag: Option<String>,

    /// Project directory
    #[arg(short = 'd', long, global = true)]
    pub dir: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start an interactive session (default)
    #[command(name = "run")]
    Run {
        /// Optional initial prompt
        prompt: Option<String>,

        /// Maximum conversation turns
        #[arg(short = 't', long, default_value = "25")]
        max_turns: usize,
    },

    /// Generate code from a prompt (non-interactive)
    Generate {
        /// The prompt to generate code for
        prompt: String,

        /// Maximum conversation turns
        #[arg(short = 't', long, default_value = "25")]
        max_turns: usize,
    },

    /// Agent Control Protocol (automated mode)
    Acp,

    /// Create a pull request from current changes
    Pr {
        /// PR title
        #[arg(short, long)]
        title: Option<String>,

        /// Base branch
        #[arg(short, long)]
        base: Option<String>,
    },

    /// GitHub operations
    Github {
        #[command(subcommand)]
        cmd: GithubCmd,
    },

    /// Start API server
    Serve {
        /// Port to listen on
        #[arg(default_value = "3030")]
        port: u16,
    },

    /// Open web UI
    Web,

    /// MCP server management
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },

    /// Provider management (add, list, remove, default)
    Provider {
        #[command(subcommand)]
        cmd: ProviderCmd,
    },

    /// Model management
    Model {
        #[command(subcommand)]
        cmd: ModelCmd,
    },

    /// Agent configuration
    Agent {
        /// Agent name to show/edit
        name: Option<String>,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },

    /// Session management (list, view, delete, rename, share)
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },

    /// Show usage statistics
    Stats,

    /// Show debug information
    Debug,

    /// Self-update
    #[command(name = "upgrade")]
    Upgrade,
}

#[derive(Subcommand, Debug)]
pub enum GithubCmd {
    /// List open pull requests
    Pr {
        #[command(subcommand)]
        action: Option<PrAction>,
    },
    /// Show issue details
    Issue {
        number: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PrAction {
    /// List PRs
    List,
    /// View a PR
    View { number: u64 },
    /// Create a PR
    Create {
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long)]
        base: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum McpCmd {
    /// List configured MCP servers
    List,
    /// Add an MCP server
    Add {
        name: String,
        command: String,
        #[arg(long)]
        args: Option<String>,
    },
    /// Remove an MCP server
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ProviderCmd {
    /// List all configured providers
    List,
    /// Add a new custom provider
    Add {
        /// Provider name
        name: String,
        /// API key for the provider
        #[arg(long)]
        api_key: Option<String>,
        /// Base URL for the provider API
        #[arg(long)]
        base_url: Option<String>,
        /// Custom headers (key=value, comma-separated)
        #[arg(long)]
        headers: Option<String>,
    },
    /// Remove a provider
    Remove {
        /// Provider name to remove
        name: String,
    },
    /// Set the default provider
    Default {
        /// Provider name to set as default
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModelCmd {
    /// List configured models
    List,
    /// Set the default model
    Default {
        /// Provider name
        provider: String,
        /// Model ID
        model: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show current configuration
    Show,
    /// Get a specific configuration value
    Get {
        /// Configuration key (dot-separated, e.g. "default_agent")
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Value to set
        value: String,
    },
    /// Show the configuration file path
    Path,
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// List all sessions
    List,
    /// View a session's details
    View {
        /// Session ID
        id: String,
    },
    /// Delete a session
    Delete {
        /// Session ID
        id: String,
    },
    /// Rename a session
    Rename {
        /// Session ID
        id: String,
        /// New name for the session
        name: String,
    },
    /// Export a session to JSON (shareable)
    Share {
        /// Session ID
        id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    // Determine which command to run; default to Run
    match &cli.command {
        Some(cmd) => dispatch_command(cmd, &cli).await,
        None => {
            // No subcommand → interactive run
            let run_cmd = Commands::Run {
                prompt: None,
                max_turns: 25,
            };
            dispatch_command(&run_cmd, &cli).await
        }
    }
}

async fn dispatch_command(cmd: &Commands, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        Commands::Run { prompt, max_turns } => {
            cmd_run(cli, prompt.as_deref(), *max_turns).await
        }
        Commands::Generate { prompt, max_turns } => {
            cmd_generate(cli, prompt, *max_turns).await
        }
        Commands::Acp => cmd_acp(cli).await,
        Commands::Pr { title, base } => cmd_pr(cli, title.as_deref(), base.as_deref()).await,
        Commands::Github { cmd: gh_cmd } => cmd_github(cli, gh_cmd).await,
        Commands::Serve { port } => cmd_serve(*port).await,
        Commands::Web => cmd_web().await,
        Commands::Mcp { cmd: mcp_cmd } => cmd_mcp(mcp_cmd).await,
        Commands::Provider { cmd: provider_cmd } => cmd_provider(provider_cmd).await,
        Commands::Model { cmd: model_cmd } => cmd_model(model_cmd).await,
        Commands::Agent { name } => cmd_agent(name.as_deref()).await,
        Commands::Config { cmd: config_cmd } => cmd_config(config_cmd).await,
        Commands::Session { cmd: session_cmd } => cmd_session(session_cmd).await,
        Commands::Stats => cmd_stats().await,
        Commands::Debug => cmd_debug().await,
        Commands::Upgrade => cmd_upgrade().await,
    }
}

// ────────────────────────────────────────────────────────────────────────
// Resolve helpers
// ────────────────────────────────────────────────────────────────────────

fn resolve_provider(cli: &Cli, config: &Config) -> String {
    cli.provider
        .clone()
        .or_else(|| {
            config
                .providers
                .keys()
                .next()
                .cloned()
        })
        .unwrap_or_else(|| "anthropic".to_string())
}

fn resolve_model(cli: &Cli, config: &Config) -> String {
    cli.model.clone().unwrap_or_else(|| {
        config
            .default_model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string())
    })
}

fn resolve_agent(cli: &Cli, config: &Config) -> String {
    cli.agent_flag
        .clone()
        .unwrap_or_else(|| config.default_agent.clone())
}

fn resolve_dir(cli: &Cli) -> PathBuf {
    match &cli.dir {
        Some(d) if d != "." => PathBuf::from(d),
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn get_api_key(provider: &str, config: &Config) -> Option<String> {
    let env_var = provider_env_var(provider);
    if let Ok(key) = std::env::var(&env_var) {
        if !key.is_empty() {
            return Some(key);
        }
    }
    if let Some(pc) = config.get_provider(provider) {
        if let Some(key) = &pc.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
    }
    // Fallback to generic env vars
    if provider == "openai" {
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            return Some(key);
        }
    }
    None
}

fn provider_env_var(provider: &str) -> String {
    format!("{}_API_KEY", provider.to_uppercase())
}

fn open_db() -> anyhow::Result<whycode_storage::db::Database> {
    let data_dir = Config::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("whycode.db");
    whycode_storage::db::Database::open(&db_path.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("Failed to open database: {}", e))
}

fn agent_info_for(cli: &Cli, config: &Config) -> AgentInfo {
    let agent_name = resolve_agent(cli, config);
    let provider = resolve_provider(cli, config);
    let model = resolve_model(cli, config);

    config.get_agent(&agent_name).cloned().unwrap_or_else(|| {
        AgentInfo {
            name: "build".to_string(),
            description: "Default build agent".to_string(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
            },
            model: Some(ModelConfig {
                model_id: model,
                provider_id: provider,
                max_tokens: None,
                temperature: None,
                top_p: None,
                thinking: None,
                supports_tools: Some(true),
                supports_images: None,
            }),
            system_prompt: None,
            temperature: None,
            top_p: None,
        }
    })
}

// ────────────────────────────────────────────────────────────────────────
// Command implementations
// ────────────────────────────────────────────────────────────────────────

/// `run` — Start an interactive session
async fn cmd_run(cli: &Cli, prompt: Option<&str>, max_turns: usize) -> anyhow::Result<()> {
    let config = Config::load()?;
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);
    let project_dir = resolve_dir(cli);

    let api_key = get_api_key(&provider, &config).unwrap_or_else(|| {
        eprintln!(
            "{} {}",
            "Error:".red().bold(),
            format!(
                "No API key for '{}'. Set {} env var or configure a provider.",
                provider,
                provider_env_var(&provider)
            )
        );
        std::process::exit(1);
    });

    let agent_info = agent_info_for(cli, &config);
    let system_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));

    let agent = Agent::new(agent_info);
    let mut session = whycode_session::session::Session::new(project_dir.clone(), system_prompt);

    println!(
        "{} {}",
        "Whycode".cyan().bold(),
        format!("[agent={}, provider={}, model={}]", agent_name, provider, model).dimmed()
    );
    println!(
        "{} {}",
        "Project:".dimmed(),
        project_dir.display().to_string().dimmed()
    );
    println!();

    if let Some(prompt) = prompt {
        if prompt.is_empty() {
            eprintln!("{}", "Error: empty prompt".red());
            return Ok(());
        }
        session.add_user_message(prompt);
        match agent
            .run_turn(&mut session, &provider, &model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if !response.is_empty() {
                    println!("\n{}", response);
                }
            }
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                return Err(anyhow::anyhow!("{}", e));
            }
        }
        return Ok(());
    }

    println!(
        "{}",
        "Interactive mode. Type /help for commands, /exit to quit.".dimmed()
    );
    loop {
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            break;
        }
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }

        if input.starts_with('/') {
            match input.as_str() {
                "/exit" | "/quit" | "/q" => break,
                "/help" | "/h" => {
                    println!("  /exit, /quit, /q — Exit");
                    println!("  /help, /h       — Help");
                    println!("  /clear          — Clear session");
                    println!("  /info           — Session info");
                    continue;
                }
                "/clear" => {
                    session = whycode_session::session::Session::new(
                        project_dir.clone(),
                        agent.system_prompt(),
                    );
                    println!("Cleared.");
                    continue;
                }
                "/info" => {
                    let i = session.info();
                    println!(
                        "ID: {} | Messages: {} | {}",
                        i.id,
                        i.message_count,
                        i.created_at.format("%H:%M:%S")
                    );
                    continue;
                }
                cmd => {
                    println!("Unknown: {}. /help for commands.", cmd);
                    continue;
                }
            }
        }

        session.add_user_message(&input);
        match agent
            .run_turn(&mut session, &provider, &model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if !response.is_empty() {
                    println!("\n{}", response);
                }
                println!();
            }
            Err(e) => eprintln!("{} {}", "Error:".red().bold(), e),
        }
    }
    println!("{}", "Goodbye!".cyan());
    Ok(())
}

/// `generate` — Non-interactive code generation
async fn cmd_generate(cli: &Cli, prompt: &str, max_turns: usize) -> anyhow::Result<()> {
    let config = Config::load()?;
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);
    let project_dir = resolve_dir(cli);

    let api_key = match get_api_key(&provider, &config) {
        Some(k) => k,
        None => {
            eprintln!(
                "{} No API key for provider '{}'. Set {} env var.",
                "Error:".red().bold(),
                provider,
                provider_env_var(&provider)
            );
            std::process::exit(1);
        }
    };

    let agent_info = agent_info_for(cli, &config);
    let system_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));

    let agent = Agent::new(agent_info);
    let mut session = whycode_session::session::Session::new(project_dir, system_prompt);

    println!(
        "{} Generating with {}/{}...",
        "⚡".bold(),
        provider.dimmed(),
        model.dimmed()
    );

    session.add_user_message(prompt);
    match agent
        .run_turn(&mut session, &provider, &model, &api_key, max_turns)
        .await
    {
        Ok(response) => {
            println!("{}", response);
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
            return Err(anyhow::anyhow!("{}", e));
        }
    }

    Ok(())
}

/// `acp` — Agent Control Protocol (automated mode placeholder)
async fn cmd_acp(_cli: &Cli) -> anyhow::Result<()> {
    println!("{} ACP mode — not yet implemented.", "ℹ".cyan());
    println!("ACP will enable automated agent-to-agent communication.");
    Ok(())
}

/// `pr` — Create a pull request from current changes
async fn cmd_pr(_cli: &Cli, title: Option<&str>, base: Option<&str>) -> anyhow::Result<()> {
    let title = title.unwrap_or("Auto-generated PR");
    let base = base.unwrap_or("main");

    println!(
        "{} Creating pull request...",
        "🔀".bold()
    );
    println!("  Title: {}", title.cyan());
    println!("  Base:  {}", base.cyan());
    println!();

    // Try to use gh CLI if available
    let status = std::process::Command::new("gh")
        .args(["pr", "create", "--title", title, "--base", base, "--fill"])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("{} PR created successfully!", "✓".green());
        }
        _ => {
            println!(
                "{} Could not create PR. Install GitHub CLI: {}",
                "⚠".yellow(),
                "https://cli.github.com/".cyan()
            );
            println!("  Or run: gh pr create --title \"{}\" --base \"{}\"", title, base);
        }
    }

    Ok(())
}

/// `github` — GitHub operations
async fn cmd_github(_cli: &Cli, cmd: &GithubCmd) -> anyhow::Result<()> {
    match cmd {
        GithubCmd::Pr { action } => {
            match action {
                Some(PrAction::List) | None => {
                    println!("{} Listing pull requests...", "📋".bold());
                    let status = std::process::Command::new("gh")
                        .args(["pr", "list"])
                        .status();
                    if status.is_err() || !status.unwrap().success() {
                        println!(
                            "{} GitHub CLI not available. Install: {}",
                            "⚠".yellow(),
                            "https://cli.github.com/".cyan()
                        );
                    }
                }
                Some(PrAction::View { number }) => {
                    println!("{} Viewing PR #{}...", "👁".bold(), number);
                    let status = std::process::Command::new("gh")
                        .args(["pr", "view", &number.to_string()])
                        .status();
                    if status.is_err() || !status.unwrap().success() {
                        println!("{} Could not view PR.", "⚠".yellow());
                    }
                }
                Some(PrAction::Create { title, base }) => {
                    let title = title.as_deref().unwrap_or("Auto PR");
                    let base = base.as_deref().unwrap_or("main");
                    let status = std::process::Command::new("gh")
                        .args([
                            "pr", "create", "--title", title, "--base", base, "--fill",
                        ])
                        .status();
                    match status {
                        Ok(s) if s.success() => {
                            println!("{} PR created!", "✓".green());
                        }
                        _ => {
                            println!("{} Could not create PR.", "⚠".yellow());
                        }
                    }
                }
            }
        }
        GithubCmd::Issue { number } => {
            if let Some(n) = number {
                println!("{} Viewing issue #{}...", "📝".bold(), n);
                let _ = std::process::Command::new("gh")
                    .args(["issue", "view", &n.to_string()])
                    .status();
            } else {
                println!("{} Listing issues...", "📝".bold());
                let _ = std::process::Command::new("gh")
                    .args(["issue", "list"])
                    .status();
            }
        }
    }
    Ok(())
}

/// `serve` — Start API server
async fn cmd_serve(port: u16) -> anyhow::Result<()> {
    println!(
        "{} Starting Whycode API server on http://localhost:{}",
        "🚀".bold(),
        port.to_string().cyan()
    );

    let config = Config::load()?;
    let agent_info = config.default_agent().cloned().unwrap_or_else(|| AgentInfo {
        name: "build".to_string(),
        description: "Default".to_string(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    });
    let agent = Agent::new(agent_info);

    let state = whycode_server::AppState {
        agent: std::sync::Arc::new(agent),
        config: std::sync::Arc::new(config),
    };

    let router = whycode_server::create_router(state);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    println!("  Endpoints:");
    println!("    GET  /api/health");
    println!("    GET  /api/tools");
    println!("    GET  /api/models");
    println!("    GET  /api/sessions");
    println!("    POST /api/session/new");
    println!("    POST /api/session/:id/chat");
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// `web` — Open web UI
async fn cmd_web() -> anyhow::Result<()> {
    println!("{} Web UI — not yet implemented.", "🌐".cyan());
    println!("Start the server with: whycode serve");
    println!("Then open http://localhost:3030 in your browser.");
    Ok(())
}

/// `mcp` — MCP server management
async fn cmd_mcp(cmd: &McpCmd) -> anyhow::Result<()> {
    match cmd {
        McpCmd::List => {
            println!("{} Configured MCP servers:", "🔌".bold());
            println!("  (none configured yet)");
            println!();
            println!("Add an MCP server:");
            println!("  whycode mcp add <name> <command> [--args args]");
        }
        McpCmd::Add {
            name,
            command,
            args,
        } => {
            println!(
                "{} Adding MCP server '{}' with command '{}'",
                "➕".bold(),
                name.cyan(),
                command.cyan()
            );
            if let Some(a) = args {
                println!("  Args: {}", a);
            }
            println!("  (MCP server configuration persistence not yet implemented)");
        }
        McpCmd::Remove { name } => {
            println!(
                "{} Removing MCP server '{}'",
                "➖".bold(),
                name.cyan()
            );
            println!("  (MCP server management not yet fully implemented)");
        }
    }
    Ok(())
}

/// `provider` — Provider management
async fn cmd_provider(cmd: &ProviderCmd) -> anyhow::Result<()> {
    let mut config = Config::load()?;

    match cmd {
        ProviderCmd::List => {
            if config.providers.is_empty() {
                println!("{} No providers configured.", "ℹ".cyan());
                println!();
                println!("Add a provider:");
                println!("  whycode provider add <name> --api-key <key> --base-url <url>");
                println!();
                println!("Built-in providers supported: openai, anthropic, deepseek, google, groq, xai");
            } else {
                println!("{} Configured providers:", "🔑".bold());
                for (name, provider) in &config.providers {
                    let key_status = if provider.api_key.is_some() {
                        "✓".green()
                    } else {
                        "✗".red()
                    };
                    let url = provider
                        .base_url
                        .as_deref()
                        .or(provider.api_base.as_deref())
                        .unwrap_or("(default)");
                    println!(
                        "  {} {}  API key: {}  Base URL: {}",
                        name.cyan(),
                        key_status,
                        if provider.api_key.is_some() {
                            "set"
                        } else {
                            "not set"
                        },
                        url
                    );
                }
            }
        }
        ProviderCmd::Add {
            name,
            api_key,
            base_url,
            headers,
        } => {
            let mut headers_map = std::collections::HashMap::new();
            if let Some(h) = headers {
                for pair in h.split(',') {
                    let parts: Vec<&str> = pair.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        headers_map
                            .insert(parts[0].trim().to_string(), parts[1].trim().to_string());
                    }
                }
            }

            let provider = ProviderConfig {
                name: name.clone(),
                api_key: api_key.clone(),
                api_base: None,
                base_url: base_url.clone(),
                headers: if headers_map.is_empty() {
                    None
                } else {
                    Some(headers_map)
                },
                models: Vec::new(),
                extra: std::collections::HashMap::new(),
            };

            if config.providers.contains_key(name) {
                println!(
                    "{} Provider '{}' already exists. Updating...",
                    "⚠".yellow(),
                    name.cyan()
                );
            }

            config.providers.insert(name.clone(), provider);
            config.save()?;
            println!(
                "{} Provider '{}' {}",
                "✓".green(),
                name.cyan(),
                if api_key.is_some() {
                    "added with API key"
                } else {
                    "added (no API key — set via env var or --api-key)"
                }
            );
        }
        ProviderCmd::Remove { name } => {
            if config.providers.remove(name).is_some() {
                config.save()?;
                println!(
                    "{} Provider '{}' removed.",
                    "✓".green(),
                    name.cyan()
                );
            } else {
                eprintln!(
                    "{} Provider '{}' not found.",
                    "✗".red(),
                    name.cyan()
                );
            }
        }
        ProviderCmd::Default { name } => {
            if config.providers.contains_key(name) {
                // Save provider name as metadata
                config.save()?;
                println!(
                    "{} Default provider set to '{}'.",
                    "✓".green(),
                    name.cyan()
                );
                println!("  Use: whycode -P {} ...", name);
            } else {
                eprintln!(
                    "{} Provider '{}' not found. Add it first: whycode provider add {}",
                    "✗".red(),
                    name.cyan(),
                    name
                );
            }
        }
    }

    Ok(())
}

/// `model` — Model management
async fn cmd_model(cmd: &ModelCmd) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        ModelCmd::List => {
            if config.models.is_empty() {
                println!("{} No models configured.", "ℹ".cyan());
                println!();
                println!("Configure models in your config file:");
                if let Ok(path) = Config::default_path() {
                    println!("  {}", path.display());
                }
            } else {
                println!("{} Configured models:", "🤖".bold());
                for (key, model) in &config.models {
                    println!(
                        "  {} → {}/{}",
                        key.cyan(),
                        model.provider_id.dimmed(),
                        model.model_id
                    );
                    if let Some(max_tok) = model.max_tokens {
                        println!("    max_tokens: {}", max_tok);
                    }
                }
            }
        }
        ModelCmd::Default { provider, model } => {
            let mut config = config.clone();
            config.default_model = Some(ModelConfig {
                model_id: model.clone(),
                provider_id: provider.clone(),
                max_tokens: None,
                temperature: None,
                top_p: None,
                thinking: None,
                supports_tools: Some(true),
                supports_images: None,
            });
            config.save()?;
            println!(
                "{} Default model set to {}/{}",
                "✓".green(),
                provider.cyan(),
                model.cyan()
            );
        }
    }

    Ok(())
}

/// `agent` — Agent configuration
async fn cmd_agent(name: Option<&str>) -> anyhow::Result<()> {
    let config = Config::load()?;

    match name {
        Some(name) => {
            if let Some(agent) = config.get_agent(name) {
                println!("{} Agent: {}", "🤖".bold(), agent.name.cyan().bold());
                println!("  Description: {}", agent.description);
                println!("  Mode: {:?}", agent.mode);
                println!("  File writes: {}", agent.permission.allow_file_writes);
                println!("  Network:     {}", agent.permission.allow_network);
                println!("  Shell:       {}", agent.permission.allow_shell);
                if let Some(ref tools) = agent.permission.allowed_tools {
                    println!("  Allowed tools: {:?}", tools);
                }
                if let Some(ref tools) = agent.permission.denied_tools {
                    println!("  Denied tools: {:?}", tools);
                }
                if let Some(ref model) = agent.model {
                    println!(
                        "  Model: {}/{}",
                        model.provider_id, model.model_id
                    );
                }
            } else {
                eprintln!("{} Agent '{}' not found.", "✗".red(), name);
                println!();
                println!("Available agents:");
                for a in &config.agents {
                    println!("  {} — {}", a.name.cyan(), a.description);
                }
            }
        }
        None => {
            println!("{} Available agents:", "🤖".bold());
            for agent in &config.agents {
                let default_marker = if agent.name == config.default_agent {
                    " (default)".dimmed()
                } else {
                    "".into()
                };
                println!(
                    "  {}{} — {}",
                    agent.name.cyan(),
                    default_marker,
                    agent.description
                );
            }
            if config.agents.is_empty() {
                println!("  (no agents configured)");
            }
        }
    }

    Ok(())
}

/// `config` — Configuration management
async fn cmd_config(cmd: &ConfigCmd) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        ConfigCmd::Show => {
            let config_path = Config::default_path()?;
            println!(
                "{} Config path: {}",
                "⚙".bold(),
                config_path.display().to_string().cyan()
            );
            println!();
            let text = toml::to_string_pretty(&config)?;
            println!("{}", text);
        }
        ConfigCmd::Get { key } => {
            match get_config_value(&config, key) {
                Some(val) => println!("{}", val),
                None => eprintln!("{} Key '{}' not found.", "✗".red(), key),
            }
        }
        ConfigCmd::Set { key, value } => {
            let mut config = config.clone();
            set_config_value(&mut config, key, value)?;
            config.save()?;
            println!(
                "{} Set '{}' = '{}'",
                "✓".green(),
                key.cyan(),
                value
            );
        }
        ConfigCmd::Path => {
            let config_path = Config::default_path()?;
            println!("{}", config_path.display());
        }
    }

    Ok(())
}

fn get_config_value(config: &Config, key: &str) -> Option<String> {
    match key {
        "default_agent" => Some(config.default_agent.clone()),
        "project_path" => config.general.project_path.as_ref().map(|p| p.display().to_string()),
        "log_level" => config.general.log_level.clone(),
        _ => None,
    }
}

fn set_config_value(config: &mut Config, key: &str, value: &str) -> anyhow::Result<()> {
    match key {
        "default_agent" => {
            config.default_agent = value.to_string();
        }
        "project_path" => {
            config.general.project_path = Some(PathBuf::from(value));
        }
        "log_level" => {
            config.general.log_level = Some(value.to_string());
        }
        _ => {
            anyhow::bail!("Unknown config key: {}. Supported: default_agent, project_path, log_level", key);
        }
    }
    Ok(())
}

/// `session` — Session management
async fn cmd_session(cmd: &SessionCmd) -> anyhow::Result<()> {
    let db = open_db()?;

    match cmd {
        SessionCmd::List => {
            let sessions = db
                .list_sessions()
                .map_err(|e| anyhow::anyhow!("Failed to list sessions: {}", e))?;

            if sessions.is_empty() {
                println!("{} No sessions found.", "ℹ".cyan());
                println!("Start a session with: whycode run");
            } else {
                println!("{} Sessions:", "📋".bold());
                for s in &sessions {
                    let msg_count = db.message_count(&s.id).unwrap_or(0);
                    println!(
                        "  {} — {} ({} messages)",
                        s.id.cyan(),
                        s.title,
                        msg_count
                    );
                    println!(
                        "    Created: {}  Updated: {}",
                        s.created_at, s.updated_at
                    );
                    if !s.project_path.is_empty() && s.project_path != "/" {
                        println!("    Project: {}", s.project_path);
                    }
                }
            }
        }
        SessionCmd::View { id } => {
            match db
                .get_session(id)
                .map_err(|e| anyhow::anyhow!("{}", e))?
            {
                Some(s) => {
                    let msg_count = db.message_count(&s.id).unwrap_or(0);
                    println!("{} Session: {}", "📋".bold(), s.id.cyan());
                    println!("  Title:     {}", s.title);
                    println!("  Created:   {}", s.created_at);
                    println!("  Updated:   {}", s.updated_at);
                    println!("  Messages:  {}", msg_count);
                    println!("  Project:   {}", s.project_path);

                    // Show recent messages
                    let messages = db.get_messages(id).unwrap_or_default();
                    if !messages.is_empty() {
                        println!("  --- Messages ---");
                        for msg in messages.iter().rev().take(10).rev() {
                            println!(
                                "    [{}] {}: {}",
                                msg.role,
                                msg.created_at,
                                truncate_str(&msg.content, 120)
                            );
                        }
                    }
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Delete { id } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    db.delete_session(id)?;
                    println!(
                        "{} Session '{}' ({}) deleted.",
                        "✓".green(),
                        id.cyan(),
                        s.title
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Rename { id, name } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    db.update_title(id, name)?;
                    println!(
                        "{} Session '{}' renamed from '{}' to '{}'.",
                        "✓".green(),
                        id.cyan(),
                        s.title,
                        name.cyan()
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Share { id } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
                Some(s) => {
                    let messages = db.get_messages(id).unwrap_or_default();
                    let share_data = serde_json::json!({
                        "session": {
                            "id": s.id,
                            "title": s.title,
                            "created_at": s.created_at,
                            "updated_at": s.updated_at,
                            "project_path": s.project_path,
                        },
                        "messages": messages.iter().map(|m| {
                            serde_json::json!({
                                "id": m.id,
                                "role": m.role,
                                "content": m.content,
                                "created_at": m.created_at,
                            })
                        }).collect::<Vec<_>>(),
                    });

                    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
                    let shares_dir = data_dir.join("shares");
                    std::fs::create_dir_all(&shares_dir)?;
                    let share_path = shares_dir.join(format!("{}.json", id));

                    let json = serde_json::to_string_pretty(&share_data)?;
                    std::fs::write(&share_path, &json)?;

                    println!(
                        "{} Session exported to: {}",
                        "✓".green(),
                        share_path.display().to_string().cyan()
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
    }

    Ok(())
}

/// `stats` — Show usage statistics
async fn cmd_stats() -> anyhow::Result<()> {
    let db = match open_db() {
        Ok(d) => d,
        Err(_) => {
            println!("{} No statistics database found.", "ℹ".cyan());
            println!("Stats are collected as you use whycode.");
            return Ok(());
        }
    };

    let sessions = db.list_sessions().unwrap_or_default();
    let session_count = sessions.len();

    let mut total_messages = 0usize;
    for s in &sessions {
        total_messages += db.message_count(&s.id).unwrap_or(0);
    }

    println!("{} Usage Statistics:", "📊".bold());
    println!("  Sessions:  {}", session_count);
    println!("  Messages:  {}", total_messages);

    // Estimate token counts (rough: average 500 tokens/message)
    let estimated_tokens = total_messages * 500;
    println!(
        "  Est. tokens: ~{} (input+output, rough estimate)",
        estimated_tokens
    );

    if session_count > 0 {
        let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
        let db_path = data_dir.join("whycode.db");
        if let Ok(meta) = std::fs::metadata(&db_path) {
            println!(
                "  DB size:    {} bytes",
                meta.len()
            );
        }
    }

    Ok(())
}

/// `debug` — Show debug information
async fn cmd_debug() -> anyhow::Result<()> {
    println!("{} Debug Information:", "🔧".bold());
    println!(
        "  Version:     {}",
        env!("CARGO_PKG_VERSION").cyan()
    );

    // Config path
    match Config::default_path() {
        Ok(p) => {
            let exists = if p.exists() { "✓".green() } else { "✗ (not found)".red() };
            println!("  Config:      {} {}", p.display(), exists);
        }
        Err(e) => {
            println!("  Config:      error: {}", e);
        }
    }

    // Data directory
    match Config::data_dir() {
        Ok(p) => {
            let exists = if p.exists() { "✓".green() } else { "✗".red() };
            println!("  Data dir:    {} {}", p.display(), exists);
        }
        Err(e) => {
            println!("  Data dir:    error: {}", e);
        }
    }

    // Current directory
    match std::env::current_dir() {
        Ok(p) => println!("  CWD:         {}", p.display()),
        Err(e) => println!("  CWD:         error: {}", e),
    }

    // Home directory
    if let Ok(home) = std::env::var("HOME") {
        println!("  HOME:        {}", home);
    }

    // Rust toolchain
    if let Ok(rustc) = std::process::Command::new("rustc").arg("--version").output() {
        let ver = String::from_utf8_lossy(&rustc.stdout).trim().to_string();
        println!("  Rust:        {}", ver);
    }

    // Git info
    if let Ok(git) = std::process::Command::new("git").arg("--version").output() {
        let ver = String::from_utf8_lossy(&git.stdout).trim().to_string();
        println!("  Git:         {}", ver);
    }

    // Relevant environment variables
    println!("  Environment:");
    for var in &[
        "WHYCODE_PROVIDER",
        "WHYCODE_MODEL",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "DEEPSEEK_API_KEY",
        "GOOGLE_API_KEY",
        "XAI_API_KEY",
    ] {
        match std::env::var(var) {
            Ok(val) => {
                let masked = if val.len() > 8 {
                    format!("{}...{}", &val[..4], &val[val.len() - 4..])
                } else {
                    "***".to_string()
                };
                println!("    {} = {} (set)", var, masked.dimmed());
            }
            Err(_) => {
                println!("    {} = (not set)", var.dimmed());
            }
        }
    }

    Ok(())
}

/// `upgrade` — Show upgrade instructions
async fn cmd_upgrade() -> anyhow::Result<()> {
    println!("{} Whycode Upgrade", "⬆".bold());
    println!();
    println!(
        "  Current version: {}",
        env!("CARGO_PKG_VERSION").cyan()
    );
    println!();
    println!("  To upgrade, re-install from source or use your package manager.");
    println!();
    println!("  From source:");
    println!(
        "    {}",
        "git clone https://github.com/whycorporation/whycode.git".dimmed()
    );
    println!(
        "    {}",
        "cd whycode && cargo install --path crates/cli".dimmed()
    );
    println!();
    println!("  Check for latest release:");
    println!(
        "    {}",
        "https://github.com/whycorporation/whycode/releases".cyan()
    );

    Ok(())
}

// ────────────────────────────────────────────────────────────────────────
// Utility helpers
// ────────────────────────────────────────────────────────────────────────

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated = s.chars().take(max_len - 3).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}
