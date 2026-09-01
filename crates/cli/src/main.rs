#[cfg(feature = "self-update")]
mod upgrade;

use clap::Parser;
use std::path::PathBuf;

use whycodes_config::Config;
use whycodes_protocol::OutputFormat;

/// Crate version only (semver from Cargo.toml).
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full version string: `0.3.0 (abc1234 2026-08-31)`.
///
/// Git hash and build date come from `build.rs` so release binaries and
/// `whycodes --version` / install smoke checks identify an exact build.
const VERSION_LONG: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("WHYCODES_GIT_HASH"),
    " ",
    env!("WHYCODES_BUILD_DATE"),
    ")"
);

mod args;
pub use args::*;

mod cmd;
pub(crate) use cmd::*;

fn main() -> anyhow::Result<()> {
    // Floor path for Boot/TTFF (`whycodes --version` / `-V`):
    // never build a Tokio runtime, never run clap, never touch config/logging.
    // The old `#[tokio::main]` wrapper paid for a multi-thread executor on
    // every invocation — including the ones that only print a version string.
    if early_print_version_from(std::env::args_os().skip(1)) {
        return Ok(());
    }

    // First statement on the real path: everything after it is time a user
    // waits for, and the first-frame benchmark measures from here.
    whycodes_tui::bench::mark_process_start();

    // Hosts that capture/close stdout (IDE, wrappers: stdout_tty=false) will
    // SIGPIPE-kill the process on any accidental write to stdout. Ignore it so
    // the TUI (which draws on /dev/tty) keeps running.
    ignore_sigpipe();

    // Parse before building any runtime so `--help` (and mixed `--version`
    // forms clap still handles) exit without a thread pool.
    let cli = Cli::parse();

    // Completions are stdout-only. Skip Tokio, logging, and plugin discovery
    // so Homebrew `generate_completions_from_executable` can run in a sandbox
    // that cannot write `~/.local/share/whycodes`.
    if let Some(Commands::Completions { shell }) = &cli.command {
        return cmd_completions(*shell);
    }

    let rt = runtime_for(&cli)?;
    rt.block_on(async_main(cli))
}

/// `whycodes --version` / `whycodes -V` only — same format clap would print.
///
/// Returns true when the process should exit immediately (caller returns Ok).
fn early_print_version_from<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    if is_version_only_argv(args) {
        println!("whycodes {VERSION_LONG}");
        return true;
    }
    false
}

pub(crate) fn is_version_only_argv<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args = args.into_iter();
    let Some(only) = args.next() else {
        return false;
    };
    if args.next().is_some() {
        return false;
    }
    only.as_ref() == "--version" || only.as_ref() == "-V"
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

/// Interactive full-screen TUI: plugins / OAuth / slash files wait until
/// after the first paint. `--plain` and structured `--format` stay eager.
fn command_uses_interactive_tui(cli: &Cli) -> bool {
    if cli.plain || std::env::var_os("WHYCODES_PLAIN").is_some() {
        return false;
    }
    match &cli.command {
        None => true,
        Some(Commands::Run { format, .. }) => !format.is_structured(),
        Some(Commands::Connect { .. }) => true,
        _ => false,
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
            | Commands::Debug
            | Commands::Completions { .. } => false,
        },
    }
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    // Grok-style logging: always-on JSONL under data_dir/logs/, optional file,
    // panic → data_dir/crash/. TUI keeps stderr quiet so the alternate screen
    // is not corrupted (use --debug or WHYCODES_LOG_FILE to capture human logs).
    init_logging(&cli);
    // Auth plugins are OAuth client defs (disk + JSON). Interactive TUI
    // loads them in `hydrate_tui_boot` after the first frame. Other commands
    // still need them before dispatch (login, generate, connect health).
    if !command_uses_interactive_tui(&cli) {
        load_auth_plugins(&cli);
    }

    // Determine which command to run; default to Run
    let result = match &cli.command {
        Some(cmd) => dispatch_command(cmd, &cli).await,
        None => {
            // No subcommand → interactive run
            let run_cmd = Commands::Run {
                prompt: None,
                max_turns: None,
                format: OutputFormat::Text,
            };
            dispatch_command(&run_cmd, &cli).await
        }
    };

    if let Err(ref e) = result {
        // Always land in unified.jsonl — TUI mode often silences stderr.
        whycodes_core::logging::emit(
            "whycodes",
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
    let log_file = std::env::var_os("WHYCODES_LOG_FILE").map(PathBuf::from);
    // Prefer env so we skip a full TOML/config walk on the common path.
    let log_level = std::env::var("WHYCODES_LOG_LEVEL").ok().or_else(|| {
        // Only open config when no env override — light commands stay cheap.
        Config::load()
            .ok()
            .and_then(|c| c.general.log_level.clone())
    });

    let is_tui = is_tui_invoke(cli);

    let opts = whycodes_core::logging::InitOptions {
        data_dir,
        log_level,
        log_file,
        debug: cli.debug,
        // Keep stderr free while the alternate screen is active unless the
        // user asked for --debug (file still gets the firehose either way).
        with_stderr: !is_tui || cli.debug,
    };

    if let Err(e) = whycodes_core::logging::init(opts) {
        eprintln!("warning: failed to initialize logging: {e}");
        // Last-resort so tracing macros still work somewhere.
        let _ = tracing_subscriber::fmt::try_init();
    }
}

pub(crate) fn is_tui_invoke(cli: &Cli) -> bool {
    !cli.plain
        && matches!(
            &cli.command,
            None | Some(Commands::Run { .. }) | Some(Commands::Connect { .. })
        )
}

/// Grok parity: `--max-turns` is a headless cap. Interactive TUI/REPL
/// ignores the flag (warning on stderr) so a long coding turn is not
/// killed at 25 LLM steps.
pub(crate) fn ignore_max_turns_interactive(max_turns: Option<usize>) -> Option<usize> {
    if max_turns.is_some() {
        eprintln!(
            "whycodes: --max-turns is headless-only (generate / --format json|stream-json); ignoring it in interactive mode"
        );
    }
    None
}

#[cfg(test)]
mod tests;
