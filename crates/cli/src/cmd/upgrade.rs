//! Self-update command.
use crate::PKG_VERSION;
use colored::*;

pub(crate) fn upgrade_header_line() -> String {
    format!("{} WhyCodes Upgrade", "⬆".bold())
}

pub(crate) fn upgrade_current_line(current: &str) -> String {
    format!("  Current version: {}", current.cyan())
}

pub(crate) fn upgrade_checking_line() -> &'static str {
    "  Checking for a newer release…"
}

pub(crate) fn upgrade_ok_line(current: &str, version: Option<String>) -> String {
    format!(
        "  {} {}",
        "✓".green(),
        crate::upgrade::format_upgrade_outcome(current, Ok(version))
    )
}

pub(crate) fn upgrade_err_line(current: &str, msg: &str) -> String {
    format!(
        "  {} {}",
        "!".yellow(),
        crate::upgrade::format_upgrade_outcome(current, Err(msg.to_string()))
    )
}

pub(crate) fn upgrade_source_build_lines() -> Vec<String> {
    vec![
        String::new(),
        "  Build from source instead:".into(),
        format!(
            "    {}",
            "git clone https://github.com/whycorporation/whycodes.git".dimmed()
        ),
        format!(
            "    {}",
            "cd whycodes && cargo install --path crates/cli".dimmed()
        ),
    ]
}

pub(crate) fn should_print_source_build(msg: &str) -> bool {
    !msg.contains("brew upgrade")
}

pub(crate) async fn cmd_upgrade() -> anyhow::Result<()> {
    let current = PKG_VERSION;
    println!("{}", upgrade_header_line());
    println!("{}", upgrade_current_line(current));
    println!("{}", upgrade_checking_line());

    match crate::upgrade::run().await {
        Ok(Some(version)) => {
            println!("{}", upgrade_ok_line(current, Some(version)));
        }
        Ok(None) => {
            println!("{}", upgrade_ok_line(current, None));
        }
        Err(e) => {
            // Not fatal: a machine with no network, or a platform with no
            // published binary, should still be told how to proceed.
            let msg = e.to_string();
            println!("{}", upgrade_err_line(current, &msg));
            if should_print_source_build(&msg) {
                for line in upgrade_source_build_lines() {
                    println!("{line}");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod tests;
