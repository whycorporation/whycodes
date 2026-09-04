//! ACP, PR, and GitHub subcommands.
use crate::Cli;
use crate::args::*;
use colored::*;

pub(crate) async fn cmd_acp(_cli: &Cli) -> anyhow::Result<()> {
    println!("{} ACP mode — not yet implemented.", "ℹ".cyan());
    println!("Agent Client Protocol (editor ↔ agent) is planned after product launch.");
    Ok(())
}

/// `pr` — Create a pull request from current changes
pub(crate) async fn cmd_pr(
    _cli: &Cli,
    title: Option<&str>,
    base: Option<&str>,
) -> anyhow::Result<()> {
    let title = title.unwrap_or("Auto-generated PR");
    let base = base.unwrap_or("main");

    println!("{} Creating pull request...", "🔀".bold());
    println!("  Title: {}", title.cyan());
    println!("  Base:  {}", base.cyan());
    println!();

    // Try to use gh CLI if available
    let status = std::process::Command::new("gh")
        .args(["pr", "create", "--title", title, "--base", base, "--fill"])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("{} PR created successfully!", "✓".green());
        }
        _ => {
            println!(
                "{} Could not create PR. Install GitHub CLI: {}",
                "⚠".yellow(),
                "https://cli.github.com/".cyan()
            );
            println!(
                "  Or run: gh pr create --title \"{}\" --base \"{}\"",
                title, base
            );
        }
    }

    Ok(())
}

/// `github` — GitHub operations
pub(crate) async fn cmd_github(_cli: &Cli, cmd: &GithubCmd) -> anyhow::Result<()> {
    match cmd {
        GithubCmd::Pr { action } => match action {
            Some(PrAction::List) | None => {
                println!("{} Listing pull requests...", "📋".bold());
                let status = std::process::Command::new("gh")
                    .args(["pr", "list"])
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    _ => {
                        println!(
                            "{} GitHub CLI not available. Install: {}",
                            "⚠".yellow(),
                            "https://cli.github.com/".cyan()
                        );
                    }
                }
            }
            Some(PrAction::View { number }) => {
                println!("{} Viewing PR #{}...", "👁".bold(), number);
                let status = std::process::Command::new("gh")
                    .args(["pr", "view", &number.to_string()])
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    _ => {
                        println!("{} Could not view PR.", "⚠".yellow());
                    }
                }
            }
            Some(PrAction::Create { title, base }) => {
                let title = title.as_deref().unwrap_or("Auto PR");
                let base = base.as_deref().unwrap_or("main");
                let status = std::process::Command::new("gh")
                    .args(["pr", "create", "--title", title, "--base", base, "--fill"])
                    .status();
                match status {
                    Ok(s) if s.success() => {
                        println!("{} PR created!", "✓".green());
                    }
                    _ => {
                        println!("{} Could not create PR.", "⚠".yellow());
                    }
                }
            }
        },
        GithubCmd::Issue { number } => {
            if let Some(n) = number {
                println!("{} Viewing issue #{}...", "📝".bold(), n);
                match std::process::Command::new("gh")
                    .args(["issue", "view", &n.to_string()])
                    .status()
                {
                    Ok(s) if s.success() => {}
                    Ok(s) => tracing::warn!(code = ?s.code(), "gh issue view failed"),
                    Err(e) => tracing::warn!(error = %e, "gh issue view failed to start"),
                }
            } else {
                println!("{} Listing issues...", "📝".bold());
                match std::process::Command::new("gh")
                    .args(["issue", "list"])
                    .status()
                {
                    Ok(s) if s.success() => {}
                    Ok(s) => tracing::warn!(code = ?s.code(), "gh issue list failed"),
                    Err(e) => tracing::warn!(error = %e, "gh issue list failed to start"),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn acp_stub_runs() {
        let cli = crate::Cli {
            command: None,
            provider: None,
            model: None,
            agent_flag: None,
            dir: None,
            plain: true,
            continue_session: false,
            resume: None,
            debug: false,
            no_auto_update: true,
            no_memory: true,
        };
        super::cmd_acp(&cli).await.unwrap();
        super::cmd_pr(&cli, Some("t"), Some("dev")).await.unwrap();
    }
}
