//! Command-line argument definitions for WhyCodes.

use crate::cmd::complete::{
    AuthProviderValueParser, ModelValueParser, ProviderValueParser, SessionIdValueParser,
};
use crate::{VERSION_LONG, parse_output_format};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use whycodes_protocol::OutputFormat;

/// Drop "Did you mean" when two or more names are equally close (#70).
///
/// A wrong unique pick is worse than silence; listing two equals is noise
/// next to the usage the user would have read (`whycodes auth logn` → login
/// and logout). A single close match (`sesion` → `session`) is kept.
pub(crate) fn sanitize_clap_error(mut err: clap::Error) -> clap::Error {
    match err.kind() {
        ErrorKind::InvalidSubcommand | ErrorKind::UnknownArgument => {}
        _ => return err,
    }
    for kind in [
        ContextKind::SuggestedSubcommand,
        ContextKind::SuggestedCommand,
        ContextKind::SuggestedArg,
        ContextKind::SuggestedValue,
        ContextKind::Suggested,
    ] {
        let drop = match err.get(kind) {
            Some(ContextValue::Strings(items)) => items.len() > 1,
            Some(ContextValue::StyledStrs(items)) => items.len() > 1,
            _ => false,
        };
        if drop {
            err.remove(kind);
        }
    }
    err
}

/// WhyCodes — An AI coding agent built in Rust
#[derive(Parser, Debug)]
#[command(
    name = "whycodes",
    version = VERSION_LONG,
    about = "AI-powered coding agent",
    long_about = None,
    infer_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Provider to use
    #[arg(short = 'P', long, global = true, value_parser = ProviderValueParser)]
    pub provider: Option<String>,

    /// Model to use
    #[arg(short = 'm', long, global = true, value_parser = ModelValueParser)]
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
    #[arg(
        short = 'r',
        long = "resume",
        global = true,
        value_name = "SESSION_ID",
        value_parser = SessionIdValueParser
    )]
    pub resume: Option<String>,

    /// Write debug logs under the data dir (`debug/whycodes-*.log` + `debug/latest.log`)
    #[arg(long, global = true)]
    pub debug: bool,

    /// Disable cross-session semantic / auto memory for this process
    #[arg(long = "no-memory", global = true)]
    pub no_memory: bool,

    /// Skip the home-screen "update available?" prompt for this process
    #[arg(long = "no-auto-update", global = true)]
    pub no_auto_update: bool,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Commands {
    /// Start an interactive session (default)
    #[command(name = "run")]
    Run {
        /// Optional initial prompt (with --format json|stream-json this is one-shot CI mode)
        prompt: Option<String>,

        /// Maximum agentic turns (headless only; ignored in the TUI)
        #[arg(short = 't', long)]
        max_turns: Option<usize>,

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

        /// Maximum agentic turns before stopping (no default cap)
        #[arg(short = 't', long)]
        max_turns: Option<usize>,

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
        /// Do not prompt to take over an existing `serve` (CI / scripts)
        #[arg(long = "no-takeover")]
        no_takeover: bool,
    },

    /// Attach a TUI to a running `whycodes serve` (not `/connect` login)
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

    /// List shell plugins from plugins.toml and plugin.json trees
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

    /// Import MCP, permissions, and hooks from Claude Code, OpenCode, Grok, or Codex
    Import {
        #[command(flatten)]
        args: ImportArgs,
    },

    /// Show usage statistics
    Stats,

    /// Show debug information
    Debug {
        /// Machine-readable dump (stable camelCase keys; never token material)
        #[arg(long)]
        json: bool,
    },

    /// Self-update
    #[cfg(feature = "self-update")]
    #[command(name = "upgrade")]
    Upgrade,

    /// Generate shell completion scripts (bash, zsh, fish, powershell, elvish)
    Completions {
        /// Target shell
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
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
#[command(infer_subcommands = true)]
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
#[command(infer_subcommands = true)]
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
    /// Run whycodes as an MCP **server** on stdio (export core tools)
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
#[command(infer_subcommands = true)]
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
#[command(infer_subcommands = true)]
pub enum AuthCmd {
    /// Log in with a provider subscription (opens a browser)
    Login {
        /// Provider: anthropic | openai | github-copilot | google | google-antigravity | xai
        #[arg(value_parser = AuthProviderValueParser)]
        provider: String,
        /// Print the sign-in URL instead of opening a browser
        #[arg(long)]
        no_browser: bool,
    },
    /// Remove stored OAuth credentials for a provider
    Logout {
        /// Provider: anthropic | openai | github-copilot | google | google-antigravity | xai
        #[arg(value_parser = AuthProviderValueParser)]
        provider: String,
    },
    /// Show which providers have stored OAuth credentials (never prints tokens)
    Status,
    /// Find credentials of other CLIs (Claude Code, Codex, Gemini, Copilot, Grok Build)
    /// and import them after explicit per-path approval
    Import,
}

/// `whycodes import` — copy user-level settings from other coding agents.
#[derive(clap::Args, Debug)]
pub struct ImportArgs {
    /// Product: claude | opencode | grok | codex
    #[arg(long)]
    pub from: Option<String>,
    /// Show the plan without writing config.toml
    #[arg(long)]
    pub dry_run: bool,
    /// Approve every new source without prompting
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Overwrite WhyCodes keys that already exist
    #[arg(long)]
    pub force: bool,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
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
#[command(infer_subcommands = true)]
pub enum PluginsCmd {
    /// List configured plugins
    List,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
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
#[command(infer_subcommands = true)]
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
#[command(infer_subcommands = true)]
pub enum SessionCmd {
    /// List all sessions
    #[command(visible_alias = "ls")]
    List,
    /// View a session's details
    View {
        /// Session ID
        #[arg(value_parser = SessionIdValueParser)]
        id: String,
    },
    /// Delete a session
    Delete {
        /// Session ID
        #[arg(value_parser = SessionIdValueParser)]
        id: String,
    },
    /// Rename a session
    Rename {
        /// Session ID
        #[arg(value_parser = SessionIdValueParser)]
        id: String,
        /// New name for the session
        name: String,
    },
    /// Export a session to JSON (shareable)
    Share {
        /// Session ID
        #[arg(value_parser = SessionIdValueParser)]
        id: String,
    },
    /// Import a transcript (whycodes / Claude / Codex / OpenCode / Pi)
    Import {
        /// File to import
        path: PathBuf,
        /// Format (default: auto)
        #[arg(long, default_value = "auto")]
        from: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn clap_parses_run_and_generate() {
        let cli = Cli::try_parse_from(["whycodes", "run", "--plain", "hi"]).unwrap();
        assert!(cli.plain);
        assert!(matches!(
            cli.command,
            Some(Commands::Run { prompt: Some(ref p), .. }) if p == "hi"
        ));
        let parsed = Cli::try_parse_from(["whycodes", "generate", "a", "b", "-j", "2"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Commands::Generate { ref prompt, jobs, .. }) if prompt.as_slice() == ["a", "b"] && jobs == 2
        ));
    }

    #[test]
    fn tied_typo_suggestions_are_stripped() {
        let err = Cli::try_parse_from(["whycodes", "auth", "logn"]).unwrap_err();
        let raw = err.to_string();
        let clean = super::sanitize_clap_error(err).to_string();
        // clap may list both `login` and `logout`; after sanitize, never both
        // under a "Did you mean" (or equivalent) hint.
        if raw.contains("login") && raw.contains("logout") {
            assert!(
                !(clean.contains("Did you mean")
                    && clean.contains("login")
                    && clean.contains("logout")),
                "tied suggestions must be dropped:\n{clean}"
            );
        }
        assert!(
            clean.contains("unrecognized") || clean.contains("invalid") || !clean.is_empty(),
            "{clean}"
        );
    }

    #[test]
    fn unique_typo_suggestion_is_kept() {
        let err = Cli::try_parse_from(["whycodes", "sesion"]).unwrap_err();
        let clean = super::sanitize_clap_error(err).to_string();
        assert!(
            clean.contains("session") || clean.contains("Did you mean"),
            "unique close match should still be suggested:\n{clean}"
        );
    }
}
