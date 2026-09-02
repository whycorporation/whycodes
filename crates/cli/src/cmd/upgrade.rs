//! Self-update command.
use crate::PKG_VERSION;
use colored::*;

pub(crate) async fn cmd_upgrade() -> anyhow::Result<()> {
    let current = PKG_VERSION;
    println!("{} WhyCodes Upgrade", "⬆".bold());
    println!("  Current version: {}", current.cyan());
    println!("  Checking for a newer release…");

    match crate::upgrade::run().await {
        Ok(Some(version)) => {
            println!(
                "  {} {}",
                "✓".green(),
                crate::upgrade::format_upgrade_outcome(current, Ok(Some(version)))
            );
        }
        Ok(None) => {
            println!(
                "  {} {}",
                "✓".green(),
                crate::upgrade::format_upgrade_outcome(current, Ok(None))
            );
        }
        Err(e) => {
            // Not fatal: a machine with no network, or a platform with no
            // published binary, should still be told how to proceed.
            let msg = e.to_string();
            println!(
                "  {} {}",
                "!".yellow(),
                crate::upgrade::format_upgrade_outcome(current, Err(msg.clone()))
            );
            if !msg.contains("brew upgrade") {
                println!();
                println!("  Build from source instead:");
                println!(
                    "    {}",
                    "git clone https://github.com/whycorporation/whycodes.git".dimmed()
                );
                println!(
                    "    {}",
                    "cd whycodes && cargo install --path crates/cli".dimmed()
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn format_outcome_covers_arms() {
        assert!(crate::upgrade::format_upgrade_outcome("1", Ok(None)).contains("latest"));
    }
}
