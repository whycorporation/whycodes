//! Import MCP / permission / hook settings from other coding agents.
use super::helpers::*;
use crate::args::*;
use colored::*;
use std::io::Write as _;
use std::path::Path;
use whycodes_config::Config;
use whycodes_import::discover::{self, KNOWN_SOURCES};
use whycodes_import::{
    ConsentStore, FoundSource, ImportPlan, Product, SourceState, apply_and_save, preview,
    scan_with_home,
};

pub(crate) async fn cmd_import(cmd: &ImportArgs) -> anyhow::Result<()> {
    let data_dir = Config::data_dir()?;
    let consent = ConsentStore::new(&data_dir);
    let home = discover::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    run_import(
        &consent,
        &home,
        cmd.from.as_deref(),
        cmd.dry_run,
        cmd.yes,
        cmd.force,
        false,
    )
}

/// First-run: TTY interactive session, no user config yet, foreign settings exist.
/// Returns true when `config.toml` was written (caller should reload).
///
/// Callers skip this when the full-screen TUI will run — that path offers
/// the same question as a home-screen confirm (see `whycodes_tui`).
pub(crate) fn maybe_first_run_import(interactive: bool) -> anyhow::Result<bool> {
    if !interactive {
        return Ok(false);
    }
    if std::env::var_os("CI").is_some() || std::env::var_os("WHYCODES_SKIP_IMPORT").is_some() {
        return Ok(false);
    }
    // Piped / CI hosts: do not consume stdin or persist a "asked" marker.
    if !cfg!(test) {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return Ok(false);
        }
    }
    if !whycodes_import::why_config_missing() {
        return Ok(false);
    }
    let data_dir = Config::data_dir()?;
    let consent = ConsentStore::new(&data_dir);
    if consent.first_run_asked()? {
        return Ok(false);
    }
    let Some(home) = discover::home_dir() else {
        consent.mark_first_run_asked()?;
        return Ok(false);
    };
    let found = scan_with_home(&home, &consent);
    if found.is_empty() || found.iter().all(|f| f.state == SourceState::Symlink) {
        consent.mark_first_run_asked()?;
        return Ok(false);
    }

    println!();
    println!("{} Found setups from other agents:", "import".bold());
    print_found(&found);
    println!();
    print!("Import into WhyCodes? [y/N] ");
    if let Err(e) = std::io::stdout().flush() {
        eprintln!("warning: could not flush prompt: {e}");
    }
    let mut line = String::new();
    read_repl_line(&mut line)?;
    let yes = matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    consent.mark_first_run_asked()?;
    if !yes {
        println!(
            "Skipped. Run `{}` later to import MCP, permissions, and hooks.",
            "whycodes import".cyan()
        );
        return Ok(false);
    }
    run_import(&consent, &home, None, false, true, false, true)?;
    Ok(true)
}

fn run_import(
    consent: &ConsentStore,
    home: &Path,
    from: Option<&str>,
    dry_run: bool,
    yes: bool,
    force: bool,
    already_asked: bool,
) -> anyhow::Result<()> {
    let product_filter = match from {
        Some(s) => Some(Product::parse(s).ok_or_else(|| {
            anyhow::anyhow!(whycodes_import::ImportError::UnknownProduct(s.into()))
        })?),
        None => None,
    };

    let mut found = scan_with_home(home, consent);
    if let Some(product) = product_filter {
        found.retain(|f| f.product == product);
    }
    if found.is_empty() {
        let looked = match product_filter {
            Some(p) => p.label().to_string(),
            None => KNOWN_SOURCES
                .iter()
                .map(|s| s.product.label())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", "),
        };
        println!("No settings from other agents found (looked for {looked}).");
        return Ok(());
    }

    println!(
        "{} Found settings (a source is read only after your approval, and never modified):",
        "·".bold()
    );
    print_found(&found);
    println!();

    if !already_asked {
        for f in &found {
            match f.state {
                SourceState::Symlink | SourceState::Denied | SourceState::Approved => {}
                SourceState::New => {
                    if yes {
                        consent.approve(&f.path)?;
                    } else {
                        print!(
                            "Import {} ({})? [y/N] ",
                            f.product.label(),
                            f.path.display()
                        );
                        if let Err(e) = std::io::stdout().flush() {
                            eprintln!("warning: could not flush prompt: {e}");
                        }
                        let mut line = String::new();
                        read_repl_line(&mut line)?;
                        let ok = matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes");
                        if ok {
                            consent.approve(&f.path)?;
                        } else {
                            consent.deny(&f.path)?;
                        }
                    }
                }
            }
        }
    } else {
        for f in &found {
            if f.state == SourceState::New {
                consent.approve(&f.path)?;
            }
        }
    }

    let found = scan_with_home(home, consent);
    let mut found: Vec<FoundSource> = found
        .into_iter()
        .filter(|f| product_filter.is_none_or(|p| f.product == p))
        .collect();
    // After first-run bulk yes, treat New as approved for this pass.
    if already_asked || yes {
        for f in &mut found {
            if f.state == SourceState::New {
                f.state = SourceState::Approved;
            }
        }
    }

    let config = Config::load().unwrap_or_default();
    let (extracted, plan) = preview(&found, &config, force)?;
    if extracted.is_empty() {
        println!("Nothing approved to import.");
        return Ok(());
    }
    print_extracted(&extracted);
    print_plan(&plan);

    if dry_run {
        println!("{}", "Dry run — config.toml not written.".dimmed());
        return Ok(());
    }
    if plan.is_empty() {
        println!("Nothing new to write (WhyCodes already has these keys).");
        return Ok(());
    }
    let path = apply_and_save(&plan)?;
    println!(
        "{} Wrote {}",
        "✓".green(),
        path.display().to_string().cyan()
    );
    println!(
        "Credentials are separate: `{}` copies API tokens.",
        "whycodes auth import".cyan()
    );
    Ok(())
}

fn print_found(found: &[FoundSource]) {
    for f in found {
        let state = match f.state {
            SourceState::New => "new".yellow(),
            SourceState::Approved => "approved".green(),
            SourceState::Denied => "denied".dimmed(),
            SourceState::Symlink => "symlink — refused".red(),
        };
        println!(
            "  {:<14} {:<48} {}",
            f.product.label().cyan(),
            f.path.display().to_string().dimmed(),
            state
        );
    }
}

fn print_extracted(extracted: &[whycodes_import::Extracted]) {
    for item in extracted {
        println!(
            "  {}  {}  {}",
            item.product.label().cyan(),
            item.path.display().to_string().dimmed(),
            item.counts_label()
        );
        for s in &item.skipped {
            println!("    {} {s}", "skip".dimmed());
        }
    }
    println!();
}

fn print_plan(plan: &ImportPlan) {
    println!("{}", plan.summary().bold());
    for (name, _) in &plan.mcp_add {
        println!("  {} MCP `{name}`", "+".green());
    }
    for (name, why) in &plan.mcp_skip {
        println!("  {} MCP `{name}` ({why})", "·".dimmed());
    }
    for (tool, action) in &plan.permission_add {
        println!("  {} permission `{tool}` = {action:?}", "+".green());
    }
    for (tool, why) in &plan.permission_skip {
        println!("  {} permission `{tool}` ({why})", "·".dimmed());
    }
    for hook in &plan.hooks_add {
        println!("  {} hook {:?} `{}`", "+".green(), hook.event, hook.command);
    }
    for s in &plan.hooks_skip {
        println!("  {} hook {s}", "·".dimmed());
    }
    for w in &plan.warnings {
        println!("  {} {w}", "!".yellow());
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_helpers_do_not_panic() {
        let src = FoundSource {
            product: Product::Claude,
            rel_path: ".claude.json",
            path: std::path::PathBuf::from("/tmp/x"),
            state: SourceState::New,
        };
        print_found(&[src]);
        print_found(&[FoundSource {
            product: Product::Claude,
            rel_path: ".claude.json",
            path: std::path::PathBuf::from("/tmp/a"),
            state: SourceState::Approved,
        }]);
        print_found(&[FoundSource {
            product: Product::Grok,
            rel_path: ".grok/config.toml",
            path: std::path::PathBuf::from("/tmp/g"),
            state: SourceState::Denied,
        }]);
        print_found(&[FoundSource {
            product: Product::Codex,
            rel_path: ".codex/config.toml",
            path: std::path::PathBuf::from("/tmp/c"),
            state: SourceState::Symlink,
        }]);
        print_extracted(&[whycodes_import::Extracted {
            product: Product::OpenCode,
            path: std::path::PathBuf::from("/tmp/o"),
            mcp: Vec::new(),
            permission: Default::default(),
            hooks: Vec::new(),
            skipped: vec!["event SessionStart".into()],
        }]);
        let mut plan = ImportPlan::default();
        plan.mcp_add.push((
            "fs".into(),
            whycodes_config::McpServerConfig {
                transport: None,
                command: Some("npx".into()),
                args: vec![],
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        ));
        plan.mcp_skip
            .push(("git".into(), "already have `git`".into()));
        plan.permission_add
            .push(("bash".into(), whycodes_core::types::PermissionAction::Ask));
        plan.permission_skip
            .push(("read".into(), "already have `read`".into()));
        plan.hooks_add.push(whycodes_config::HookConfig {
            event: whycodes_config::HookEvent::PreTool,
            tool_match: "bash".into(),
            command: "echo hi".into(),
            block_on_failure: true,
            timeout_secs: 30,
        });
        plan.hooks_skip.push("pre_tool echo hi".into());
        plan.warnings.push("ignored".into());
        print_plan(&plan);
        assert!(!maybe_first_run_import(false).unwrap());
    }
}
