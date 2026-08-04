mod upgrade;

use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;
use std::sync::Arc;

use whycode_agent::agent::Agent;
use whycode_agent::events::{TurnEvent, new_cancel_flag};
use whycode_agent::permission::AutoApprovePrompter;
use whycode_config::Config;
use whycode_core::types::{AgentInfo, AgentMode, ModelConfig, PermissionSet, ProviderConfig};
use whycode_protocol::{CiEvent, OutputFormat, ResultMeta};

/// Crate version only (semver from Cargo.toml).
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full version string: `0.1.0 (abc1234 2026-08-04)`.
///
/// Git hash and build date come from `build.rs` so release binaries and
/// `whycode --version` / install smoke checks identify an exact build.
const VERSION_LONG: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("WHYCODE_GIT_HASH"),
    " ",
    env!("WHYCODE_BUILD_DATE"),
    ")"
);

/// Whycode — An AI coding agent built in Rust
#[derive(Parser, Debug)]
#[command(
    name = "whycode",
    version = VERSION_LONG,
    about = "AI-powered coding agent",
    long_about = None
)]
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

    /// Use plain stdin REPL instead of the full-screen TUI
    #[arg(long, global = true)]
    pub plain: bool,

    /// Write debug logs under the data dir (`debug/whycode-*.log` + `debug/latest.log`)
    #[arg(long, global = true)]
    pub debug: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start an interactive session (default)
    #[command(name = "run")]
    Run {
        /// Optional initial prompt (with --format json|stream-json this is one-shot CI mode)
        prompt: Option<String>,

        /// Maximum conversation turns
        #[arg(short = 't', long, default_value = "25")]
        max_turns: usize,

        /// Output format for headless / CI: text (default), json, or stream-json
        #[arg(
            long = "format",
            visible_alias = "output-format",
            value_parser = parse_output_format,
            default_value = "text"
        )]
        format: OutputFormat,
    },

    /// Generate code from a prompt (non-interactive)
    Generate {
        /// The prompt to generate code for
        prompt: String,

        /// Maximum conversation turns
        #[arg(short = 't', long, default_value = "25")]
        max_turns: usize,

        /// Output format for headless / CI: text (default), json, or stream-json
        #[arg(
            long = "format",
            visible_alias = "output-format",
            value_parser = parse_output_format,
            default_value = "text"
        )]
        format: OutputFormat,
    },

    /// Agent Client Protocol (stub; after product launch)
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
    Issue { number: Option<u64> },
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
    /// Add an MCP server (stdio via command, or remote via --url)
    Add {
        /// Server name (tools bind as `{name}_{tool}`)
        name: String,
        /// Local command to spawn (stdio). Omit when using `--url`.
        command: Option<String>,
        /// Arguments for the local command
        #[arg(long)]
        args: Option<String>,
        /// Remote MCP endpoint URL (Streamable HTTP or legacy SSE)
        #[arg(long)]
        url: Option<String>,
        /// Transport: `stdio` | `http` | `sse` | `auto` (default: inferred)
        #[arg(long = "type")]
        transport: Option<String>,
        /// Extra HTTP header for remote servers (`Key: Value`). Repeatable.
        #[arg(long = "header")]
        headers: Vec<String>,
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
    // First statement: everything after it is time a user waits for, and the
    // first-frame benchmark measures from here.
    whycode_tui::bench::mark_process_start();

    // Hosts that capture/close stdout (IDE, wrappers: stdout_tty=false) will
    // SIGPIPE-kill the process on any accidental write to stdout. Ignore it so
    // the TUI (which draws on /dev/tty) keeps running.
    ignore_sigpipe();

    let cli = Cli::parse();

    // Grok-style logging: always-on JSONL under data_dir/logs/, optional file,
    // panic → data_dir/crash/. TUI keeps stderr quiet so the alternate screen
    // is not corrupted (use --debug or WHYCODE_LOG_FILE to capture human logs).
    init_logging(&cli);

    // Determine which command to run; default to Run
    let result = match &cli.command {
        Some(cmd) => dispatch_command(cmd, &cli).await,
        None => {
            // No subcommand → interactive run
            let run_cmd = Commands::Run {
                prompt: None,
                max_turns: 25,
                format: OutputFormat::Text,
            };
            dispatch_command(&run_cmd, &cli).await
        }
    };

    if let Err(ref e) = result {
        // Always land in unified.jsonl — TUI mode often silences stderr.
        whycode_core::logging::emit(
            "whycode",
            "error",
            "main.exit_error",
            Some(serde_json::json!({ "error": e.to_string() })),
        );
        // Print once here; exit 1 so CI / scripts can branch on failure.
        // (Returning Ok would make `anyhow` silent and the process succeed.)
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
    result
}

/// Ignore SIGPIPE so a closed stdout pipe cannot kill the process.
#[cfg(unix)]
fn ignore_sigpipe() {
    // libc::SIG_IGN without pulling libc as a hard dep for this one call.
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_IGN: usize = 1;
    unsafe {
        let _ = signal(SIGPIPE, SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_sigpipe() {}

/// Resolve data dir + env/config filters and install the process logger.
fn init_logging(cli: &Cli) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let log_file = std::env::var_os("WHYCODE_LOG_FILE").map(PathBuf::from);
    let log_level = std::env::var("WHYCODE_LOG_LEVEL").ok().or_else(|| {
        Config::load()
            .ok()
            .and_then(|c| c.general.log_level.clone())
    });

    // Full-screen TUI is the default for Run / bare invoke without --plain.
    let is_tui = !cli.plain && matches!(&cli.command, None | Some(Commands::Run { .. }));

    let opts = whycode_core::logging::InitOptions {
        data_dir,
        log_level,
        log_file,
        debug: cli.debug,
        // Keep stderr free while the alternate screen is active unless the
        // user asked for --debug (file still gets the firehose either way).
        with_stderr: !is_tui || cli.debug,
    };

    if let Err(e) = whycode_core::logging::init(opts) {
        eprintln!("warning: failed to initialize logging: {e}");
        // Last-resort so tracing macros still work somewhere.
        let _ = tracing_subscriber::fmt::try_init();
    }
}

async fn dispatch_command(cmd: &Commands, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        Commands::Run {
            prompt,
            max_turns,
            format,
        } => cmd_run(cli, prompt.as_deref(), *max_turns, *format).await,
        Commands::Generate {
            prompt,
            max_turns,
            format,
        } => cmd_generate(cli, prompt, *max_turns, *format).await,
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
                .default_model
                .as_ref()
                .map(|m| m.provider_id.clone())
                .filter(|id| !id.is_empty())
        })
        .or_else(|| config.providers.keys().next().cloned())
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
    if let Ok(key) = std::env::var(&env_var)
        && !key.is_empty()
    {
        return Some(key);
    }
    if let Some(pc) = config.get_provider(provider)
        && let Some(key) = &pc.api_key
        && !key.is_empty()
    {
        return Some(key.clone());
    }
    // Fallback to generic env vars
    if provider == "openai"
        && let Ok(key) = std::env::var("OPENAI_API_KEY")
    {
        return Some(key);
    }
    None
}

fn provider_env_var(provider: &str) -> String {
    format!("{}_API_KEY", provider.to_uppercase())
}

/// True when opening the database failed because there is nothing there yet,
/// as opposed to failing for a reason the user should hear about.
fn is_missing_database(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        e.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
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

    config
        .get_agent(&agent_name)
        .cloned()
        .unwrap_or_else(|| AgentInfo {
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
                rules: Default::default(),
            },
            model: Some(ModelConfig {
                model_id: model,
                provider_id: provider,
                max_tokens: None,
                context_window: None,
                temperature: None,
                top_p: None,
                thinking: None,
                supports_tools: Some(true),
                supports_images: None,
            }),
            system_prompt: None,
            temperature: None,
            top_p: None,
        })
}

// ────────────────────────────────────────────────────────────────────────
// Command implementations
// ────────────────────────────────────────────────────────────────────────

/// `run` — Start an interactive session (TUI by default, `--plain` for readline REPL).
/// With `--format json|stream-json` and a prompt, runs one-shot CI mode instead.
async fn cmd_run(
    cli: &Cli,
    prompt: Option<&str>,
    max_turns: usize,
    format: OutputFormat,
) -> anyhow::Result<()> {
    // Structured output is headless-only; needs a prompt.
    if format.is_structured() {
        let Some(prompt) = prompt.filter(|p| !p.is_empty()) else {
            anyhow::bail!(
                "--format {format} requires a non-empty prompt \
                 (e.g. `whycode run \"…\" --format {format}` or `whycode generate \"…\" --format {format}`)"
            );
        };
        return cmd_generate(cli, prompt, max_turns, format).await;
    }

    let project_dir_early = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir_early)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);
    let project_dir = resolve_dir(cli);
    config.load_command_files(&project_dir);

    // Interactive mode always starts (OpenCode-style). API key is optional until
    // the user actually sends a prompt that needs the LLM.
    let mut api_key = get_api_key(&provider, &config).unwrap_or_default();

    // Full-screen TUI unless --plain / WHYCODE_PLAIN.
    // Hosts that capture stdout (IDE, some wrappers) report stdout_tty=false
    // while still having a controlling terminal — tui_available() opens
    // /dev/tty in that case so the TUI still works.
    let force_plain = cli.plain || std::env::var_os("WHYCODE_PLAIN").is_some();
    let use_tui = !force_plain && whycode_tui::tui_available();
    if !use_tui && !force_plain {
        use std::io::IsTerminal;
        eprintln!(
            "whycode: no interactive terminal \
             (stdin_tty={} stdout_tty={} /dev/tty unavailable).\n\
             Falling back to plain mode. Use a real terminal, or pass --plain.",
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        );
    }
    if use_tui {
        return whycode_tui::run(whycode_tui::TuiRunOptions {
            project_dir,
            provider,
            model,
            api_key,
            agent_name,
            max_turns,
            initial_prompt: prompt.map(|s| s.to_string()),
            config,
        })
        .await
        .map_err(|e| {
            // Crossterm ENXIO / similar — make the message actionable.
            let msg = e.to_string();
            if msg.contains("No such device")
                || msg.contains("os error 6")
                || msg.contains("not a terminal")
            {
                anyhow::anyhow!(
                    "{msg}\n\n\
                     TUI needs a real terminal. Run in a terminal emulator, or:\n\
                       whycode --plain"
                )
            } else {
                e
            }
        });
    }

    let agent_info = {
        let mut info = agent_info_for(cli, &config);
        info.permission = config.effective_permission(&info.permission);
        info
    };
    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));
    let system_prompt = Agent::with_agents_md(&base_prompt, &project_dir);

    let mut agent_name = agent_name;
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_mcp(&config)
        .await;
    let mut session = whycode_session::session::Session::new(project_dir.clone(), system_prompt);
    let mut history = whycode_session::SessionHistory::new();
    let mut provider = provider;
    let mut model = model;
    let mut show_thinking = false;

    println!(
        "{} {}",
        "Whycode".cyan().bold(),
        format!(
            "[agent={}, provider={}, model={}]",
            agent_name, provider, model
        )
        .dimmed()
    );
    println!(
        "{} {}",
        "Project:".dimmed(),
        project_dir.display().to_string().dimmed()
    );
    if api_key.is_empty() {
        println!(
            "{} No API key for '{}'. Set {} or run /connect. UI is ready.",
            "ℹ".yellow(),
            provider.cyan(),
            provider_env_var(&provider).cyan()
        );
    }
    println!();

    if let Some(prompt) = prompt {
        if prompt.is_empty() {
            eprintln!("{}", "Error: empty prompt".red());
            return Ok(());
        }
        if api_key.is_empty() {
            eprintln!(
                "{} No API key for '{}'. Set {} then retry.",
                "Error:".red().bold(),
                provider,
                provider_env_var(&provider)
            );
            return Ok(());
        }
        let expanded = expand_user_input(prompt, &project_dir);
        session.add_user_message(&expanded);
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
        "Interactive mode. Type /help for commands, /agent build|plan to switch. /exit to quit."
            .dimmed()
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

        // OpenCode: !command runs bash and injects output into the conversation
        if let Some(cmd) = input.strip_prefix('!') {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                println!("Usage: ! <shell command>");
                continue;
            }
            println!("{} {}", "$".dimmed(), cmd.dimmed());
            let output = run_shell_capture(cmd, &project_dir);
            println!("{}", output);
            session.add_user_message(&format!(
                "I ran the shell command `{}` and got:\n```\n{}\n```",
                cmd, output
            ));
            continue;
        }

        if input.starts_with('/') {
            let (cmd, rest) = split_slash_command(&input);
            // Custom markdown / config commands (OpenCode `/commands`)
            if let Some(name) = cmd.strip_prefix('/')
                && let Some(custom) = config.commands.get(name)
            {
                let rendered = custom.render(rest);
                if !ensure_api_key(&mut api_key, &provider, &config) {
                    continue;
                }
                println!("{} /{} → prompt", "⚡".bold(), name.cyan());
                history.push_before_turn(&session.messages, &project_dir);
                session.add_user_message(&rendered);
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
                continue;
            }
            match cmd {
                "/exit" | "/quit" | "/q" => break,
                "/help" | "/h" => {
                    print_slash_help();
                    continue;
                }
                "/new" | "/clear" => {
                    history = whycode_session::SessionHistory::new();
                    session = whycode_session::session::Session::new(
                        project_dir.clone(),
                        Agent::with_agents_md(&agent.system_prompt(), &project_dir),
                    );
                    println!("{} New session started.", "✓".green());
                    continue;
                }
                "/info" | "/details" => {
                    let i = session.info();
                    // The provider's own counts when it reported any; the
                    // character heuristic only otherwise, and labelled as an
                    // estimate. They are different measurements and printing
                    // them the same way would suggest they are not.
                    let tokens = if session.usage.is_empty() {
                        format!("Tokens≈{} (est)", session.token_count())
                    } else {
                        format!(
                            "Tokens: {} in / {} out / {} total",
                            session.usage.input_tokens,
                            session.usage.output_tokens,
                            session.usage.total()
                        )
                    };
                    println!(
                        "ID: {} | Messages: {} | {} | Agent: {} | {}/{}",
                        i.id, i.message_count, tokens, agent_name, provider, model
                    );
                    if let Some(read) = session.usage.cache_read_input_tokens {
                        println!(
                            "  Cache: {} read | {} written",
                            read,
                            session.usage.cache_creation_input_tokens.unwrap_or(0)
                        );
                    }
                    println!(
                        "  Created: {} | Project: {}",
                        i.created_at.format("%Y-%m-%d %H:%M:%S"),
                        project_dir.display()
                    );
                    continue;
                }
                "/init" => {
                    match run_init_agents_md(&project_dir, &agent, &provider, &model, &api_key)
                        .await
                    {
                        Ok(path) => println!(
                            "{} Wrote project instructions: {}",
                            "✓".green(),
                            path.cyan()
                        ),
                        Err(e) => eprintln!("{} /init failed: {}", "✗".red(), e),
                    }
                    // Reload system prompt with new AGENTS.md
                    session.set_system_prompt(&Agent::with_agents_md(
                        &Agent::system_prompt_for(&agent_name),
                        &project_dir,
                    ));
                    continue;
                }
                "/undo" => {
                    if let Some(msgs) = history.undo(&session.messages, &project_dir) {
                        session.set_messages(msgs);
                        println!(
                            "{} Undid last turn ({} messages left).",
                            "↩".cyan(),
                            session.messages.len()
                        );
                    } else if session.undo_last_turn() > 0 {
                        println!(
                            "{} Undid last turn ({} messages left).",
                            "↩".cyan(),
                            session.messages.len()
                        );
                    } else {
                        println!("{} Nothing to undo.", "ℹ".cyan());
                    }
                    continue;
                }
                "/redo" => {
                    if let Some(msgs) = history.redo(&session.messages, &project_dir) {
                        session.set_messages(msgs);
                        println!(
                            "{} Redid turn ({} messages).",
                            "↪".cyan(),
                            session.messages.len()
                        );
                    } else {
                        println!("{} Nothing to redo.", "ℹ".cyan());
                    }
                    continue;
                }
                "/share" | "/export" => {
                    match session.export_share() {
                        Ok(path) => println!("{} Session exported: {}", "✓".green(), path.cyan()),
                        Err(e) => eprintln!("{} Export failed: {}", "✗".red(), e),
                    }
                    continue;
                }
                "/compact" | "/summarize" => {
                    let before = session.messages.len();
                    session.compact(config.session.compaction_threshold);
                    println!(
                        "{} Compacted session ({} → {} messages).",
                        "✓".green(),
                        before,
                        session.messages.len()
                    );
                    continue;
                }
                "/sessions" | "/resume" | "/continue" => {
                    let _ = cmd_session(&SessionCmd::List).await;
                    continue;
                }
                "/models" => {
                    let _ = cmd_model(&ModelCmd::List).await;
                    println!("Current: {}/{}", provider.cyan(), model.cyan());
                    if !rest.is_empty() {
                        // /models provider/model
                        if let Some((p, m)) = rest.split_once('/') {
                            provider = p.to_string();
                            model = m.to_string();
                            if let Some(k) = get_api_key(&provider, &config) {
                                api_key = k;
                            }
                            println!(
                                "{} Switched model to {}/{}",
                                "✓".green(),
                                provider.cyan(),
                                model.cyan()
                            );
                        } else {
                            model = rest.to_string();
                            println!("{} Model set to {}", "✓".green(), model.cyan());
                        }
                    }
                    continue;
                }
                "/agent" | "/agents" => {
                    if rest.is_empty() {
                        let _ = cmd_agent(None).await;
                        println!("Current agent: {}", agent_name.cyan());
                    } else {
                        match switch_agent(rest, &config, &project_dir) {
                            Ok((name, new_agent, prompt)) => {
                                agent_name = name;
                                agent = new_agent;
                                session.set_system_prompt(&prompt);
                                println!(
                                    "{} Switched to agent '{}'",
                                    "✓".green(),
                                    agent_name.cyan()
                                );
                            }
                            Err(e) => eprintln!("{} {}", "✗".red(), e),
                        }
                    }
                    continue;
                }
                "/connect" => {
                    // Re-load config + env in case user set a key in another shell
                    if let Ok(cfg) = Config::load() {
                        config = cfg;
                    }
                    if let Some(k) = get_api_key(&provider, &config) {
                        api_key = k;
                        println!(
                            "{} API key loaded for {} ({}…)",
                            "✓".green(),
                            provider.cyan(),
                            api_key.chars().take(8).collect::<String>()
                        );
                    } else {
                        println!("Add a provider:");
                        println!("  whycode provider add {} --api-key <key>", provider);
                        println!("  or set env {}", provider_env_var(&provider));
                        println!();
                        println!(
                            "Env vars: ANTHROPIC_API_KEY, OPENAI_API_KEY, XAI_API_KEY, GOOGLE_API_KEY, ..."
                        );
                        let _ = cmd_provider(&ProviderCmd::List).await;
                    }
                    continue;
                }
                "/thinking" => {
                    show_thinking = !show_thinking;
                    println!(
                        "Thinking display: {}",
                        if show_thinking {
                            "ON".green().to_string()
                        } else {
                            "OFF".dimmed().to_string()
                        }
                    );
                    let _ = show_thinking; // reserved for TUI streaming
                    continue;
                }
                "/themes" => {
                    let names: Vec<&str> = whycode_tui::theme::ThemeName::ALL
                        .iter()
                        .map(|t| t.name())
                        .collect();
                    println!("{} Themes (TUI), {}:", "🎨".bold(), names.len());
                    println!("  {}", names.join(", "));
                    println!("Set in config: [tui] theme = \"{}\"", names[0]);
                    continue;
                }
                "/tools" => {
                    let tools =
                        whycode_tools::ToolExecutor::new().get_definitions(&agent.info.permission);
                    println!("{} Available tools ({}):", "🔧".bold(), tools.len());
                    for t in tools {
                        println!("  {} — {}", t.name.cyan(), t.description);
                    }
                    continue;
                }
                other => {
                    println!("Unknown command: {}. Type /help", other);
                    continue;
                }
            }
        }

        // Expand @file references (OpenCode parity)
        let expanded = expand_user_input(&input, &project_dir);

        if !ensure_api_key(&mut api_key, &provider, &config) {
            continue;
        }

        history.push_before_turn(&session.messages, &project_dir);
        session.add_user_message(&expanded);
        match agent
            .run_turn(&mut session, &provider, &model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if !response.is_empty() {
                    println!("\n{}", response);
                }
                println!();
                // Persist session best-effort (success)
                if let Ok(db) = open_db() {
                    if let Err(err) = session.save_to_db(&db) {
                        tracing::warn!(error = %err, "failed to persist session");
                    } else {
                        whycode_core::logging::emit_sid(
                            "session",
                            "info",
                            "session.persist",
                            Some(session.id.as_str()),
                            Some(serde_json::json!({
                                "reason": "ok",
                                "messages": session.messages.len(),
                            })),
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                whycode_core::logging::emit_sid(
                    "cli",
                    "error",
                    "turn.error",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({ "error": e.to_string() })),
                );
                // Persist even on error so a crash mid-debug still has history.
                if let Ok(db) = open_db() {
                    let _ = session.save_to_db(&db);
                }
            }
        }
    }
    println!("{}", "Goodbye!".cyan());
    Ok(())
}

/// Refresh API key from env/config; print how to connect if still missing.
/// Returns false if no key is available (caller should not call the LLM).
fn ensure_api_key(api_key: &mut String, provider: &str, config: &Config) -> bool {
    if !api_key.is_empty() {
        return true;
    }
    if let Some(k) = get_api_key(provider, config) {
        *api_key = k;
        return true;
    }
    let env = provider_env_var(provider);
    eprintln!(
        "{}\n  {}\n  {}\n  {}",
        format!("Setup needed · no API key for `{provider}`")
            .yellow()
            .bold(),
        format!("→ export {env}=…").dimmed(),
        format!("→ whycode provider add {provider} --api-key <key>").dimmed(),
        "Then /connect and try again.".dimmed(),
    );
    false
}

fn print_slash_help() {
    println!("{}", "Slash commands (OpenCode-compatible):".bold());
    println!("  /help, /h              — Show this help");
    println!("  /exit, /quit, /q       — Exit");
    println!("  /new, /clear           — Start a new session");
    println!("  /init                  — Create/update AGENTS.md for this project");
    println!("  /undo                  — Undo last message + file changes (git)");
    println!("  /redo                  — Redo previously undone turn");
    println!("  /share, /export        — Export session JSON");
    println!("  /compact, /summarize   — Compact long context");
    println!("  /sessions              — List saved sessions");
    println!("  /models [provider/id]  — List or switch models");
    println!("  /agent [name]          — List or switch agents (build|plan|…)");
    println!("  /connect               — Provider setup help");
    println!("  /thinking              — Toggle thinking display");
    println!("  /themes                — Theme info");
    println!("  /tools                 — List tools for current agent");
    println!("  /info, /details        — Session info");
    println!();
    println!("{}", "Also:".bold());
    println!("  !cmd                   — Run shell command, add output to chat");
    println!("  @path/to/file          — Include file contents in your message");
    println!("  Custom commands        — .whycode/commands/*.md or config [commands]");
    println!("  whycode --plain        — readline REPL instead of TUI");
}

fn split_slash_command(input: &str) -> (&str, &str) {
    let s = input.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

fn switch_agent(
    name: &str,
    config: &Config,
    project_dir: &std::path::Path,
) -> anyhow::Result<(String, Agent, String)> {
    let name = name.trim();
    let info = config.get_agent(name).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown agent '{}'. Try: build, plan, explore, general, scout",
            name
        )
    })?;
    let base = info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(name));
    let prompt = Agent::with_agents_md(&base, project_dir);
    let agent = Agent::new(info);
    Ok((name.to_string(), agent, prompt))
}

/// Expand `@path` file references and return the full prompt text.
fn expand_user_input(input: &str, project_dir: &std::path::Path) -> String {
    let mut result = String::new();
    let mut rest = input;
    while let Some(at) = rest.find('@') {
        result.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        // path continues until whitespace or end
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
                    "\n\n--- file: {} ---\n{}\n--- end file ---\n\n",
                    path_str, content
                ));
            }
            Err(_) => {
                // keep as plain text if file missing
                result.push('@');
                result.push_str(path_str);
            }
        }
        rest = &after[end..];
    }
    result.push_str(rest);
    result
}

fn run_shell_capture(cmd: &str, cwd: &std::path::Path) -> String {
    #[cfg(windows)]
    let output = std::process::Command::new("cmd")
        .args(["/C", cmd])
        .current_dir(cwd)
        .output();
    #[cfg(not(windows))]
    let output = std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .output();

    match output {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.is_empty() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(&err);
            }
            if s.is_empty() {
                s = format!("(exit {})", o.status.code().unwrap_or(-1));
            }
            s
        }
        Err(e) => format!("Failed to run command: {}", e),
    }
}

/// `/init` — analyze project and write AGENTS.md (OpenCode parity).
async fn run_init_agents_md(
    project_dir: &std::path::Path,
    agent: &Agent,
    provider: &str,
    model: &str,
    api_key: &str,
) -> anyhow::Result<String> {
    let agents_path = project_dir.join("AGENTS.md");
    let existing = std::fs::read_to_string(&agents_path).unwrap_or_default();

    // Quick project snapshot for the prompt
    let mut snapshot = String::new();
    if let Ok(entries) = std::fs::read_dir(project_dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        snapshot.push_str("Top-level entries:\n");
        for n in names.iter().take(40) {
            snapshot.push_str(&format!("- {}\n", n));
        }
    }
    for marker in [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "README.md",
    ] {
        let p = project_dir.join(marker);
        if let Ok(c) = std::fs::read_to_string(&p) {
            let preview: String = c.chars().take(2000).collect();
            snapshot.push_str(&format!("\n## {}\n```\n{}\n```\n", marker, preview));
        }
    }

    let prompt = format!(
        "Create or update an AGENTS.md file for this project. \
         AGENTS.md gives coding agents project-specific instructions \
         (build/test commands, conventions, architecture notes).\n\n\
         Project path: {}\n\n{}\n\n\
         Existing AGENTS.md (may be empty):\n```\n{}\n```\n\n\
         Write a complete AGENTS.md in Markdown. Output ONLY the file contents, no fence.",
        project_dir.display(),
        snapshot,
        existing
    );

    let mut tmp = whycode_session::session::Session::new(
        project_dir.to_path_buf(),
        "You write clear AGENTS.md project instruction files.".to_string(),
    );
    tmp.add_user_message(&prompt);
    let content = agent
        .run_turn(&mut tmp, provider, model, api_key, 5)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let content = content
        .trim()
        .trim_start_matches("```markdown")
        .trim_start_matches("```md")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string();

    if content.is_empty() {
        anyhow::bail!("Model returned empty AGENTS.md");
    }

    std::fs::write(&agents_path, format!("{}\n", content))?;
    Ok(agents_path.display().to_string())
}

/// `generate` — Non-interactive code generation (supports `--format` for CI).
async fn cmd_generate(
    cli: &Cli,
    prompt: &str,
    max_turns: usize,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);

    let api_key = match get_api_key(&provider, &config) {
        Some(k) => k,
        None => {
            let msg = format!(
                "No API key for provider '{}'. Set {} env var.",
                provider,
                provider_env_var(&provider)
            );
            return emit_headless_setup_error(format, &msg);
        }
    };

    if prompt.is_empty() {
        return emit_headless_setup_error(format, "empty prompt");
    }

    let mut agent_info = agent_info_for(cli, &config);
    agent_info.permission = config.effective_permission(&agent_info.permission);
    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));
    let system_prompt = Agent::with_agents_md(&base_prompt, &project_dir);

    // Structured CI formats cannot prompt on stdin; auto-approve tool asks.
    // Catastrophic shell risk still hard-blocks regardless of this.
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_mcp(&config)
        .await;
    if format.is_structured() {
        agent = agent.with_permission_prompter(Arc::new(AutoApprovePrompter));
    }

    let mut session = whycode_session::session::Session::new(project_dir.clone(), system_prompt);

    if format == OutputFormat::Text {
        println!(
            "{} Generating with {}/{}...",
            "⚡".bold(),
            provider.dimmed(),
            model.dimmed()
        );
    }

    let expanded = expand_user_input(prompt, &project_dir);
    session.add_user_message(&expanded);

    run_headless_turn(
        &agent,
        &mut session,
        &provider,
        &model,
        &api_key,
        &agent_name,
        max_turns,
        format,
    )
    .await
}

/// Parse `--format` / `--output-format` CLI values.
fn parse_output_format(s: &str) -> Result<OutputFormat, String> {
    s.parse()
}

/// Setup failures before a turn starts (missing key, empty prompt, …).
fn emit_headless_setup_error(format: OutputFormat, message: &str) -> anyhow::Result<()> {
    match format {
        OutputFormat::Text => {
            eprintln!("{} {}", "Error:".red().bold(), message);
            Err(anyhow::anyhow!("{}", message))
        }
        OutputFormat::Json | OutputFormat::StreamJson => {
            let _ = CiEvent::Error {
                message: message.to_string(),
            }
            .emit_stdout();
            Err(anyhow::anyhow!("{}", message))
        }
    }
}

/// Run one agent turn and write stdout according to `format`.
#[allow(clippy::too_many_arguments)]
async fn run_headless_turn(
    agent: &Agent,
    session: &mut whycode_session::session::Session,
    provider: &str,
    model: &str,
    api_key: &str,
    agent_name: &str,
    max_turns: usize,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let session_id = session.id.clone();
    let cwd = session.project_path.display().to_string();

    if format == OutputFormat::StreamJson {
        let _ = CiEvent::Init {
            session_id: session_id.clone(),
            provider: provider.to_string(),
            model: model.to_string(),
            agent: agent_name.to_string(),
            cwd,
        }
        .emit_stdout();
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let cancel = new_cancel_flag();

    // Drain TurnEvents → CiEvent while the agent runs.
    let stream = format == OutputFormat::StreamJson;
    let drain = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if !stream {
                continue;
            }
            if let Some(ci) = turn_event_to_ci(ev) {
                let _ = ci.emit_stdout();
            }
        }
    });

    let turn_result = agent
        .run_turn_with_events(
            session,
            provider,
            model,
            api_key,
            max_turns,
            Some(event_tx),
            Some(cancel),
        )
        .await;

    // Drop the sender side (inside agent) already closed; wait for drain.
    let _ = drain.await;

    let duration_ms = started.elapsed().as_millis() as u64;
    let meta = ResultMeta {
        session_id: session.id.clone(),
        provider: provider.to_string(),
        model: model.to_string(),
        agent: agent_name.to_string(),
        usage: session.usage.clone(),
        duration_ms,
    };

    match turn_result {
        Ok(response) => match format {
            OutputFormat::Text => {
                if !response.is_empty() {
                    println!("{response}");
                }
                Ok(())
            }
            OutputFormat::Json | OutputFormat::StreamJson => {
                let _ = meta.ok(response).emit_stdout();
                Ok(())
            }
        },
        Err(e) => {
            let msg = e.to_string();
            match format {
                OutputFormat::Text => {
                    eprintln!("{} {}", "Error:".red().bold(), msg);
                    Err(anyhow::anyhow!("{}", msg))
                }
                OutputFormat::Json => {
                    let _ = meta.err(&msg).emit_stdout();
                    Err(anyhow::anyhow!("{}", msg))
                }
                OutputFormat::StreamJson => {
                    if msg.to_ascii_lowercase().contains("cancel") {
                        let _ = CiEvent::Cancelled.emit_stdout();
                    } else {
                        let _ = CiEvent::Error {
                            message: msg.clone(),
                        }
                        .emit_stdout();
                    }
                    let _ = meta.err(&msg).emit_stdout();
                    Err(anyhow::anyhow!("{}", msg))
                }
            }
        }
    }
}

/// Map an agent turn event onto a CI wire event (skips Cancelled mid-stream).
fn turn_event_to_ci(ev: TurnEvent) -> Option<CiEvent> {
    match ev {
        TurnEvent::TextDelta(text) => Some(CiEvent::TextDelta { text }),
        TurnEvent::ThinkingDelta(text) => Some(CiEvent::ThinkingDelta { text }),
        TurnEvent::ToolStart { id, name, input } => Some(CiEvent::ToolStart { id, name, input }),
        TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        } => Some(CiEvent::ToolEnd {
            id,
            content,
            is_error,
        }),
        TurnEvent::Usage(u) => Some(CiEvent::Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_input_tokens: u.cache_creation_input_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
        }),
        TurnEvent::Status(message) => Some(CiEvent::Status { message }),
        TurnEvent::Cancelled => Some(CiEvent::Cancelled),
    }
}

/// `acp` — Agent Client Protocol stub (deferred until after product launch).
/// Real target: editor ↔ agent (JSON-RPC), not agent-to-agent. See docs/status.md.
async fn cmd_acp(_cli: &Cli) -> anyhow::Result<()> {
    println!("{} ACP mode — not yet implemented.", "ℹ".cyan());
    println!("Agent Client Protocol (editor ↔ agent) is planned after product launch.");
    Ok(())
}

/// `pr` — Create a pull request from current changes
async fn cmd_pr(_cli: &Cli, title: Option<&str>, base: Option<&str>) -> anyhow::Result<()> {
    let title = title.unwrap_or("Auto-generated PR");
    let base = base.unwrap_or("main");

    println!("{} Creating pull request...", "🔀".bold());
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
            println!(
                "  Or run: gh pr create --title \"{}\" --base \"{}\"",
                title, base
            );
        }
    }

    Ok(())
}

/// `github` — GitHub operations
async fn cmd_github(_cli: &Cli, cmd: &GithubCmd) -> anyhow::Result<()> {
    match cmd {
        GithubCmd::Pr { action } => match action {
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
                    .args(["pr", "create", "--title", title, "--base", base, "--fill"])
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
        },
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

/// `serve` — Start API + local share server
async fn cmd_serve(port: u16) -> anyhow::Result<()> {
    println!(
        "{} Starting Whycode API server on http://localhost:{}",
        "🚀".bold(),
        port.to_string().cyan()
    );

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
    println!("    GET  /api/shares");
    println!("    POST /api/session/new");
    println!("    POST /api/session/:id/chat");
    println!("    GET  /s/:id        — shared session (HTML)");
    println!("    GET  /s/:id.md     — shared session (Markdown)");
    println!("    GET  /s/:id.json   — shared session (JSON)");
    println!();
    println!(
        "  Share tip: in TUI run {} then open {}",
        "/share".cyan(),
        format!("http://localhost:{port}/s/<session-id>").cyan()
    );
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

/// `mcp` — MCP server management (persisted in config.toml)
async fn cmd_mcp(cmd: &McpCmd) -> anyhow::Result<()> {
    let mut config = Config::load()?;

    match cmd {
        McpCmd::List => {
            if config.mcp_servers.is_empty() {
                println!("{} No MCP servers configured.", "🔌".bold());
                println!();
                println!("Add one:");
                println!("  whycode mcp add <name> <command> [--args \"arg1 arg2\"]");
                println!("  whycode mcp add <name> --url https://mcp.example.com/mcp");
                println!("  whycode mcp add <name> --url https://host/sse --type sse");
            } else {
                println!("{} Configured MCP servers:", "🔌".bold());
                for (name, server) in &config.mcp_servers {
                    if let Some(url) = &server.url {
                        let kind = server
                            .transport
                            .map(|t| format!("{t:?}").to_lowercase())
                            .unwrap_or_else(|| "auto".into());
                        println!("  {} → {} {}", name.cyan(), kind.dimmed(), url);
                    } else {
                        let cmd = server.command.as_deref().unwrap_or("?");
                        let args = if server.args.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", server.args.join(" "))
                        };
                        println!(
                            "  {} → {}{}{}",
                            name.cyan(),
                            "stdio ".dimmed(),
                            cmd,
                            args.dimmed()
                        );
                    }
                }
            }
        }
        McpCmd::Add {
            name,
            command,
            args,
            url,
            transport,
            headers,
        } => {
            let transport_kind = match transport.as_deref() {
                None => None,
                Some("stdio") | Some("local") => Some(whycode_config::McpTransportKind::Stdio),
                Some("http") | Some("streamable-http") | Some("remote") => {
                    Some(whycode_config::McpTransportKind::Http)
                }
                Some("sse") => Some(whycode_config::McpTransportKind::Sse),
                Some("auto") => Some(whycode_config::McpTransportKind::Auto),
                Some(other) => {
                    anyhow::bail!("unknown MCP transport '{other}' (expected stdio|http|sse|auto)");
                }
            };

            let header_map = if headers.is_empty() {
                None
            } else {
                let mut map = std::collections::HashMap::new();
                for h in headers {
                    let (k, v) = h.split_once(':').ok_or_else(|| {
                        anyhow::anyhow!("invalid --header '{h}' (expected 'Key: Value')")
                    })?;
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
                Some(map)
            };

            if url.is_none() && command.is_none() {
                anyhow::bail!("provide either a local <command> or --url <endpoint>");
            }
            if url.is_some() && command.is_some() {
                anyhow::bail!("use either a local <command> or --url, not both");
            }

            let arg_vec: Vec<String> = args
                .as_deref()
                .map(|s| s.split_whitespace().map(|a| a.to_string()).collect())
                .unwrap_or_default();

            let server = whycode_config::McpServerConfig {
                transport: transport_kind,
                command: command.clone(),
                args: arg_vec.clone(),
                env: None,
                cwd: None,
                url: url.clone(),
                headers: header_map,
            };
            server
                .resolved_transport()
                .map_err(|e| anyhow::anyhow!(e))?;

            config.mcp_servers.insert(name.clone(), server);
            config.save()?;

            if let Some(url) = url {
                println!(
                    "{} MCP server '{}' saved (remote {}).",
                    "✓".green(),
                    name.cyan(),
                    url
                );
            } else {
                println!(
                    "{} MCP server '{}' saved (stdio {} {}).",
                    "✓".green(),
                    name.cyan(),
                    command.as_deref().unwrap_or("?"),
                    arg_vec.join(" ")
                );
            }
        }
        McpCmd::Remove { name } => {
            if config.mcp_servers.remove(name).is_some() {
                config.save()?;
                println!("{} MCP server '{}' removed.", "✓".green(), name.cyan());
            } else {
                eprintln!("{} MCP server '{}' not found.", "✗".red(), name.cyan());
            }
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
                println!(
                    "Built-in providers supported: openai, anthropic, deepseek, google, groq, xai"
                );
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
                tool_arguments: None,
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
                println!("{} Provider '{}' removed.", "✓".green(), name.cyan());
            } else {
                eprintln!("{} Provider '{}' not found.", "✗".red(), name.cyan());
            }
        }
        ProviderCmd::Default { name } => {
            if config.providers.contains_key(name) {
                // Save provider name as metadata
                config.save()?;
                println!("{} Default provider set to '{}'.", "✓".green(), name.cyan());
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
                context_window: None,
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
                    println!("  Model: {}/{}", model.provider_id, model.model_id);
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
        ConfigCmd::Get { key } => match get_config_value(&config, key) {
            Some(val) => println!("{}", val),
            None => eprintln!("{} Key '{}' not found.", "✗".red(), key),
        },
        ConfigCmd::Set { key, value } => {
            let mut config = config.clone();
            set_config_value(&mut config, key, value)?;
            config.save()?;
            println!("{} Set '{}' = '{}'", "✓".green(), key.cyan(), value);
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
        "project_path" => config
            .general
            .project_path
            .as_ref()
            .map(|p| p.display().to_string()),
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
            anyhow::bail!(
                "Unknown config key: {}. Supported: default_agent, project_path, log_level",
                key
            );
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
                    println!("  {} — {} ({} messages)", s.id.cyan(), s.title, msg_count);
                    println!("    Created: {}  Updated: {}", s.created_at, s.updated_at);
                    if !s.project_path.is_empty() && s.project_path != "/" {
                        println!("    Project: {}", s.project_path);
                    }
                }
            }
        }
        SessionCmd::View { id } => {
            match db.get_session(id).map_err(|e| anyhow::anyhow!("{}", e))? {
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
    // A missing database is the normal state before the first session, and is
    // not worth an error. Anything else — a locked file, a permission problem —
    // is reported rather than hidden behind "no database found", which is what
    // previously made a database fault indistinguishable from a fresh install.
    let db = match open_db() {
        Ok(d) => d,
        Err(e) if is_missing_database(&e) => {
            println!("{} No statistics database found.", "ℹ".cyan());
            println!("Stats are collected as you use whycode.");
            return Ok(());
        }
        Err(e) => {
            println!(
                "{} Could not open the statistics database: {e}",
                "!".yellow()
            );
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
            println!("  DB size:    {} bytes", meta.len());
        }
    }

    Ok(())
}

/// `debug` — Show debug information
async fn cmd_debug() -> anyhow::Result<()> {
    println!("{} Debug Information:", "🔧".bold());
    println!("  Version:     {}", VERSION_LONG.cyan());

    // Config path
    match Config::default_path() {
        Ok(p) => {
            let exists = if p.exists() {
                "✓".green()
            } else {
                "✗ (not found)".red()
            };
            println!("  Config:      {} {}", p.display(), exists);
        }
        Err(e) => {
            println!("  Config:      error: {}", e);
        }
    }

    // Data directory + log paths (Grok-style)
    match Config::data_dir() {
        Ok(p) => {
            let exists = if p.exists() {
                "✓".green()
            } else {
                "✗".red()
            };
            println!("  Data dir:    {} {}", p.display(), exists);
            let dirs = whycode_core::logging::LogDirs::from_data_dir(&p);
            println!(
                "  JSONL log:   {} {}",
                dirs.unified_jsonl().display(),
                if dirs.unified_jsonl().exists() {
                    "✓".green()
                } else {
                    "·".dimmed()
                }
            );
            println!("  Crash dir:   {}", dirs.crash.display());
            println!(
                "  Debug log:   {} (or WHYCODE_LOG_FILE / --debug)",
                dirs.debug.join("latest.log").display()
            );
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
    if let Ok(rustc) = std::process::Command::new("rustc")
        .arg("--version")
        .output()
    {
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
        "WHYCODE_LOG_LEVEL",
        "WHYCODE_LOG_FILE",
        "RUST_LOG",
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

/// `upgrade` — Self-update from the latest GitHub release
async fn cmd_upgrade() -> anyhow::Result<()> {
    let current = PKG_VERSION;
    println!("{} Whycode Upgrade", "⬆".bold());
    println!("  Current version: {}", current.cyan());
    println!("  Checking for a newer release…");

    match upgrade::run().await {
        Ok(Some(version)) => {
            println!("  {} Upgraded {current} → {}", "✓".green(), version.cyan());
        }
        Ok(None) => {
            println!("  {} Already on the latest release.", "✓".green());
        }
        Err(e) => {
            // Not fatal: a machine with no network, or a platform with no
            // published binary, should still be told how to proceed.
            println!("  {} {e}", "!".yellow());
            println!();
            println!("  Build from source instead:");
            println!(
                "    {}",
                "git clone https://github.com/whycorporation/whycode.git".dimmed()
            );
            println!(
                "    {}",
                "cd whycode && cargo install --path crates/cli".dimmed()
            );
        }
    }

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
