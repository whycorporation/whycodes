//! `cd`, `pushd` and `popd` wrappers.
//!
//! brush keeps the working directory in the shell, not the process. Windows
//! resolves a relative program path (`./build.cmd`) against the *process*
//! directory, so after `cd sub` that program was not found (upstream #1251).
//! The wrappers run brush's builtin, then move the process to the shell's
//! directory. They also rewrite an MSYS drive path (`cd /c/Users`).
//!
//! Limitation: a subshell (`(cd sub && ./x)`) shares the process, so after
//! it returns the process directory is the subshell's until the next `cd`.

use std::sync::OnceLock;

use brush_core::builtins::{BoxFuture, CommandExecuteFunc};
use brush_core::commands::{CommandArg, ExecutionContext};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{ExecutionResult, Shell};

type Original = CommandExecuteFunc<DefaultShellExtensions>;

const WRAPPED: [&str; 3] = ["cd", "pushd", "popd"];

static ORIGINALS: OnceLock<Vec<(&'static str, Original)>> = OnceLock::new();

/// Replace the directory builtins with wrappers.
pub(crate) fn wrap(shell: &mut Shell) {
    let mut originals = Vec::new();
    for name in WRAPPED {
        if let Some(mut registration) = shell.builtins().get(name).cloned() {
            originals.push((name, registration.execute_func));
            registration.execute_func = execute;
            shell.register_builtin(name, registration);
        }
    }
    ORIGINALS.get_or_init(|| originals);
}

fn execute(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let ExecutionContext {
            shell,
            command_name,
            params,
        } = context;
        let original = ORIGINALS
            .get()
            .and_then(|all| all.iter().find(|(name, _)| *name == command_name))
            .map(|(_, run)| *run)
            .ok_or_else(|| brush_core::ErrorKind::CommandNotFound(command_name.clone()))?;
        let args = args.into_iter().map(translate).collect();
        let inner = ExecutionContext {
            shell: &mut *shell,
            command_name,
            params,
        };
        let result = original(inner, args).await;
        if let Err(err) = std::env::set_current_dir(shell.working_dir()) {
            tracing::debug!(error = %err, "embedded shell: process directory not moved");
        }
        result
    })
}

fn translate(arg: CommandArg) -> CommandArg {
    match arg {
        CommandArg::String(text) => CommandArg::String(
            crate::msys::translate_path(text.as_ref())
                .to_string_lossy()
                .into_owned(),
        ),
        other => other,
    }
}
