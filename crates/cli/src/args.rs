//! Command-line argument definitions for WhyCodes.

use crate::cmd::complete::{
    AuthProviderValueParser, ModelValueParser, ProviderValueParser, SessionIdValueParser,
};
use crate::{VERSION_LONG, parse_output_format};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use whycodes_core::types::ApprovalMode;
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

    /// With --continue / --resume: continue in a copy under a new id and
    /// leave the saved session untouched
    #[arg(long = "fork", global = true)]
    pub fork_session: bool,

    /// Write debug logs under the data dir (`debug/whycodes-*.log` + `debug/latest.log`)
    #[arg(long, global = true)]
    pub debug: bool,

    /// Disable cross-session semantic / auto memory for this process
    #[arg(long = "no-memory", global = true)]
    pub no_memory: bool,

    /// Skip the home-screen "update available?" prompt for this process
    #[arg(long = "no-auto-update", global = true)]
    pub no_auto_update: bool,

    /// Plan on the strong model, then hand each turn to the cheap model at
    /// the first edit (`[session] prewalk`)
    #[arg(long, global = true)]
    pub prewalk: bool,
    /// Approval preset for this run: auto, important or manual (overrides `[general] approval_mode` and WHYCODES_APPROVAL_MODE)
    #[arg(
        long = "approval-mode",
        global = true,
        value_name = "MODE",
        value_parser = parse_approval_mode
    )]
    pub approval_mode: Option<ApprovalMode>,
}

/// `--approval-mode` value: the names `/mode` and `[general] approval_mode`
/// accept, aliases included.
pub(crate) fn parse_approval_mode(raw: &str) -> Result<ApprovalMode, String> {
    ApprovalMode::parse(raw).ok_or_else(|| {
        format!("unknown approval mode `{raw}` (expected auto, important or manual)")
    })
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

        /// Auto-allow permission `ask` in `--format json` / `stream-json`
        /// (default is fail-closed: deny and stamp `denied:headless`)
        #[arg(long = "approve-tools")]
        approve_tools: bool,
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

        /// Auto-allow permission `ask` in `--format json` / `stream-json`
        /// (default is fail-closed: deny and stamp `denied:headless`)
        #[arg(long = "approve-tools")]
        approve_tools: bool,
    },

    /// Agent Client Protocol server on stdio, for editors (Zed, Neovim, …)
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

    /// Provider management (add, list, remove, default, discover)
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
        /// Dump the built-in tool catalog (name, description, parameter
        /// schema, profile, long guide) instead of environment info
        #[arg(long)]
        tools: bool,
    },

    /// Stream rules: list them, or test which would fire on a text
    Rules {
        #[command(subcommand)]
        cmd: RulesCmd,
    },

    /// Measure agent-induced code slop (ΔLOC / verbosity / erosion)
    Slop {
        /// Git base (default: merge-base with default branch, else HEAD)
        #[arg(long)]
        base: Option<String>,
        /// Machine-readable report (verdict + thresholds)
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
    /// Sign in to a remote MCP server with OAuth (opens a browser)
    Login { name: String },
    /// Forget the OAuth token stored for an MCP server
    Logout { name: String },
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
    /// Find local model servers on 127.0.0.1 (LM Studio, vLLM, llama.cpp server)
    Discover {
        /// Machine-readable list (provider, label, api_base, models)
        #[arg(long)]
        json: bool,
        /// Write each found server as `[providers.<name>]` (api_base, no key)
        #[arg(long)]
        save: bool,
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
    Status {
        /// Machine-readable: one object per provider (`provider`, `authKind`,
        /// `credentialSource`, `expiry`, `validatedAt`, `stale`); never secrets
        #[arg(long)]
        json: bool,
    },
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
    /// Get a specific configuration value and which layer set it
    Get {
        /// Configuration key (dot-separated, e.g. "default_agent")
        key: String,
        /// Machine-readable: `{ "key", "value", "overriddenBy" }`
        #[arg(long)]
        json: bool,
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
pub enum RulesCmd {
    /// List stream rules from config and `.whycodes/rules/*.md`
    List,
    /// Print which stream rules would fire on TEXT (ignores `repeat`)
    Test {
        /// Text to match, as if the model streamed it
        text: String,
        /// Channel(s): text, thinking, tool_args (comma-separated; default text)
        #[arg(long, value_delimiter = ',', value_parser = parse_stream_rule_scope)]
        scope: Vec<whycodes_config::StreamRuleScope>,
        /// Tool name for `tool_args`; without it only rules with no `tools` list match
        #[arg(long)]
        tool: Option<String>,
    },
}

fn parse_stream_rule_scope(s: &str) -> Result<whycodes_config::StreamRuleScope, String> {
    whycodes_config::StreamRuleScope::parse(s)
        .ok_or_else(|| format!("unknown scope `{s}` (text, thinking, tool_args)"))
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
    /// Find saved sessions whose messages contain every word of a query
    #[command(visible_alias = "grep")]
    Search {
        /// Words to look for (all must appear in one message)
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Most sessions to show
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
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
#[path = "args_tests.rs"]
mod tests;
