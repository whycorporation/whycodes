//! Embedded bash-compatible shell for Windows hosts without Git Bash.
//!
//! `whycodes __shell -c <command>` runs `command` in brush (a bash-compatible
//! interpreter) with a bundled set of coreutils, findutils, sed, diff, grep
//! and which. Bundled utilities resolve before `PATH`, so `find` and `sort`
//! never reach the DOS programs in `System32`.
//!
//! Every bundled utility runs in its own child process
//! (`whycodes __shell/<name> <args...>`), never in the shell process or the
//! agent process: pipes and redirections are real OS handles, and a panic in
//! a utility only ends that child. The agent itself only ever spawns
//! `whycodes __shell -c`; see `whycodes_sandbox::set_embedded_shell`.

mod bundled;
mod cwd;
mod diff;
mod grep;
mod msys;
#[cfg(any(windows, test))]
mod pathext;
mod run;
mod which;

pub use bundled::is_bundled;

/// First argument of every embedded-shell invocation.
pub const SUBCOMMAND: &str = "__shell";

/// Prefix of `argv[1]` for a bundled-utility child: `__shell/<name>`.
///
/// The utility name is the last path component, so the uutils crates report
/// errors as `ls: ...` instead of naming the host binary.
const INVOKE_PREFIX: &str = "__shell/";

/// Run an embedded-shell invocation if `args` (argv without `argv[0]`) is
/// one. Returns the exit code, or `None` when `args` is some other command.
pub fn dispatch(args: &[std::ffi::OsString]) -> Option<i32> {
    let first = args.first()?.to_str()?;
    if first == SUBCOMMAND {
        return Some(run::main(&args[1..]));
    }
    let name = first.strip_prefix(INVOKE_PREFIX)?;
    Some(bundled::invoke(name, &args[1..]))
}

#[cfg(test)]
mod tests;
