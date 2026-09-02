#[cfg(feature = "self-update")]
mod upgrade;

use clap::Parser;
use std::path::PathBuf;

use whycodes_config::Config;
use whycodes_protocol::OutputFormat;

/// Crate version only (semver from Cargo.toml).
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full version string: `0.4.0 (abc1234 2026-09-02)`.
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

/// Worker threads for interactive TUI / `run`.
///
/// `crossterm::event::poll` blocks one runtime thread; 2 workers let
/// turn HTTP + hydrate run while poll is blocked. Do not use
/// `current_thread` — spawned turns starve until the next poll return.
const TUI_WORKER_THREADS: usize = 2;

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
/// they do not pay for worker-thread spawn. Interactive TUI / network / agent
/// paths keep the multi-thread pool.
///
/// Do **not** put the TUI on `current_thread`. The event loop blocks on
/// `crossterm::event::poll`; spawned turn / stream / catalog tasks never
/// run until that poll returns, so a submitted prompt hangs until Esc
/// force-cancels (2026-09-01).
fn runtime_for(cli: &Cli) -> std::io::Result<tokio::runtime::Runtime> {
    if command_needs_multi_thread(cli) {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        // Generate / Serve keep the default nproc pool; interactive TUI
        // and other multi-thread commands cap at two workers.
        if !command_needs_full_worker_pool(cli) {
            builder.worker_threads(TUI_WORKER_THREADS);
        }
        builder.build()
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }
}

fn command_needs_multi_thread(cli: &Cli) -> bool {
    match &cli.command {
        // Bare invoke → interactive TUI. Worker-pool spawn costs a few ms of
        // TTFF (#49) but is required so `tokio::spawn` turns progress while
        // the loop is inside blocking `event::poll`.
        None => true,
        Some(cmd) => match cmd {
            Commands::Run { .. } => true,
            Commands::Generate { .. }
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
            | Commands::Import { .. }
            | Commands::Stats
            | Commands::Debug
            | Commands::Completions { .. } => false,
        },
    }
}

/// `generate -j` and `serve` benefit from the default nproc pool.
/// Everything else that needs multi-thread (TUI / `run` / mcp / auth / …)
/// is capped at [`TUI_WORKER_THREADS`].
fn command_needs_full_worker_pool(cli: &Cli) -> bool {
    match &cli.command {
        Some(Commands::Generate { .. }) => true,
        #[cfg(feature = "server")]
        Some(Commands::Serve { .. }) => true,
        _ => false,
    }
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    // Grok-style logging: always-on JSONL under data_dir/logs/, optional file,
    // panic → data_dir/crash/. TUI keeps stderr quiet so the alternate screen
    // is not corrupted (use --debug or WHYCODES_LOG_FILE to capture human logs).
    init_logging(&cli);
    if !is_tui_invoke(&cli) {
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
        let _sigpipe = signal(SIGPIPE, SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_sigpipe() {}

/// Resolve data dir + env/config filters and install the process logger.
fn init_logging(cli: &Cli) {
    let data_dir = Config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let log_file = std::env::var_os("WHYCODES_LOG_FILE").map(PathBuf::from);
    // Prefer env so we skip a full TOML/config walk on the common path.
    // When WHYCODES_BENCH is set the first-frame clock is running; avoid any
    // config file I/O before the first paint.
    let bench = std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty());
    let log_level = std::env::var("WHYCODES_LOG_LEVEL").ok().or_else(|| {
        if bench {
            return None;
        }
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
