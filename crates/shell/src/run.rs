//! `whycodes __shell -c <command> [name [args...]]`: run one command string.

use std::ffi::OsString;

use brush_builtins::{BuiltinSet, ShellBuilderExt};
use brush_core::{ProfileLoadBehavior, RcLoadBehavior};

const USAGE: &str = "usage: whycodes __shell -c <command> [name [args...]]";

/// Entry for `args` after `__shell`.
pub(crate) fn main(args: &[OsString]) -> i32 {
    match args {
        [flag, command, rest @ ..] if flag == "-c" => {
            report(run_dash_c(&command.to_string_lossy(), rest))
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

/// Print a fatal shell error the way bash does (`<shell>: <error>`, status 1).
pub(crate) fn report(result: Result<i32, String>) -> i32 {
    result.unwrap_or_else(|err| {
        eprintln!("whycodes shell: {err}");
        1
    })
}

/// Run `command` the way `bash -c command name args...` would.
pub(crate) fn run_dash_c(command: &str, positional: &[OsString]) -> Result<i32, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(execute(command, positional))
        .map_err(|err| err.to_string())
}

async fn execute(command: &str, positional: &[OsString]) -> Result<i32, brush_core::Error> {
    let mut words = positional.iter().map(|a| a.to_string_lossy().into_owned());
    let name = words.next().unwrap_or_else(|| "bash".to_string());
    let mut shell = brush_core::Shell::builder()
        .command_string_mode(true)
        .profile(ProfileLoadBehavior::Skip)
        .rc(RcLoadBehavior::Skip)
        .shell_name(name)
        .shell_args(words.collect())
        .default_builtins(BuiltinSet::BashMode)
        .build()
        .await?;
    crate::bundled::register(&mut shell);
    crate::cwd::wrap(&mut shell);
    #[cfg(windows)]
    crate::pathext::seed(
        &mut shell,
        command,
        &std::env::var_os("PATH").unwrap_or_default(),
    );
    let result = shell.run_dash_c_command(command).await?;
    Ok(i32::from(u8::from(result.exit_code)))
}
