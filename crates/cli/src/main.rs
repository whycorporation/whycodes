#[cfg(feature = "self-update")]
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

    /// Continue the most recently updated saved session
    #[arg(short = 'c', long = "continue", global = true)]
    pub continue_session: bool,

    /// Resume a saved session by id (full id or unique prefix)
    #[arg(short = 'r', long = "resume", global = true, value_name = "SESSION_ID")]
    pub resume: Option<String>,

    /// Write debug logs under the data dir (`debug/whycode-*.log` + `debug/latest.log`)
    #[arg(long, global = true)]
    pub debug: bool,

    /// Disable cross-session semantic / auto memory for this process
    #[arg(long = "no-memory", global = true)]
    pub no_memory: bool,
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
        /// The prompt(s) to generate code for; multiple prompts run with -j
        #[arg(required = true)]
        prompt: Vec<String>,

        /// Maximum conversation turns
        #[arg(short = 't', long, default_value = "25")]
        max_turns: usize,

        /// Parallel workers when multiple prompts are given
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,

        /// Output format for headless / CI: text (default), json, or stream-json
        #[arg(
            long = "format",
            visible_alias = "output-format",
            value_parser = parse_output_format,
            default_value = "text"
        )]
        format: OutputFormat,
    },

    /// Agent Client Protocol (not yet implemented)
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
    #[cfg(feature = "server")]
    Serve {
        /// Port to listen on
        #[arg(default_value = "3030")]
        port: u16,
    },

    /// Attach a TUI to a running `whycode serve` (not `/connect` login)
    Connect {
        /// Host:port or URL (default 127.0.0.1:3030)
        #[arg(default_value = "127.0.0.1:3030")]
        addr: String,
        /// Session id (creates a new one when omitted)
        #[arg(long)]
        session: Option<String>,
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

    /// List shell plugins from plugins.toml (global + project)
    Plugins {
        #[command(subcommand)]
        cmd: Option<PluginsCmd>,
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

    /// Cross-session memory (list, search, add, delete, clear, path)
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },

    /// Subscription login via OAuth (Claude Pro/Max, ChatGPT, Copilot, Gemini)
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },

    /// Show usage statistics
    Stats,

    /// Show debug information
    Debug,

    /// Self-update
    #[cfg(feature = "self-update")]
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
    /// Run whycode as an MCP **server** on stdio (export core tools)
    Serve {
        /// Tool profile: `core` (default) or `full`
        #[arg(long, default_value = "core")]
        tools: String,
        /// Working directory for tools (default: cwd)
        #[arg(long)]
        cwd: Option<String>,
    },
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
pub enum AuthCmd {
    /// Log in with a provider subscription (opens a browser)
    Login {
        /// Provider: anthropic | openai | github-copilot | google
        provider: String,
        /// Print the sign-in URL instead of opening a browser
        #[arg(long)]
        no_browser: bool,
    },
    /// Remove stored OAuth credentials for a provider
    Logout {
        /// Provider: anthropic | openai | github-copilot | google
        provider: String,
    },
    /// Show which providers have stored OAuth credentials (never prints tokens)
    Status,
    /// Find credentials of other CLIs (Claude Code, Codex, Gemini, Copilot)
    /// and import them after explicit per-path approval
    Import,
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
pub enum PluginsCmd {
    /// List configured plugins
    List,
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
pub enum MemoryCmd {
    /// List memories for the current project
    List {
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Semantic search over stored memories
    Search {
        query: String,
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Add a durable fact
    Add { text: Vec<String> },
    /// Delete a memory by id or unique prefix
    Delete { id: String },
    /// Clear all memories for this project
    Clear,
    /// Print MEMORY.md path for this project
    Path,
    /// Export memories to a JSON file (cross-machine sync)
    Export {
        /// Output path (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Import memories from a JSON export
    Import {
        /// Input JSON path
        path: PathBuf,
    },
    /// Index the codebase for lightweight code RAG
    Index {
        #[arg(long, default_value = "2000")]
        max_files: usize,
        #[arg(long, default_value = "8000")]
        max_chunks: usize,
    },
    /// Semantic search over the code index
    CodeSearch {
        query: String,
        #[arg(long, default_value = "8")]
        limit: usize,
    },
    /// Semantic search over prior session turns
    SessionSearch {
        query: String,
        #[arg(long, default_value = "8")]
        limit: usize,
    },
    /// Download MiniLM (if needed), verify checksums, run a probe embed
    /// (requires binary built with `--features onnx`)
    OnnxSmoke,
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
    /// Import a transcript (whycode / Claude / Codex / OpenCode / Pi)
    Import {
        /// File to import
        path: PathBuf,
        /// Format (default: auto)
        #[arg(long, default_value = "auto")]
        from: String,
    },
}

fn main() -> anyhow::Result<()> {
    // Floor path for Boot/TTFF (`whycode --version` / `-V`):
    // never build a Tokio runtime, never run clap, never touch config/logging.
    // The old `#[tokio::main]` wrapper paid for a multi-thread executor on
    // every invocation — including the ones that only print a version string.
    if early_print_version() {
        return Ok(());
    }

    // First statement on the real path: everything after it is time a user
    // waits for, and the first-frame benchmark measures from here.
    whycode_tui::bench::mark_process_start();

    // Hosts that capture/close stdout (IDE, wrappers: stdout_tty=false) will
    // SIGPIPE-kill the process on any accidental write to stdout. Ignore it so
    // the TUI (which draws on /dev/tty) keeps running.
    ignore_sigpipe();

    // Parse before building any runtime so `--help` (and mixed `--version`
    // forms clap still handles) exit without a thread pool.
    let cli = Cli::parse();

    let rt = runtime_for(&cli)?;
    rt.block_on(async_main(cli))
}

/// `whycode --version` / `whycode -V` only — same format clap would print.
///
/// Returns true when the process should exit immediately (caller returns Ok).
fn early_print_version() -> bool {
    let mut args = std::env::args_os().skip(1);
    let Some(only) = args.next() else {
        return false;
    };
    // Single-flag only so we never disagree with clap on combined argv.
    if args.next().is_some() {
        return false;
    }
    if only == "--version" || only == "-V" {
        // clap's default: "{bin-name} {version}"
        println!("whycode {VERSION_LONG}");
        return true;
    }
    false
}

/// Light subcommands (config/session/stats/…) use a current-thread runtime so
/// they do not pay for worker-thread spawn. Interactive / network / agent paths
/// keep the multi-thread pool.
fn runtime_for(cli: &Cli) -> std::io::Result<tokio::runtime::Runtime> {
    if command_needs_multi_thread(cli) {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }
}

fn command_needs_multi_thread(cli: &Cli) -> bool {
    match &cli.command {
        // Default bare invoke → interactive TUI / agent.
        None => true,
        Some(cmd) => match cmd {
            Commands::Run { .. }
            | Commands::Generate { .. }
            | Commands::Acp
            | Commands::Pr { .. }
            | Commands::Github { .. }
            | Commands::Web
            | Commands::Mcp { .. } => true,
            #[cfg(feature = "server")]
            Commands::Serve { .. } => true,
            Commands::Connect { .. } => true,
            #[cfg(feature = "self-update")]
            Commands::Upgrade => true,
            // OAuth login does network I/O (token endpoints + a loopback
            // listener) even though logout/status are local.
            Commands::Auth { .. } => true,
            // Local file / sqlite / print-only commands.
            Commands::Provider { .. }
            | Commands::Model { .. }
            | Commands::Agent { .. }
            | Commands::Plugins { .. }
            | Commands::Config { .. }
            | Commands::Session { .. }
            | Commands::Memory { .. }
            | Commands::Stats
            | Commands::Debug => false,
        },
    }
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
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
    // Prefer env so we skip a full TOML/config walk on the common path.
    let log_level = std::env::var("WHYCODE_LOG_LEVEL").ok().or_else(|| {
        // Only open config when no env override — light commands stay cheap.
        Config::load()
            .ok()
            .and_then(|c| c.general.log_level.clone())
    });

    // Full-screen TUI is the default for Run / bare invoke without --plain.
    let is_tui = !cli.plain
        && matches!(
            &cli.command,
            None | Some(Commands::Run { .. }) | Some(Commands::Connect { .. })
        );

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
            jobs,
            format,
        } => cmd_generate(cli, prompt, *max_turns, *jobs, *format).await,
        Commands::Acp => cmd_acp(cli).await,
        Commands::Pr { title, base } => cmd_pr(cli, title.as_deref(), base.as_deref()).await,
        Commands::Github { cmd: gh_cmd } => cmd_github(cli, gh_cmd).await,
        #[cfg(feature = "server")]
        Commands::Serve { port } => cmd_serve(*port).await,
        Commands::Connect { addr, session } => cmd_connect(cli, addr, session.as_deref()).await,
        Commands::Web => cmd_web().await,
        Commands::Mcp { cmd: mcp_cmd } => cmd_mcp(mcp_cmd).await,
        Commands::Provider { cmd: provider_cmd } => cmd_provider(provider_cmd).await,
        Commands::Model { cmd: model_cmd } => cmd_model(model_cmd).await,
        Commands::Agent { name } => cmd_agent(name.as_deref()).await,
        Commands::Plugins { cmd } => cmd_plugins(cli, cmd.as_ref()).await,
        Commands::Config { cmd: config_cmd } => cmd_config(config_cmd).await,
        Commands::Session { cmd: session_cmd } => cmd_session(session_cmd).await,
        Commands::Memory { cmd: memory_cmd } => cmd_memory(cli, memory_cmd).await,
        Commands::Auth { cmd } => cmd_auth(cmd).await,
        Commands::Stats => cmd_stats().await,
        Commands::Debug => cmd_debug().await,
        #[cfg(feature = "self-update")]
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

/// Resolve the credential for `provider`: env var → config `api_key` →
/// OAuth token store (`whycode auth login`), refreshing the token when it
/// is near expiry. Env and config win so a stored subscription login never
/// overrides an explicit key.
async fn get_api_key(provider: &str, config: &Config) -> Option<String> {
    let env_var = provider_env_var(provider);
    if let Ok(key) = std::env::var(&env_var)
        && !key.is_empty()
    {
        whycode_llm::oauth_refresh::unregister(provider);
        return Some(key);
    }
    if let Some(pc) = config.get_provider(provider)
        && let Some(key) = &pc.api_key
        && !key.is_empty()
    {
        whycode_llm::oauth_refresh::unregister(provider);
        return Some(key.clone());
    }
    // Fallback to generic env vars
    if provider == "openai"
        && let Ok(key) = std::env::var("OPENAI_API_KEY")
    {
        whycode_llm::oauth_refresh::unregister(provider);
        return Some(key);
    }
    // OAuth subscription login (`whycode auth login <provider>`).
    if whycode_auth::providers::supports_oauth(provider)
        && let Ok(data_dir) = Config::data_dir()
        && let Some(token) = whycode_auth::providers::access_token(provider, &data_dir).await
    {
        // A 401 on this credential may trigger one forced refresh + retry.
        whycode_llm::oauth_refresh::register(provider, data_dir);
        return Some(token);
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

/// Map CLI `--continue` / `--resume` onto a session lookup key.
///
/// `--resume` wins when both are set. Returns `None` when neither flag is used.
fn resolve_resume_want(cli: &Cli) -> Option<String> {
    if let Some(id) = cli
        .resume
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Some(id.to_string());
    }
    if cli.continue_session {
        return Some(whycode_tui::RESUME_LATEST.to_string());
    }
    None
}

/// Load `want` into `session`, preserving the current system prompt.
///
/// Returns `Ok(true)` when a session was loaded, `Ok(false)` when none matched.
fn resume_session_into(
    session: &mut whycode_session::session::Session,
    want: &str,
) -> anyhow::Result<bool> {
    let db = open_db()?;
    let Some(loaded) =
        whycode_tui::resolve_and_load_session(&db, want).map_err(|e| anyhow::anyhow!("{e}"))?
    else {
        return Ok(false);
    };
    let system_prompt = session.system_prompt.clone();
    *session = loaded;
    session.system_prompt = system_prompt;
    // Legacy `New session - …` / placeholder titles: name from first user msg.
    if session.maybe_upgrade_title_from_history()
        && let Err(err) = session.save_to_db(&db)
    {
        tracing::warn!(error = %err, "failed to persist backfilled session title");
    }
    Ok(true)
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
        let prompt_owned = prompt.to_string();
        return cmd_generate(
            cli,
            std::slice::from_ref(&prompt_owned),
            max_turns,
            1,
            format,
        )
        .await;
    }

    let project_dir_early = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir_early)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);
    let project_dir = resolve_dir(cli);
    config.load_command_files(&project_dir);

    // Interactive mode always starts (OpenCode-style). API key is optional until
    // the user actually sends a prompt that needs the LLM.
    let mut api_key = get_api_key(&provider, &config).await.unwrap_or_default();

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
    let resume_want = resolve_resume_want(cli);

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
            resume_session_id: resume_want,
            remote: None,
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
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base_prompt, &project_dir),
        &project_dir,
        &config,
        None,
    );

    // Wall clock for the Cline-style exit summary (process open → quit).
    let session_started = std::time::Instant::now();

    let mut agent_name = agent_name;
    config.general.project_path = Some(project_dir.clone());
    let file_index = whycode_index::WorkspaceIndex::start(
        whycode_index::WorkspaceIndex::project_roots(&project_dir),
    );
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index)
        .with_mcp(&config)
        .await;
    let mut session = whycode_session::session::Session::new(project_dir.clone(), system_prompt);
    maybe_session_auto_index(&project_dir, &config);
    let mut history = whycode_session::SessionHistory::new();
    let mut provider = provider;
    let mut model = model;
    let mut show_thinking = false;

    // Plain-mode resume (same flags as TUI).
    if let Some(ref want) = resume_want {
        match resume_session_into(&mut session, want) {
            Ok(true) => {
                println!(
                    "{} Resumed session {} ({}) — {} messages",
                    "✓".green(),
                    session.title.cyan(),
                    session.id.chars().take(8).collect::<String>().dimmed(),
                    session.messages.len()
                );
            }
            Ok(false) => {
                eprintln!(
                    "{} No session to resume ({}).",
                    "ℹ".yellow(),
                    if want == whycode_tui::RESUME_LATEST {
                        "none saved yet"
                    } else {
                        want.as_str()
                    }
                );
            }
            Err(e) => eprintln!("{} Resume failed: {e}", "✗".red()),
        }
    }

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
        refresh_session_memory(&mut session, &agent, &project_dir, &config, Some(&expanded));
        session.add_user_message(&expanded);
        if config.session.auto_title {
            // Prefer first user message (resume of placeholder-titled sessions).
            let seed = session
                .first_user_text()
                .unwrap_or_else(|| expanded.clone());
            // bool: whether the title changed (not a Result).
            session.apply_heuristic_title(&seed);
        }
        let (run_provider, run_model) = whycode_agent::resolve_turn_model(
            &provider,
            &model,
            &expanded,
            config.session.model_fast.as_deref(),
        );
        match agent
            .run_turn(&mut session, &run_provider, &run_model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if config.session.auto_title {
                    agent
                        .maybe_refine_title(
                            &mut session,
                            &provider,
                            &model,
                            &api_key,
                            config.session.title_model.as_deref(),
                        )
                        .await;
                }
                if !response.is_empty() {
                    println!("\n{}", response);
                }
                // Retain is spawned inside Agent::run_turn (async; best-effort).
                if let Ok(db) = open_db() {
                    let _ = session.save_to_db(&db);
                }
            }
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                if let Ok(db) = open_db() {
                    let _ = session.save_to_db(&db);
                }
                return Err(anyhow::anyhow!("{}", e));
            }
        }
        let model_label = format!("{provider}/{model}");
        print!(
            "{}",
            session.format_exit_summary(session_started.elapsed(), &model_label, "whycode")
        );
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
                if !ensure_api_key(&mut api_key, &provider, &config).await {
                    continue;
                }
                println!("{} /{} → prompt", "⚡".bold(), name.cyan());
                history.push_before_turn(&session.messages, &project_dir);
                refresh_session_memory(
                    &mut session,
                    &agent,
                    &project_dir,
                    &config,
                    Some(&rendered),
                );
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
                        with_project_memory(
                            &Agent::with_agents_md(&agent.system_prompt(), &project_dir),
                            &project_dir,
                            &config,
                            None,
                        ),
                    );
                    println!(
                        "{} New session started ({})",
                        "✓".green(),
                        session.title.dimmed()
                    );
                    continue;
                }
                "/rename" => {
                    if rest.is_empty() {
                        println!(
                            "Title: {} ({:?}) — usage: /rename <name>",
                            session.title.cyan(),
                            session.title_source
                        );
                    } else {
                        session.set_title_manual(rest);
                        if let Ok(db) = open_db()
                            && let Err(err) = db.update_title(&session.id, &session.title)
                        {
                            tracing::warn!(error = %err, "failed to persist session title");
                        }
                        println!("{} Renamed to '{}'", "✓".green(), session.title.cyan());
                    }
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
                    println!("Title: {} ({:?})", i.title.cyan(), session.title_source);
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
                    // Reload system prompt with new AGENTS.md + memory
                    session.set_system_prompt(&with_project_memory(
                        &Agent::with_agents_md(
                            &Agent::system_prompt_for(&agent_name),
                            &project_dir,
                        ),
                        &project_dir,
                        &config,
                        None,
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
                    let outcome = session.compact(config.session.compaction_threshold);
                    println!(
                        "{} Compacted session ({} → {} messages, ~{} → ~{} tok).",
                        "✓".green(),
                        outcome.messages_before,
                        outcome.messages_after,
                        outcome.tokens_before,
                        outcome.tokens_after
                    );
                    let _ = before;
                    continue;
                }
                "/diff" => {
                    let status = std::process::Command::new("git")
                        .args(["status", "--short", "--branch"])
                        .current_dir(&project_dir)
                        .output();
                    match status {
                        Ok(o) if o.status.success() => {
                            println!("{}", "Diff".bold());
                            print!("{}", String::from_utf8_lossy(&o.stdout));
                            if let Ok(d) = std::process::Command::new("git")
                                .args(["diff", "--stat", "HEAD"])
                                .current_dir(&project_dir)
                                .output()
                            {
                                let s = String::from_utf8_lossy(&d.stdout);
                                if !s.trim().is_empty() {
                                    println!("{}", s);
                                }
                            }
                        }
                        Ok(o) => eprintln!(
                            "{} git status: {}",
                            "✗".red(),
                            String::from_utf8_lossy(&o.stderr).trim()
                        ),
                        Err(e) => eprintln!("{} git unavailable: {}", "✗".red(), e),
                    }
                    continue;
                }
                "/cost" | "/usage" => {
                    let u = &session.usage;
                    println!("{}", "Cost / usage".bold());
                    if u.is_empty() {
                        println!("  session: ~{} tokens (estimated)", session.token_count());
                    } else {
                        println!(
                            "  session: {} in / {} out · total {}",
                            u.input_tokens,
                            u.output_tokens,
                            u.total()
                        );
                    }
                    continue;
                }
                "/context" => {
                    println!("{}", "Context".bold());
                    println!("  messages: {}", session.messages.len());
                    println!("  estimate: ~{} tok", session.token_count());
                    println!(
                        "  compact:  threshold={} llm={}",
                        config.session.compaction_threshold, config.session.compaction_llm
                    );
                    println!("  tools:    profile={}", config.session.tool_profile);
                    continue;
                }
                "/doctor" => {
                    println!("{}", "Doctor".bold());
                    println!("  provider: {provider}");
                    println!("  model:    {model}");
                    println!("  project:  {}", project_dir.display());
                    let key_ok = !api_key.is_empty();
                    println!("  api_key:  {}", if key_ok { "set" } else { "MISSING" });
                    println!(
                        "  sandbox:  {} network={}",
                        config.security.sandbox, config.security.sandbox_network
                    );
                    println!("  tools:    profile={}", config.session.tool_profile);
                    continue;
                }
                "/sessions" => {
                    if let Err(err) = cmd_session(&SessionCmd::List).await {
                        eprintln!("{} {}", "✗".red(), err);
                    }
                    continue;
                }
                "/resume" | "/continue" => {
                    let want = if !rest.is_empty() {
                        rest.to_string()
                    } else if cmd == "/continue" {
                        whycode_tui::RESUME_LATEST.to_string()
                    } else {
                        // /resume with no id → list, same as /sessions
                        if let Err(err) = cmd_session(&SessionCmd::List).await {
                            eprintln!("{} {}", "✗".red(), err);
                        }
                        println!("{}", "Tip: /resume <id> or /continue (latest)".dimmed());
                        continue;
                    };
                    match resume_session_into(&mut session, &want) {
                        Ok(true) => {
                            history = whycode_session::SessionHistory::new();
                            println!(
                                "{} Resumed {} ({}) — {} messages",
                                "✓".green(),
                                session.title.cyan(),
                                session.id.chars().take(8).collect::<String>().dimmed(),
                                session.messages.len()
                            );
                        }
                        Ok(false) => {
                            eprintln!("{} Session not found.", "✗".red());
                        }
                        Err(e) => eprintln!("{} Resume failed: {e}", "✗".red()),
                    }
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
                            if let Some(k) = get_api_key(&provider, &config).await {
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
                    if let Some(k) = get_api_key(&provider, &config).await {
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
                        if whycode_auth::providers::supports_oauth(&provider) {
                            println!(
                                "  or log in with your subscription: whycode auth login {}",
                                provider
                            );
                        }
                        println!();
                        println!(
                            "Env vars: ANTHROPIC_API_KEY, OPENAI_API_KEY, XAI_API_KEY, GOOGLE_API_KEY, ..."
                        );
                        let _ = cmd_provider(&ProviderCmd::List).await;
                    }
                    continue;
                }
                "/login" => {
                    let arg = rest.trim();
                    if arg.is_empty() {
                        println!("{}", "Subscription sign-in (OAuth):".bold());
                        if let Ok(dir) = Config::data_dir() {
                            let store = whycode_auth::TokenStore::new(&dir);
                            for name in whycode_auth::OAUTH_PROVIDERS {
                                let label = whycode_auth::providers::spec_for(name)
                                    .map(|s| s.label)
                                    .unwrap_or(name);
                                let status = if store.get(name).ok().flatten().is_some() {
                                    "connected".green()
                                } else {
                                    "not connected".dimmed()
                                };
                                println!(
                                    "  {} {} — {}",
                                    format!("{name:<15}").cyan(),
                                    label,
                                    status
                                );
                            }
                        }
                        println!(
                            "\nSign in: {}  ·  CLI: {}",
                            "/login <provider>".cyan(),
                            "whycode auth login <provider>".cyan()
                        );
                    } else if whycode_auth::providers::supports_oauth(arg) {
                        if let Err(e) = cmd_auth(&AuthCmd::Login {
                            provider: arg.to_string(),
                            no_browser: false,
                        })
                        .await
                        {
                            eprintln!("{} {e}", "sign-in failed:".red());
                        }
                        if arg == provider.as_str()
                            && let Some(k) = get_api_key(&provider, &config).await
                        {
                            api_key = k;
                        }
                    } else {
                        println!(
                            "OAuth login is not available for `{}` — choose from: {}",
                            arg.red(),
                            whycode_auth::OAUTH_PROVIDERS.join(", ")
                        );
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
                "/remember" => {
                    if rest.is_empty() {
                        println!("Usage: /remember <text to store>");
                    } else {
                        match whycode_memory::MemoryService::open(
                            &project_dir,
                            Config::data_dir().unwrap_or_else(|_| PathBuf::from(".")),
                            memory_settings(&config),
                        ) {
                            Ok(svc) => match svc.remember(rest, Some(&session.id)) {
                                Ok(id) => println!(
                                    "{} Remembered {} — {}",
                                    "✓".green(),
                                    id.chars().take(8).collect::<String>().cyan(),
                                    rest
                                ),
                                Err(e) => eprintln!("{} {e}", "✗".red()),
                            },
                            Err(e) => eprintln!("{} {e}", "✗".red()),
                        }
                    }
                    continue;
                }
                "/memory" => {
                    match whycode_memory::MemoryService::open(
                        &project_dir,
                        Config::data_dir().unwrap_or_else(|_| PathBuf::from(".")),
                        memory_settings(&config),
                    ) {
                        Ok(svc) => {
                            let n = svc.list(1000).map(|r| r.len()).unwrap_or(0);
                            println!(
                                "Memory: enabled={}  entries={}  path={}",
                                config.memory.enabled,
                                n,
                                svc.memory_md_path().display()
                            );
                            println!("  project_key={}", svc.project_key.dimmed());
                            println!("  CLI: whycode memory list|search|add|delete|clear");
                            if let Ok(rows) = svc.list(10) {
                                for r in rows {
                                    println!(
                                        "  · {}  {}",
                                        r.id.chars().take(8).collect::<String>().dimmed(),
                                        r.text
                                    );
                                }
                            }
                        }
                        Err(e) => eprintln!("{} {e}", "✗".red()),
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

        if !ensure_api_key(&mut api_key, &provider, &config).await {
            continue;
        }

        history.push_before_turn(&session.messages, &project_dir);
        refresh_session_memory(&mut session, &agent, &project_dir, &config, Some(&expanded));
        session.add_user_message(&expanded);
        if config.session.auto_title {
            let seed = session
                .first_user_text()
                .unwrap_or_else(|| expanded.clone());
            // bool: whether the title changed (not a Result).
            session.apply_heuristic_title(&seed);
        }
        match agent
            .run_turn(&mut session, &provider, &model, &api_key, max_turns)
            .await
        {
            Ok(response) => {
                if config.session.auto_title {
                    agent
                        .maybe_refine_title(
                            &mut session,
                            &provider,
                            &model,
                            &api_key,
                            config.session.title_model.as_deref(),
                        )
                        .await;
                }
                if !response.is_empty() {
                    println!("\n{}", response);
                }
                // Retain is spawned inside Agent::run_turn (async; best-effort).
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
                                "title": session.title,
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
    // Final flush + Cline-style summary (same shape as the TUI exit path).
    if let Ok(db) = open_db() {
        let _ = session.save_to_db(&db);
    }
    let model_label = format!("{provider}/{model}");
    print!(
        "{}",
        session.format_exit_summary(session_started.elapsed(), &model_label, "whycode")
    );
    Ok(())
}

/// Refresh API key from env/config/OAuth store; print how to connect if
/// still missing. Returns false if no key is available (caller should not
/// call the LLM).
async fn ensure_api_key(api_key: &mut String, provider: &str, config: &Config) -> bool {
    if !api_key.is_empty() {
        return true;
    }
    if let Some(k) = get_api_key(provider, config).await {
        *api_key = k;
        return true;
    }
    let env = provider_env_var(provider);
    let oauth_hint = if whycode_auth::providers::supports_oauth(provider) {
        format!("\n  → whycode auth login {provider}  (subscription)")
    } else {
        String::new()
    };
    eprintln!(
        "{}\n  {}\n  {}{}\n  {}",
        format!("Setup needed · no API key for `{provider}`")
            .yellow()
            .bold(),
        format!("→ export {env}=…").dimmed(),
        format!("→ whycode provider add {provider} --api-key <key>").dimmed(),
        oauth_hint.dimmed(),
        "Then /connect and try again.".dimmed(),
    );
    false
}

fn print_slash_help() {
    println!("{}", "Slash commands (OpenCode-compatible):".bold());
    println!("  /help, /h              — Show this help");
    println!("  /exit, /quit, /q       — Exit");
    println!("  /new, /clear           — Start a new session");
    println!("  /rename <name>         — Set session title (locks auto-title)");
    println!("  /init                  — Create/update AGENTS.md for this project");
    println!("  /undo                  — Undo last message + file changes (git)");
    println!("  /redo                  — Redo previously undone turn");
    println!("  /share, /export        — Export session JSON");
    println!("  /compact, /summarize   — Compact long context");
    println!("  /diff                  — Git status + diff --stat");
    println!("  /context               — Context window breakdown");
    println!("  /review                — AI review of git changes");
    println!("  /security-review       — Security-focused review");
    println!("  /commit                — Draft a git commit");
    println!("  /cost, /usage          — Session token usage");
    println!("  /doctor                — Environment diagnostics");
    println!("  /remember <text>       — Save a durable project memory");
    println!("  /memory                — Show memory path and entry count");
    println!("  /sessions              — List saved sessions");
    println!("  /resume [id]           — Resume a session (list if no id)");
    println!("  /continue              — Resume the most recent session");
    println!("  /models [provider/id]  — List or switch models");
    println!("  /agent [name]          — List or switch agents (build|plan|…)");
    println!("  /connect               — Provider setup help");
    println!("  /login [provider]      — Subscription sign-in (list if none)");
    println!("  /thinking              — Toggle thinking display");
    println!("  /themes                — Theme info");
    println!("  /tools                 — List tools for current agent");
    println!("  /info, /details        — Session info");
    println!();
    println!("{}", "Also:".bold());
    println!("  !cmd                   — Run shell command, add output to chat");
    println!("  @path/to/file          — Include file contents in your message");
    println!("  Custom commands        — .whycode/commands/*.md or config [commands]");
    println!("  whycode memory …       — list|search|add|delete|clear|path");
    println!("  whycode --no-memory    — disable memory for this process");
    println!("  whycode --plain        — readline REPL instead of TUI");
}

fn split_slash_command(input: &str) -> (&str, &str) {
    let s = input.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

/// Settings bag for `whycode-memory` from config (config does not depend on memory).
fn memory_settings(config: &Config) -> whycode_memory::MemorySettings {
    memory_settings_for(config, None)
}

fn memory_settings_for(
    config: &Config,
    agent_bank: Option<String>,
) -> whycode_memory::MemorySettings {
    let mut s = whycode_agent::memory_settings_from_config(config);
    s.agent_bank = agent_bank;
    s
}

/// Best-effort code index on session start (skips if already indexed).
fn maybe_session_auto_index(project_dir: &std::path::Path, config: &Config) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(n) =
        whycode_memory::maybe_auto_index(project_dir, &data_dir, &memory_settings(config))
    {
        println!("{} Auto-indexed {n} code chunks", "📇".dimmed());
    }
}

fn with_project_memory(
    system_prompt: &str,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) -> String {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    whycode_memory::apply_memory_prompt(
        system_prompt,
        project_dir,
        &data_dir,
        &memory_settings(config),
        query,
    )
}

/// Rebuild system prompt with AGENTS.md + memory recall for the current query.
fn refresh_session_memory(
    session: &mut whycode_session::session::Session,
    agent: &Agent,
    project_dir: &std::path::Path,
    config: &Config,
    query: Option<&str>,
) {
    let base = Agent::with_agents_md(&agent.system_prompt(), project_dir);
    session.set_system_prompt(&with_project_memory(&base, project_dir, config, query));
}

fn open_memory_service(
    cli: &Cli,
    config: &Config,
) -> anyhow::Result<whycode_memory::MemoryService> {
    let project_dir = resolve_dir(cli);
    let data_dir = Config::data_dir()?;
    whycode_memory::MemoryService::open(project_dir, data_dir, memory_settings(config))
}

async fn cmd_memory(cli: &Cli, cmd: &MemoryCmd) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let svc = open_memory_service(cli, &config)?;

    match cmd {
        MemoryCmd::List { limit } => {
            let rows = svc.list(*limit)?;
            if rows.is_empty() {
                println!("{} No memories for this project.", "ℹ".cyan());
            } else {
                println!(
                    "{} {} memories ({})",
                    "🧠".bold(),
                    rows.len(),
                    svc.project_key.dimmed()
                );
                for r in rows {
                    println!(
                        "  {}  {}",
                        r.id.chars().take(8).collect::<String>().dimmed(),
                        r.text
                    );
                }
            }
        }
        MemoryCmd::Search { query, limit } => {
            let hits = svc.search(query, *limit, config.memory.recall_min_score.min(0.15))?;
            if hits.is_empty() {
                println!("{} No matches.", "ℹ".cyan());
            } else {
                for h in hits {
                    println!(
                        "  [{:.2}] {}  {}",
                        h.score,
                        h.entry.id.chars().take(8).collect::<String>().dimmed(),
                        h.entry.text
                    );
                }
            }
        }
        MemoryCmd::Add { text } => {
            let text = text.join(" ");
            if text.trim().is_empty() {
                anyhow::bail!("usage: whycode memory add <text>");
            }
            let id = svc.remember(&text, None)?;
            println!(
                "{} Saved {} — {}",
                "✓".green(),
                id.chars().take(8).collect::<String>().cyan(),
                text
            );
        }
        MemoryCmd::Delete { id } => {
            if svc.delete(id)? {
                println!("{} Deleted {id}", "✓".green());
            } else {
                println!("{} No memory matching '{id}'", "ℹ".cyan());
            }
        }
        MemoryCmd::Clear => {
            let n = svc.clear()?;
            println!("{} Cleared {n} memories", "✓".green());
        }
        MemoryCmd::Path => {
            println!("{}", svc.memory_md_path().display());
            println!(
                "{} project_key={} bank={} scope={} backend={} enabled={} onnx_build={}",
                "ℹ".dimmed(),
                svc.project_key,
                svc.bank_key,
                config.memory.scope,
                config.memory.embed_backend,
                config.memory.enabled,
                whycode_memory::onnx::onnx_available()
            );
        }
        MemoryCmd::Export { output } => {
            let json = svc.export_json()?;
            match output {
                Some(path) => {
                    std::fs::write(path, &json)?;
                    println!("{} Exported to {}", "✓".green(), path.display());
                }
                None => println!("{json}"),
            }
        }
        MemoryCmd::Import { path } => {
            let json = std::fs::read_to_string(path)?;
            let (added, skipped) = svc.import_json(&json)?;
            println!(
                "{} Import complete: {added} added, {skipped} skipped",
                "✓".green()
            );
        }
        MemoryCmd::Index {
            max_files,
            max_chunks,
        } => {
            println!("{} Indexing codebase…", "⚡".bold());
            let n = svc.index_codebase(*max_files, *max_chunks)?;
            println!("{} Indexed {n} code chunks", "✓".green());
        }
        MemoryCmd::SessionSearch { query, limit } => {
            let hits =
                svc.search_sessions(query, *limit, config.memory.session_min_score.min(0.1))?;
            if hits.is_empty() {
                println!(
                    "{} No session hits yet. They appear after turns are retained.",
                    "ℹ".cyan()
                );
            } else {
                for h in hits {
                    let sid = &h.entry.session_id;
                    println!(
                        "  [{:.2}] {} turn {}",
                        h.score,
                        &sid[..8.min(sid.len())],
                        h.entry.turn_index
                    );
                    for line in h.entry.text.lines().take(4) {
                        println!("      {}", line.dimmed());
                    }
                }
            }
        }
        MemoryCmd::CodeSearch { query, limit } => {
            let hits = svc.search_code(query, *limit, config.memory.code_min_score.min(0.1))?;
            if hits.is_empty() {
                println!(
                    "{} No code hits. Run `whycode memory index` first.",
                    "ℹ".cyan()
                );
            } else {
                for h in hits {
                    println!(
                        "  [{:.2}] {}:{}-{}",
                        h.score, h.entry.path, h.entry.start_line, h.entry.end_line
                    );
                    for line in h.entry.text.lines().take(4) {
                        println!("      {}", line.dimmed());
                    }
                }
            }
        }
        MemoryCmd::OnnxSmoke => {
            if !whycode_memory::onnx::onnx_available() {
                anyhow::bail!(
                    "ONNX not in this binary. Rebuild with: cargo build -p whycode-cli --features onnx"
                );
            }
            let data_dir = Config::data_dir()?;
            println!(
                "{} Running ONNX smoke (download + checksum + embed)…",
                "⚡".bold()
            );
            let (dim, norm) = whycode_memory::onnx::smoke_embed(&data_dir)?;
            println!(
                "{} OK — embedding dim={dim}, L2-norm={norm:.4} (≈1.0 expected)",
                "✓".green()
            );
            println!(
                "  model dir: {}",
                whycode_memory::onnx::model_dir(&data_dir).display()
            );
        }
    }
    Ok(())
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
    let prompt = with_project_memory(
        &Agent::with_agents_md(&base, project_dir),
        project_dir,
        config,
        None,
    );
    let agent = Agent::new(info);
    Ok((name.to_string(), agent, prompt))
}

/// Max chars inlined per `@file` (matches TUI; keeps prefill bounded).
const AT_FILE_MAX_CHARS: usize = 24_000;

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
    prompts: &[String],
    max_turns: usize,
    jobs: usize,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let mut config = Config::load_layered(&project_dir)
        .or_else(|_| Config::load())
        .unwrap_or_default();
    if cli.no_memory {
        config.memory.enabled = false;
    }
    let provider = resolve_provider(cli, &config);
    let model = resolve_model(cli, &config);
    let agent_name = resolve_agent(cli, &config);

    let api_key = match get_api_key(&provider, &config).await {
        Some(k) => k,
        None => {
            let oauth_hint = if whycode_auth::providers::supports_oauth(&provider) {
                format!(" Or log in with your subscription: `whycode auth login {provider}`.")
            } else {
                String::new()
            };
            let msg = format!(
                "No API key for provider '{}'. Set {} env var.{}",
                provider,
                provider_env_var(&provider),
                oauth_hint
            );
            return emit_headless_setup_error(format, &msg);
        }
    };

    if prompts.iter().all(|p| p.is_empty()) {
        return emit_headless_setup_error(format, "empty prompt");
    }

    // S5: parallel fan-out. Each prompt gets its own Agent + Session; a
    // semaphore caps concurrency at `jobs`. Per-prompt failures never abort
    // siblings; the process exits non-zero if any prompt failed.
    if prompts.len() > 1 {
        let mut agent_info = agent_info_for(cli, &config);
        agent_info.permission = config.effective_permission(&agent_info.permission);
        return run_generate_parallel(
            prompts,
            &config,
            agent_info,
            &provider,
            &model,
            &agent_name,
            &api_key,
            max_turns,
            jobs.max(1),
            format,
            &project_dir,
        )
        .await;
    }

    let prompt = &prompts[0];

    let mut agent_info = agent_info_for(cli, &config);
    agent_info.permission = config.effective_permission(&agent_info.permission);
    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(&agent_name));
    let expanded = expand_user_input(prompt, &project_dir);
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base_prompt, &project_dir),
        &project_dir,
        &config,
        Some(&expanded),
    );

    // Structured CI formats cannot prompt on stdin; auto-approve tool asks.
    // Catastrophic shell risk still hard-blocks regardless of this.
    let file_index = whycode_index::WorkspaceIndex::start(
        whycode_index::WorkspaceIndex::project_roots(&project_dir),
    );
    let mut agent = Agent::new(agent_info)
        .with_config(&config)
        .with_file_index(file_index)
        .with_mcp(&config)
        .await;
    if format.is_structured() {
        agent = agent
            .with_permission_prompter(Arc::new(AutoApprovePrompter))
            .with_question_prompter(Arc::new(whycode_agent::AutoAnswerPrompter));
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

/// S5: run N prompts concurrently, each with its own Agent + Session.
///
/// A semaphore caps in-flight turns at `jobs`. Every prompt always gets a
/// final envelope: `Result` (ok or is_error) for json/stream-json, plain
/// text or an error line for text. One prompt's failure never aborts the
/// others; the process returns Err if any prompt failed.
#[allow(clippy::too_many_arguments)]
async fn run_generate_parallel(
    prompts: &[String],
    config: &Config,
    agent_info: AgentInfo,
    provider: &str,
    model: &str,
    agent_name: &str,
    api_key: &str,
    max_turns: usize,
    jobs: usize,
    format: OutputFormat,
    project_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let sem = Arc::new(tokio::sync::Semaphore::new(jobs));
    let structured = format.is_structured();
    let mut handles = Vec::new();

    for prompt in prompts {
        if prompt.is_empty() {
            continue;
        }
        let sem = Arc::clone(&sem);
        let config = config.clone();
        let agent_info = agent_info.clone();
        let provider = provider.to_string();
        let model = model.to_string();
        let agent_name = agent_name.to_string();
        let api_key = api_key.to_string();
        let prompt = prompt.clone();
        let project_dir = project_dir.to_path_buf();

        handles.push(tokio::spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return true;
            };
            run_one_parallel_turn(
                &prompt,
                &config,
                agent_info,
                &provider,
                &model,
                &agent_name,
                &api_key,
                max_turns,
                format,
                &project_dir,
                structured,
            )
            .await
        }));
    }

    let mut any_failed = false;
    for h in handles {
        match h.await {
            Ok(false) => {}
            Ok(true) => any_failed = true,
            Err(e) => {
                any_failed = true;
                let msg = format!("worker panicked: {e}");
                if structured {
                    let _ = CiEvent::Error { message: msg }.emit_stdout();
                } else {
                    eprintln!("{} {}", "Error:".red().bold(), msg);
                }
            }
        }
    }

    if any_failed {
        Err(anyhow::anyhow!("one or more prompts failed"))
    } else {
        Ok(())
    }
}

/// One prompt inside the parallel fan-out. Returns whether it failed.
/// Stdout writes are serialized inside (CiEvent locks stdout per line).
#[allow(clippy::too_many_arguments)]
async fn run_one_parallel_turn(
    prompt: &str,
    config: &Config,
    agent_info: AgentInfo,
    provider: &str,
    model: &str,
    agent_name: &str,
    api_key: &str,
    max_turns: usize,
    format: OutputFormat,
    project_dir: &std::path::Path,
    structured: bool,
) -> bool {
    let started = std::time::Instant::now();

    let base_prompt = agent_info
        .system_prompt
        .clone()
        .unwrap_or_else(|| Agent::system_prompt_for(agent_name));
    let expanded = expand_user_input(prompt, project_dir);
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&base_prompt, project_dir),
        project_dir,
        config,
        Some(&expanded),
    );

    let file_index = whycode_index::WorkspaceIndex::start(
        whycode_index::WorkspaceIndex::project_roots(project_dir),
    );
    let mut agent = Agent::new(agent_info)
        .with_config(config)
        .with_file_index(file_index)
        .with_mcp(config)
        .await;
    if structured {
        agent = agent
            .with_permission_prompter(Arc::new(AutoApprovePrompter))
            .with_question_prompter(Arc::new(whycode_agent::AutoAnswerPrompter));
    }

    let mut session =
        whycode_session::session::Session::new(project_dir.to_path_buf(), system_prompt);
    let session_id = session.id.clone();
    session.add_user_message(&expanded);

    let wrap = |ev: CiEvent| CiEvent::Session {
        session_id: session_id.clone(),
        event: Box::new(ev),
    };

    if format == OutputFormat::StreamJson {
        let _ = wrap(CiEvent::Init {
            session_id: session_id.clone(),
            provider: provider.to_string(),
            model: model.to_string(),
            agent: agent_name.to_string(),
            cwd: project_dir.display().to_string(),
        })
        .emit_stdout();
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let cancel = new_cancel_flag();
    let stream = format == OutputFormat::StreamJson;
    let sid = session_id.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if !stream {
                continue;
            }
            if let Some(ci) = turn_event_to_ci(ev) {
                let _ = CiEvent::Session {
                    session_id: sid.clone(),
                    event: Box::new(ci),
                }
                .emit_stdout();
            }
        }
    });

    let turn_result = agent
        .run_turn_with_events(
            &mut session,
            provider,
            model,
            api_key,
            max_turns,
            Some(event_tx),
            Some(cancel),
        )
        .await;
    let _ = drain.await;

    let meta = ResultMeta {
        session_id: session.id.clone(),
        provider: provider.to_string(),
        model: model.to_string(),
        agent: agent_name.to_string(),
        usage: session.usage.clone(),
        duration_ms: started.elapsed().as_millis() as u64,
    };

    match turn_result {
        Ok(response) => {
            match format {
                OutputFormat::Text => {
                    if !response.is_empty() {
                        println!("{response}");
                    }
                }
                OutputFormat::Json => {
                    let _ = meta.ok(response).emit_stdout();
                }
                OutputFormat::StreamJson => {
                    let _ = wrap(meta.ok(response)).emit_stdout();
                }
            }
            false
        }
        Err(e) => {
            let msg = e.to_string();
            match format {
                OutputFormat::Text => {
                    eprintln!("{} {}", "Error:".red().bold(), msg);
                }
                OutputFormat::Json => {
                    let _ = meta.err(&msg).emit_stdout();
                }
                OutputFormat::StreamJson => {
                    if msg.to_ascii_lowercase().contains("cancel") {
                        let _ = wrap(CiEvent::Cancelled).emit_stdout();
                    } else {
                        let _ = wrap(CiEvent::Error {
                            message: msg.clone(),
                        })
                        .emit_stdout();
                    }
                    let _ = wrap(meta.err(&msg)).emit_stdout();
                }
            }
            true
        }
    }
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
        TurnEvent::Intent {
            kind,
            confidence,
            badge,
            notice_kind,
            notice,
        } => {
            // Surface as status so CI consumers see intent without a schema break.
            let mut message = format!("intent:{kind} conf={confidence:.2}");
            if !badge.is_empty() {
                message.push_str(&format!(" badge={badge}"));
            }
            if !notice.is_empty() {
                message.push_str(&format!(" [{notice_kind}] {notice}"));
            }
            Some(CiEvent::Status { message })
        }
        TurnEvent::Cancelled => Some(CiEvent::Cancelled),
        // Surface swarm coordination as status lines (no CI schema break).
        TurnEvent::FileConflict {
            path,
            claimant,
            owner,
        } => Some(CiEvent::Status {
            message: format!("file_conflict path={path} claimant={claimant} owner={owner}"),
        }),
        TurnEvent::SwarmStatus {
            active,
            total,
            message,
        } => Some(CiEvent::Status {
            message: if message.is_empty() {
                format!("swarm active={active} total={total}")
            } else {
                message
            },
        }),
        TurnEvent::Background {
            id,
            status,
            summary,
        } => Some(CiEvent::Status {
            message: format!("bg {id} {status}: {summary}"),
        }),
        TurnEvent::EnqueuePrompt { text } => Some(CiEvent::Status {
            message: format!("enqueue_prompt: {text}"),
        }),
        TurnEvent::SwarmMessage { from, to, text } => Some(CiEvent::Status {
            message: format!("swarm_msg from={from} to={to}: {text}"),
        }),
        TurnEvent::FileStale {
            path,
            reader,
            writer,
        } => Some(CiEvent::Status {
            message: format!("file_stale path={path} reader={reader} writer={writer}"),
        }),
        TurnEvent::Panel(update) => Some(CiEvent::Status {
            message: match update {
                whycode_core::PanelUpdate::Clear => "panel clear".into(),
                whycode_core::PanelUpdate::File { path, .. } => format!("panel file={path}"),
                whycode_core::PanelUpdate::Diff { path, .. } => format!("panel diff={path}"),
                whycode_core::PanelUpdate::Mermaid { .. } => "panel mermaid".into(),
            },
        }),
    }
}

/// `acp` — Agent Client Protocol stub (deferred until after product launch).
/// Real target: editor ↔ agent (JSON-RPC), not agent-to-agent. See docs/roadmap.md.
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

/// `connect` — Attach a TUI to `whycode serve`. `/connect` remains OAuth login.
async fn cmd_connect(cli: &Cli, addr: &str, session: Option<&str>) -> anyhow::Result<()> {
    use whycode_tui::remote;

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
                "cannot reach {base}: {e}\n\nStart the daemon first:\n  whycode serve\nthen:\n  whycode connect {addr}"
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

    if !whycode_tui::tui_available() {
        anyhow::bail!("connect needs a real TUI terminal (not --plain)");
    }

    whycode_tui::run(whycode_tui::TuiRunOptions {
        project_dir,
        provider,
        model,
        api_key,
        agent_name,
        max_turns: 25,
        initial_prompt: None,
        config,
        resume_session_id: None,
        remote: Some(whycode_tui::RemoteAttach::new(base, session_id)),
    })
    .await
}

/// `serve` — Warm multi-session API + local share server.
///
/// Loads config, MCP, plugins, and a workspace file index once so clients
/// reconnect without cold startup cost (jcode/OpenCode daemon spirit).
#[cfg(feature = "server")]
async fn cmd_serve(port: u16) -> anyhow::Result<()> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use whycode_agent::{
        AutoAnswerPrompter, AutoApprovePrompter, PermissionPrompter, QuestionPrompter,
    };

    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!(
        "{} Starting Whycode warm server on http://localhost:{}",
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

    // Headless: auto-approve permissions / questions (no TUI channel).
    let file_index = whycode_index::WorkspaceIndex::start(vec![project_dir.clone()]);
    let agent = Agent::new(agent_info)
        .with_config(&config)
        .with_permission_prompter(Arc::new(AutoApprovePrompter) as Arc<dyn PermissionPrompter>)
        .with_question_prompter(Arc::new(AutoAnswerPrompter) as Arc<dyn QuestionPrompter>)
        .with_file_index(file_index)
        .with_plugins(Some(&project_dir))
        .with_mcp(&config)
        .await;

    let state = whycode_server::AppState {
        agent: Arc::new(agent),
        config: Arc::new(config),
        project_dir,
        sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        max_turns: 25,
        mcp_warm: true,
        index_warm: true,
        started_at: std::time::Instant::now(),
    };

    let router = whycode_server::create_router(state);
    // Loopback only — this is a local warm daemon, not a public API.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    println!("  Endpoints:");
    println!("    GET  /api/health          (warm status + uptime)");
    println!("    GET  /api/tools");
    println!("    GET  /api/models");
    println!("    GET  /api/sessions        (memory + SQLite)");
    println!("    POST /api/session/new");
    println!("    GET  /api/session/:id");
    println!("    POST /api/session/:id/chat  (SSE turn stream)");
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
        McpCmd::Serve { tools, cwd } => {
            use std::sync::Arc;
            use whycode_core::types::PermissionSet;
            use whycode_tools::executor::ToolExecutor;
            use whycode_tools::profile::ToolProfile;

            let profile = ToolProfile::parse(tools);
            let working_dir = cwd.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| ".".into())
            });
            let permissions = PermissionSet {
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                ..Default::default()
            };
            // Restrictive defaults: shell still risk-gated only when used via agent.
            // MCP serve runs tools directly — document risk; prefer core profile.
            let executor = Arc::new(ToolExecutor::new());
            eprintln!(
                "whycode mcp serve — profile={} cwd={} (stdio JSON-RPC)",
                profile.as_str(),
                working_dir
            );
            whycode_mcp::run_stdio_server(executor, permissions, profile, working_dir).await?;
            return Ok(());
        }
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
async fn cmd_plugins(cli: &Cli, cmd: Option<&PluginsCmd>) -> anyhow::Result<()> {
    let _ = cmd; // only List for now
    let project = resolve_dir(cli);
    let reg = whycode_skill::PluginRegistry::load_layered(&project).unwrap_or_default();
    if reg.plugins.is_empty() {
        println!("{} No shell plugins configured.", "🔌".bold());
        println!();
        println!("Create ~/.config/com.whycorporation.whycode/plugins.toml or");
        println!("  .whycode/plugins.toml:");
        println!();
        println!("  [[plugins]]");
        println!("  name = \"hello\"");
        println!("  command = \"echo hello from plugin\"");
        println!("  description = \"Demo plugin\"");
        println!();
        println!("Tools appear as plugin_<name> (tool_profile=full or tool_search).");
        return Ok(());
    }
    println!("{} Shell plugins ({}):", "🔌".bold(), reg.plugins.len());
    for p in &reg.plugins {
        println!(
            "  {} → {} — {}",
            format!("plugin_{}", p.name).cyan(),
            p.command.dimmed(),
            p.description
        );
    }
    Ok(())
}

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
                    let mut title = s.title.clone();
                    // Backfill legacy placeholders so the list is scannable.
                    if msg_count > 0
                        && whycode_session::title::looks_like_default_title(
                            &title,
                            std::path::Path::new(&s.project_path),
                        )
                        && let Ok(Some(mut loaded)) =
                            whycode_session::session::Session::load_from_db(&db, &s.id)
                        && loaded.maybe_upgrade_title_from_history()
                    {
                        if let Err(err) = loaded.save_to_db(&db) {
                            tracing::warn!(
                                error = %err,
                                "failed to persist backfilled session title"
                            );
                        }
                        title = loaded.title;
                    }
                    println!("  {} — {} ({} messages)", s.id.cyan(), title, msg_count);
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
                    let cleaned = whycode_session::sanitize_title(name);
                    if cleaned.is_empty() {
                        eprintln!("{} Empty title after sanitize.", "✗".red());
                        return Ok(());
                    }
                    db.update_title(id, &cleaned)?;
                    println!(
                        "{} Session '{}' renamed from '{}' to '{}'.",
                        "✓".green(),
                        id.cyan(),
                        s.title,
                        cleaned.cyan()
                    );
                }
                None => {
                    eprintln!("{} Session '{}' not found.", "✗".red(), id);
                }
            }
        }
        SessionCmd::Import { path, from } => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
            let kind = whycode_session::ImportKind::parse(from);
            let messages = whycode_session::import_messages(&raw, kind)?;
            let project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let title = path.file_stem().and_then(|s| s.to_str());
            let session = whycode_session::Session::from_imported(project, messages, title);
            session.save_to_db(&db)?;
            println!(
                "{} Imported {} messages as session {}",
                "✓".green(),
                session.messages.len(),
                session.id.cyan()
            );
            println!("Resume with: whycode --resume {}", &session.id[..8]);
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

/// `stats` — Show usage statistics (provider-reported tokens when available)
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

    let totals = match db.usage_totals() {
        Ok(t) => t,
        Err(e) => {
            println!("{} Could not read usage totals: {e}", "!".yellow());
            return Ok(());
        }
    };

    println!("{} Usage Statistics:", "📊".bold());
    println!("  Sessions:  {}", totals.session_count);
    println!("  Messages:  {}", totals.message_count);

    if totals.usage.is_empty() {
        println!("  Tokens:    (none recorded yet)");
        println!(
            "  {}",
            "Token totals appear after sessions that report provider usage.".dimmed()
        );
    } else {
        println!(
            "  Tokens:    {} total ({} in + {} out)",
            totals.usage.total(),
            totals.usage.input_tokens,
            totals.usage.output_tokens
        );
        if let Some(read) = totals.usage.cache_read_input_tokens {
            println!(
                "  Cache:     {} read, {} write",
                read,
                totals.usage.cache_creation_input_tokens.unwrap_or(0)
            );
        } else if let Some(write) = totals.usage.cache_creation_input_tokens {
            println!("  Cache:     {} write", write);
        }
    }

    // Top sessions by total tokens (when any usage is stored).
    if !totals.usage.is_empty() {
        let mut sessions = db.list_sessions().unwrap_or_default();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.usage.total()));
        let top: Vec<_> = sessions
            .into_iter()
            .filter(|s| !s.usage.is_empty())
            .take(5)
            .collect();
        if !top.is_empty() {
            println!();
            println!("  Top sessions by tokens:");
            for s in top {
                let title = if s.title.is_empty() {
                    s.id.chars().take(8).collect::<String>()
                } else {
                    s.title.clone()
                };
                println!("    {:>8}  {}  {}", s.usage.total(), title, s.project_path);
            }
        }
    }

    if totals.session_count > 0 {
        let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
        let db_path = data_dir.join("whycode.db");
        if let Ok(meta) = std::fs::metadata(&db_path) {
            println!();
            println!("  DB size:   {} bytes", meta.len());
        }
    }

    Ok(())
}

// ────────────────────────────────────────────────────────────────────────
// Auth (OAuth subscription login)
// ────────────────────────────────────────────────────────────────────────

async fn cmd_auth(cmd: &AuthCmd) -> anyhow::Result<()> {
    let data_dir = Config::data_dir()?;
    let store = whycode_auth::TokenStore::new(&data_dir);
    match cmd {
        AuthCmd::Login {
            provider,
            no_browser,
        } => {
            if !whycode_auth::providers::supports_oauth(provider) {
                anyhow::bail!(
                    "provider `{provider}` does not support OAuth login (supported: {})",
                    whycode_auth::OAUTH_PROVIDERS.join(", ")
                );
            }
            whycode_auth::providers::login(provider, &store, !no_browser).await?;
            println!(
                "{} Logged in to {} — credential stored in {}",
                "✓".green(),
                provider.cyan(),
                store.path().display()
            );
        }
        AuthCmd::Logout { provider } => {
            if store.remove(provider)? {
                println!(
                    "{} Removed stored credentials for {}",
                    "✓".green(),
                    provider.cyan()
                );
            } else {
                println!("No stored credentials for `{provider}`.");
            }
        }
        AuthCmd::Status => {
            let entries = store.list()?;
            if entries.is_empty() {
                println!(
                    "No OAuth logins yet. Run: whycode auth login <{}>",
                    whycode_auth::OAUTH_PROVIDERS.join("|")
                );
            } else {
                println!("{} OAuth logins ({}):", "🔑".bold(), store.path().display());
                for (name, auth) in entries {
                    println!(
                        "  {:<15} {} · {}",
                        name.cyan(),
                        auth.method,
                        auth_expiry_label(&auth).dimmed()
                    );
                }
            }
        }
        AuthCmd::Import => cmd_auth_import(&data_dir).await?,
    }
    Ok(())
}

/// `auth import` — scan for other CLIs' credential files, ask once per new
/// source (the decision is persisted), import approved ones. Sources are
/// only ever read, never modified; symlinks are refused.
async fn cmd_auth_import(data_dir: &std::path::Path) -> anyhow::Result<()> {
    use whycode_auth::discover::{ConsentStore, SourceState, import, scan};

    let consent = ConsentStore::new(data_dir);
    let found = scan(&consent);
    if found.is_empty() {
        println!(
            "No credentials from other CLIs found (looked for {}).",
            whycode_auth::discover::KNOWN_SOURCES
                .iter()
                .map(|s| s.label)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }

    println!(
        "{} Found credentials (a source is read only after your approval, and never modified):",
        "🔍".bold()
    );
    for f in &found {
        let state = match f.state {
            SourceState::New => "new".yellow(),
            SourceState::Approved => "approved".green(),
            SourceState::Denied => "denied".dimmed(),
            SourceState::Symlink => "symlink — refused".red(),
        };
        println!(
            "  {:<15} {:<45} {}",
            f.source.label.cyan(),
            f.path.display().to_string().dimmed(),
            state
        );
    }
    println!();

    let store = whycode_auth::TokenStore::new(data_dir);
    let mut imported = 0usize;
    for f in &found {
        match f.state {
            SourceState::Symlink | SourceState::Denied => {}
            SourceState::Approved => match import(&store, &consent, f) {
                Ok(()) => {
                    imported += 1;
                    println!(
                        "{} Imported {} → `{}`",
                        "✓".green(),
                        f.source.label.cyan(),
                        f.source.provider
                    );
                }
                Err(e) => println!("{} {}: {e}", "✗".red(), f.source.label),
            },
            SourceState::New => {
                print!(
                    "Import {} ({}) as `{}`? [y/N] ",
                    f.source.label,
                    f.path.display(),
                    f.source.provider
                );
                use std::io::Write as _;
                if let Err(e) = std::io::stdout().flush() {
                    eprintln!("warning: could not flush prompt: {e}");
                }
                let answer = tokio::task::spawn_blocking(|| {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).map(|_| line)
                })
                .await??;
                let yes = matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");
                consent.record(&f.path, yes)?;
                if yes {
                    match import(&store, &consent, f) {
                        Ok(()) => {
                            imported += 1;
                            println!("{} Imported `{}`", "✓".green(), f.source.provider);
                        }
                        Err(e) => println!("{} {}: {e}", "✗".red(), f.source.label),
                    }
                } else {
                    println!(
                        "Skipped (won't ask again — delete {} to reset)",
                        consent.path().display()
                    );
                }
            }
        }
    }
    if imported > 0 {
        println!(
            "\n{} {imported} credential(s) ready — `whycode auth status` lists them.",
            "✓".green()
        );
    }
    Ok(())
}

/// Human expiry label for `auth status` / `debug` — never token material.
fn auth_expiry_label(auth: &whycode_auth::ProviderAuth) -> String {
    // A derived API token (e.g. Copilot's) is the one that actually expires;
    // it lives in extra. "copilot_expires_at" is the pre-rename key name.
    let derived_expiry = auth
        .token
        .extra
        .get("derived_expires_at")
        .or_else(|| auth.token.extra.get("copilot_expires_at"))
        .and_then(|v| v.as_str());
    if let Some(at) = derived_expiry {
        return format!("derived API token expires {at}");
    }
    match auth.token.expires_at {
        Some(at) => {
            if auth.token.is_expired() {
                format!("expired {at} (refreshes on next use)")
            } else {
                format!("expires {at}")
            }
        }
        None => "no expiry".to_string(),
    }
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

    // OAuth subscription logins — method + expiry only, never token material.
    println!("  OAuth (auth.json):");
    match Config::data_dir() {
        Ok(dir) => {
            let store = whycode_auth::TokenStore::new(&dir);
            match store.list() {
                Ok(entries) if entries.is_empty() => {
                    println!("    (none — `whycode auth login <provider>`)");
                }
                Ok(entries) => {
                    for (name, auth) in entries {
                        println!(
                            "    {:<15} {} · {}",
                            name,
                            auth.method,
                            auth_expiry_label(&auth)
                        );
                    }
                }
                Err(e) => println!("    error reading store: {e}"),
            }
        }
        Err(e) => println!("    data dir error: {e}"),
    }

    Ok(())
}

/// `upgrade` — Self-update from the latest GitHub release
#[cfg(feature = "self-update")]
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
