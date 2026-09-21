//! Strip provider credentials from child-process environments.
//!
//! Host `bash` / plugin / MCP children inherit the process env by default.
//! API keys and refresh tokens must not ride along unless a spawn explicitly
//! re-injects them.

use std::process::Command;

/// Exact names always stripped, even when they do not match `*_API_KEY`.
const SECRET_ENV_EXACT: &[&str] = &[
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

/// True when `name` is a known secret the child must not inherit.
pub fn is_secret_env_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    if SECRET_ENV_EXACT.iter().any(|s| n == *s) {
        return true;
    }
    if n.ends_with("_API_KEY") {
        return true;
    }
    n.starts_with("WHYCODES_")
        && (n.ends_with("_TOKEN") || n.ends_with("_KEY") || n.ends_with("_SECRET"))
}

/// Names currently set in this process that [`is_secret_env_name`] matches.
pub fn secret_env_names_present() -> Vec<String> {
    std::env::vars_os()
        .filter_map(|(k, _)| {
            let name = k.to_str()?.to_string();
            is_secret_env_name(&name).then_some(name)
        })
        .collect()
}

/// Call `remove` for every known exact name, then any extra matching names
/// currently set in this process (`*_API_KEY`, `WHYCODES_*_TOKEN`, …).
pub fn strip_secret_env_vars(mut remove: impl FnMut(&str)) {
    for name in SECRET_ENV_EXACT {
        remove(name);
    }
    for name in secret_env_names_present() {
        if SECRET_ENV_EXACT
            .iter()
            .any(|exact| name.eq_ignore_ascii_case(exact))
        {
            continue;
        }
        remove(&name);
    }
}

/// Drop known secrets from a `std::process::Command` before spawn.
pub fn strip_std_command_secrets(cmd: &mut Command) {
    strip_secret_env_vars(|name| {
        cmd.env_remove(name);
    });
}
