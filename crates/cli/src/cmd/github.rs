//! PR and GitHub subcommands.
use crate::Cli;
use crate::args::*;
use colored::*;

pub(crate) fn pr_create_header_lines(title: &str, base: &str) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("{} Creating pull request...", "🔀".bold()));
    lines.push(format!("  Title: {}", title.cyan()));
    lines.push(format!("  Base:  {}", base.cyan()));
    lines.push(String::new());
    lines
}

pub(crate) fn pr_created_line() -> String {
    format!("{} PR created successfully!", "✓".green())
}

pub(crate) fn pr_created_short_line() -> String {
    format!("{} PR created!", "✓".green())
}

pub(crate) fn pr_create_failed_lines(title: &str, base: &str) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} Could not create PR. Install GitHub CLI: {}",
        "⚠".yellow(),
        "https://cli.github.com/".cyan()
    ));
    lines.push(format!(
        "  Or run: gh pr create --title \"{}\" --base \"{}\"",
        title, base
    ));
    lines
}

pub(crate) fn pr_list_header_line() -> String {
    format!("{} Listing pull requests...", "🔀".bold())
}

pub(crate) fn gh_cli_missing_line() -> String {
    format!(
        "{} GitHub CLI not available. Install: {}",
        "⚠".yellow(),
        "https://cli.github.com/".cyan()
    )
}

pub(crate) fn pr_view_header_line(number: u64) -> String {
    format!("{} Viewing PR #{}...", "🔀".bold(), number)
}

pub(crate) fn pr_view_failed_line() -> String {
    format!("{} Could not view PR.", "⚠".yellow())
}

pub(crate) fn pr_create_failed_short_line() -> String {
    format!("{} Could not create PR.", "⚠".yellow())
}

pub(crate) fn issue_view_header_line(number: u64) -> String {
    format!("{} Viewing issue #{}...", "🔀".bold(), number)
}

pub(crate) fn issue_list_header_line() -> String {
    format!("{} Listing issues...", "🔀".bold())
}

pub(crate) fn gh_status_ok(status: Result<std::process::ExitStatus, std::io::Error>) -> bool {
    matches!(status, Ok(s) if s.success())
}

/// GitHub CLI binary. Production always uses `gh` from `PATH`. Tests must
/// point this at a stub on the current thread (`TestGhGuard`): a test build
/// that would run the real `gh` panics instead, because `gh pr create` /
/// `gh issue list` act on the developer's real account and repository.
pub(crate) fn gh_program() -> String {
    #[cfg(test)]
    if let Some(path) = TEST_GH.with(|slot| slot.borrow().clone()) {
        return path;
    }
    real_gh(cfg!(test))
}

/// The real `gh` from `PATH`. `refuse` is `cfg!(test)`: a test that reaches
/// here has no stub and would act on the developer's GitHub account.
fn real_gh(refuse: bool) -> String {
    assert!(
        !refuse,
        "test ran the real `gh` from PATH; stub it with TestGhGuard"
    );
    "gh".into()
}

#[cfg(test)]
std::thread_local! {
    static TEST_GH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct TestGhGuard(Option<String>);

#[cfg(test)]
impl TestGhGuard {
    pub(crate) fn set(program: Option<String>) -> Self {
        let prev = TEST_GH.with(|slot| slot.replace(program));
        Self(prev)
    }
}

#[cfg(test)]
impl Drop for TestGhGuard {
    fn drop(&mut self) {
        TEST_GH.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

/// `pr` — Create a pull request from current changes
pub(crate) async fn cmd_pr(
    _cli: &Cli,
    title: Option<&str>,
    base: Option<&str>,
) -> anyhow::Result<()> {
    let title = title.unwrap_or("Auto-generated PR");
    let base = base.unwrap_or("main");

    for line in pr_create_header_lines(title, base) {
        println!("{line}");
    }

    // Try to use gh CLI if available
    let status = std::process::Command::new(gh_program())
        .args(["pr", "create", "--title", title, "--base", base, "--fill"])
        .status();

    if gh_status_ok(status) {
        println!("{}", pr_created_line());
    } else {
        for line in pr_create_failed_lines(title, base) {
            println!("{line}");
        }
    }

    Ok(())
}

/// `github` — GitHub operations
pub(crate) async fn cmd_github(_cli: &Cli, cmd: &GithubCmd) -> anyhow::Result<()> {
    match cmd {
        GithubCmd::Pr { action } => match action {
            Some(PrAction::List) | None => {
                println!("{}", pr_list_header_line());
                let status = std::process::Command::new(gh_program())
                    .args(["pr", "list"])
                    .status();
                if !gh_status_ok(status) {
                    println!("{}", gh_cli_missing_line());
                }
            }
            Some(PrAction::View { number }) => {
                println!("{}", pr_view_header_line(*number));
                let status = std::process::Command::new(gh_program())
                    .args(["pr", "view", &number.to_string()])
                    .status();
                if !gh_status_ok(status) {
                    println!("{}", pr_view_failed_line());
                }
            }
            Some(PrAction::Create { title, base }) => {
                let title = title.as_deref().unwrap_or("Auto PR");
                let base = base.as_deref().unwrap_or("main");
                let status = std::process::Command::new(gh_program())
                    .args(["pr", "create", "--title", title, "--base", base, "--fill"])
                    .status();
                if gh_status_ok(status) {
                    println!("{}", pr_created_short_line());
                } else {
                    println!("{}", pr_create_failed_short_line());
                }
            }
        },
        GithubCmd::Issue { number } => {
            if let Some(n) = number {
                println!("{}", issue_view_header_line(*n));
                match std::process::Command::new(gh_program())
                    .args(["issue", "view", &n.to_string()])
                    .status()
                {
                    Ok(s) if s.success() => {}
                    Ok(s) => tracing::warn!(code = ?s.code(), "gh issue view failed"),
                    Err(e) => tracing::warn!(error = %e, "gh issue view failed to start"),
                }
            } else {
                println!("{}", issue_list_header_line());
                match std::process::Command::new(gh_program())
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
#[path = "github_tests.rs"]
mod tests;
