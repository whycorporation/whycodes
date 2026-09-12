#[cfg(feature = "self-update")]
mod upgrade;

use clap::Parser;
use std::path::PathBuf;

use whycodes_config::Config;
use whycodes_protocol::OutputFormat;

/// Crate version only (semver from Cargo.toml).
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full version string: `0.5.0 (abc1234 2026-09-12)`.
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
    //
    // Windows: scan GetCommandLineW (no CommandLineToArgvW) and WriteFile
    // (no println! locale). Unix: write(1) the same bytes.
    if try_print_version_fast() {
        return Ok(());
    }

    // First statement on the real path: everything after it is time a user
    // waits for, and the first-frame benchmark measures from here.
    whycodes_tui::bench::mark_process_start();

    // `whycodes run -d <dir>` (harness) and bare `whycodes`: skip clap + Tokio
    // until after the first paint. Extra flags still take the full parser.
    if let Some(project_dir) = early_tui_run_dir_from(std::env::args_os().skip(1)) {
        return cmd_run_fast_tui(project_dir);
    }

    // Hosts that capture/close stdout (IDE, wrappers: stdout_tty=false) will
    // SIGPIPE-kill the process on any accidental write to stdout. Ignore it so
    // the TUI (which draws on the controlling console) keeps running.
    ignore_sigpipe();

    // Parse before building any runtime so `--help` (and mixed `--version`
    // forms clap still handles) exit without a thread pool.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => args::sanitize_clap_error(err).exit(),
    };

    // Completions are stdout-only. Skip Tokio, logging, and plugin discovery
    // so Homebrew `generate_completions_from_executable` can run in a sandbox
    // that cannot write `~/.local/share/whycodes`.
    if let Some(Commands::Completions { shell }) = &cli.command {
        return cmd_completions(*shell);
    }

    let short = crate::cmd::hang::is_short_command(&cli);
    let rt = runtime_for(&cli)?;
    let result = rt.block_on(async_main(cli));
    if short {
        crate::cmd::hang::shutdown_runtime(rt);
    }
    result
}

const VERSION_LINE: &str = concat!(
    "whycodes ",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("WHYCODES_GIT_HASH"),
    " ",
    env!("WHYCODES_BUILD_DATE"),
    ")\n"
);

/// `whycodes --version` / `whycodes -V` only — same format clap would print.
///
/// Returns true when the process should exit immediately (caller returns Ok).
fn try_print_version_fast() -> bool {
    if !is_version_only_process() {
        return false;
    }
    write_version_line();
    true
}

fn write_version_line() {
    #[cfg(windows)]
    {
        write_version_line_windows();
    }
    #[cfg(not(windows))]
    {
        if let Err(e) = std::io::Write::write_all(&mut std::io::stdout(), VERSION_LINE.as_bytes()) {
            tracing::debug!(error = %e, "version line write failed");
        }
    }
}

#[cfg(windows)]
fn write_version_line_windows() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(nStdHandle: i32) -> *mut core::ffi::c_void;
        fn WriteFile(
            hFile: *mut core::ffi::c_void,
            lpBuffer: *const u8,
            nNumberOfBytesToWrite: u32,
            lpNumberOfBytesWritten: *mut u32,
            lpOverlapped: *mut core::ffi::c_void,
        ) -> i32;
    }
    const STD_OUTPUT_HANDLE: i32 = -11;
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() || handle == (-1isize as *mut core::ffi::c_void) {
            return;
        }
        let bytes = VERSION_LINE.as_bytes();
        let mut written = 0u32;
        let ok = WriteFile(
            handle,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        );
        if ok == 0 || written == 0 {
            tracing::debug!("WriteFile version line failed");
        }
    }
}

#[cfg(windows)]
fn is_version_only_process() -> bool {
    is_version_only_command_line_utf16(windows_command_line_utf16())
}

/// Raw `GetCommandLineW` as UTF-16, no String alloc.
#[cfg(windows)]
fn windows_command_line_utf16() -> &'static [u16] {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCommandLineW() -> *const u16;
    }
    unsafe {
        let ptr = GetCommandLineW();
        if ptr.is_null() {
            return &[];
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 32_768 {
                break;
            }
        }
        core::slice::from_raw_parts(ptr, len)
    }
}

#[cfg(any(windows, test))]
pub(crate) fn is_version_only_command_line_utf16(line: &[u16]) -> bool {
    let line = trim_u16(line);
    let rest = trim_u16(strip_exe_prefix_u16(line));
    u16_eq_ascii(rest, b"--version") || u16_eq_ascii(rest, b"-V")
}

#[cfg(any(windows, test))]
fn u16_eq_ascii(u: &[u16], ascii: &[u8]) -> bool {
    u.len() == ascii.len() && u.iter().zip(ascii).all(|(c, b)| *c == u16::from(*b))
}

#[cfg(any(windows, test))]
fn trim_u16(s: &[u16]) -> &[u16] {
    let start = s.iter().position(|&c| !is_u16_space(c)).unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&c| !is_u16_space(c))
        .map(|i| i + 1)
        .unwrap_or(0);
    if start >= end { &[] } else { &s[start..end] }
}

#[cfg(any(windows, test))]
fn is_u16_space(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16 || c == 0x0a || c == 0x0d
}

#[cfg(any(windows, test))]
fn strip_exe_prefix_u16(line: &[u16]) -> &[u16] {
    let line = trim_u16(line);
    if line.first().copied() == Some(b'"' as u16) {
        return match line.iter().skip(1).position(|&c| c == b'"' as u16) {
            Some(end) => &line[end + 2..],
            None => &[],
        };
    }
    match line.iter().position(|&c| is_u16_space(c)) {
        Some(i) => &line[i..],
        None => &[],
    }
}

#[cfg(not(windows))]
fn is_version_only_process() -> bool {
    is_version_only_argv(std::env::args_os().skip(1))
}

/// True when the process was invoked as `whycodes --version` or `whycodes -V`
/// (optional quoted exe path, optional surrounding spaces). Extra flags fail.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn is_version_only_command_line(line: &str) -> bool {
    let line = line.trim();
    let rest = strip_exe_prefix(line).trim();
    rest == "--version" || rest == "-V"
}

#[cfg_attr(not(test), allow(dead_code))]
fn strip_exe_prefix(line: &str) -> &str {
    let line = line.trim_start();
    if let Some(inner) = line.strip_prefix('"') {
        return match inner.find('"') {
            Some(end) => &inner[end + 1..],
            None => "",
        };
    }
    match line.find(char::is_whitespace) {
        Some(i) => &line[i..],
        None => "",
    }
}

/// Iterator form for tests / Unix. Writes the version line when argv is only
/// `--version` / `-V`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn early_print_version_from<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    if is_version_only_argv(args) {
        write_version_line();
        return true;
    }
    false
}

#[cfg_attr(windows, allow(dead_code))]
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

/// `whycodes`, `whycodes run`, or `whycodes run -d <dir>` with no other flags.
///
/// Anything else (`--plain`, `-P`, a prompt) needs clap. The first-frame
/// harness is exactly `run -d <tempdir>`.
pub(crate) fn early_tui_run_dir_from<I, S>(args: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    };
    let first = first.as_ref();
    if first == "run" {
        let Some(flag) = args.next() else {
            return Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        };
        if flag.as_ref() != "-d" && flag.as_ref() != "--dir" {
            return None;
        }
        let dir = args.next()?;
        if args.next().is_some() {
            return None;
        }
        return Some(PathBuf::from(dir.as_ref()));
    }
    if first == "-d" || first == "--dir" {
        let dir = args.next()?;
        if args.next().is_some() {
            return None;
        }
        return Some(PathBuf::from(dir.as_ref()));
    }
    None
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
        // First-frame harness never runs a turn. A current-thread runtime
        // skips worker spawn (~Windows CreateThread tax) and is enough for
        // chrome → draw → exit (`WHYCODES_BENCH_DURATION_MS=0`).
        if std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty()) {
            return tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
        }
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
            | Commands::Debug { .. }
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
    // First-frame harness (`WHYCODES_BENCH`) skips disk logging so AppData
    // mkdir + JSONL open is not on the TTFF clock (issue #85).
    let bench = std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty());
    if !bench {
        init_logging(&cli);
    }
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
        #[cfg(test)]
        {
            return result;
        }
        #[cfg(not(test))]
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
