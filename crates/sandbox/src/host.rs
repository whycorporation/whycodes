use std::path::PathBuf;

use super::policy::{Backend, PreparedCommand};

/// Plain `bash -c <command>` on the host (no namespace isolation).
pub fn prepare_host(
    command: &str,
    working_dir: PathBuf,
    warning: Option<String>,
) -> PreparedCommand {
    PreparedCommand {
        program: "bash".into(),
        args: vec!["-c".into(), command.to_string()],
        working_dir,
        backend: Backend::Host,
        warning,
    }
}
