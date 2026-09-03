//! Settings import from other coding agents (home-screen confirm + `/import`).
//!
//! Discovery never reads a file until the user confirms. Source files are
//! never modified. Same consent store as `whycodes import`.

use super::*;
use std::path::Path;
use whycodes_import::{
    ConsentStore, FoundSource, ImportPlan, Product, SourceState, apply_and_save, preview,
    scan_with_home,
};

/// True when CI / `WHYCODES_SKIP_IMPORT` should suppress the home popup.
pub(super) fn import_env_skipped() -> bool {
    std::env::var_os("CI").is_some() || std::env::var_os("WHYCODES_SKIP_IMPORT").is_some()
}

/// Home-screen first-run offer. Same gating as the TTY prompt, minus stdin:
/// no user `config.toml`, not already asked, at least one non-symlink source.
pub(super) fn maybe_offer_import(app: &mut TuiApp) {
    if app.import_prompted || app.dialogs.is_open() || import_env_skipped() {
        return;
    }
    if !app.messages.is_empty() {
        app.import_prompted = true;
        return;
    }
    if !whycodes_import::why_config_missing() {
        app.import_prompted = true;
        return;
    }
    let Ok(data_dir) = Config::data_dir() else {
        app.import_prompted = true;
        return;
    };
    let consent = ConsentStore::new(&data_dir);
    match consent.first_run_asked() {
        Ok(true) => {
            app.import_prompted = true;
            return;
        }
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(error = %e, "import consent read failed");
            app.import_prompted = true;
            return;
        }
    }
    let Some(home) = whycodes_import::discover::home_dir() else {
        if let Err(e) = consent.mark_first_run_asked() {
            tracing::warn!(error = %e, "import first-run mark failed");
        }
        app.import_prompted = true;
        return;
    };
    let found = scan_with_home(&home, &consent);
    if found.is_empty() || found.iter().all(|f| f.state == SourceState::Symlink) {
        if let Err(e) = consent.mark_first_run_asked() {
            tracing::warn!(error = %e, "import first-run mark failed");
        }
        app.import_prompted = true;
        return;
    }
    app.import_prompted = true;
    let products = unique_product_labels(&found);
    match prepare_import_preview(None) {
        Ok(PreviewOutcome::Ready { plan }) => {
            app.open_import_picker(&plan);
        }
        Ok(_) => {
            // Nothing mapped — still offer a yes/no so the user can skip once.
            offer_import_confirm(app, &products);
        }
        Err(e) => {
            tracing::warn!(error = %e, "import first-run preview failed");
            offer_import_confirm(app, &products);
        }
    }
}

fn offer_import_confirm(app: &mut TuiApp, products: &str) {
    app.confirm(
        "Import settings",
        format!(
            "Found setups from {products}.\n\
             Copy MCP, permissions, and hooks into WhyCodes?\n\
             Sources are never modified. Credentials stay on `whycodes auth import`."
        ),
        ConfirmAction::ImportSettings,
    );
}

/// `/import [product]` — preview + checkbox picker. Bare `/import` uses every source.
pub(super) fn handle_import_slash(app: &mut TuiApp, rest: &str) {
    if import_env_skipped() {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "Import skipped (CI / WHYCODES_SKIP_IMPORT)",
        );
        return;
    }
    let filter = match parse_product_filter(rest) {
        Ok(f) => f,
        Err(msg) => {
            app.toasts.push(crate::toast::ToastKind::Warning, msg);
            return;
        }
    };
    match prepare_import_preview(filter.as_ref()) {
        Ok(PreviewOutcome::NoneFound { looked }) => {
            app.alert(
                "Import settings",
                format!("No settings from other agents found (looked for {looked})."),
            );
        }
        Ok(PreviewOutcome::NothingApproved) => {
            app.alert("Import settings", "Nothing approved to import.");
        }
        Ok(PreviewOutcome::EmptyPlan { summary }) => {
            app.alert(
                "Import settings",
                format!("{summary}\nNothing new to write (WhyCodes already has these keys)."),
            );
        }
        Ok(PreviewOutcome::Ready { plan }) => {
            app.open_import_picker(&plan);
        }
        Err(e) => {
            app.toasts.push(
                crate::toast::ToastKind::Error,
                format!("Import failed: {e}"),
            );
        }
    }
}

#[derive(Debug)]
pub(super) enum PreviewOutcome {
    NoneFound { looked: String },
    NothingApproved,
    EmptyPlan { summary: String },
    Ready { plan: ImportPlan },
}

pub(super) fn parse_product_filter(rest: &str) -> Result<Option<Product>, String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(None);
    }
    Product::parse(rest)
        .map(Some)
        .ok_or_else(|| format!("Unknown product '{rest}' (claude, opencode, grok, codex)"))
}

fn unique_product_labels(found: &[FoundSource]) -> String {
    found
        .iter()
        .map(|f| f.product.label())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ")
}

fn looked_for(filter: Option<&Product>) -> String {
    match filter {
        Some(p) => p.label().to_string(),
        None => whycodes_import::discover::KNOWN_SOURCES
            .iter()
            .map(|s| s.product.label())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Approve every New source for this pass (home confirm / `/import` yes).
fn approve_new_sources(consent: &ConsentStore, found: &[FoundSource]) -> anyhow::Result<()> {
    for f in found {
        if f.state == SourceState::New {
            consent.approve(&f.path)?;
        }
    }
    Ok(())
}

fn rescan_approved(
    home: &Path,
    consent: &ConsentStore,
    filter: Option<&Product>,
) -> Vec<FoundSource> {
    scan_with_home(home, consent)
        .into_iter()
        .filter(|f| filter.is_none_or(|p| f.product == *p))
        .map(|mut f| {
            if f.state == SourceState::New {
                f.state = SourceState::Approved;
            }
            f
        })
        .collect()
}

pub(super) fn prepare_import_preview(filter: Option<&Product>) -> anyhow::Result<PreviewOutcome> {
    let data_dir = Config::data_dir()?;
    let consent = ConsentStore::new(&data_dir);
    let home =
        whycodes_import::discover::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut found = scan_with_home(&home, &consent);
    if let Some(p) = filter {
        found.retain(|f| f.product == *p);
    }
    if found.is_empty() {
        return Ok(PreviewOutcome::NoneFound {
            looked: looked_for(filter),
        });
    }
    let preview_found = rescan_approved(&home, &consent, filter);
    let config = Config::load().unwrap_or_default();
    let (extracted, plan) = preview(&preview_found, &config, false)?;
    if extracted.is_empty() {
        return Ok(PreviewOutcome::NothingApproved);
    }
    if plan.is_empty() {
        return Ok(PreviewOutcome::EmptyPlan {
            summary: plan.summary(),
        });
    }
    Ok(PreviewOutcome::Ready { plan })
}

/// Apply the pending import: approve New sources, merge, save, reload live config.
pub(super) async fn apply_pending_import(
    app: &mut TuiApp,
    agent: &mut Agent,
    config: &mut Config,
    project_dir: &Path,
    file_index: &std::sync::Arc<whycodes_index::WorkspaceIndex>,
) {
    let selected =
        (!app.import_picker.items.is_empty()).then_some(app.import_picker.checked.as_slice());
    match apply_import_now(config, project_dir, selected) {
        Ok(ApplyOutcome::Wrote { path, summary }) => {
            agent.apply_config(config);
            agent.load_mcp(config).await;
            refresh_sidebar(app, config, file_index);
            app.status_message = format!("Imported · {summary}");
            app.toasts.push(
                crate::toast::ToastKind::Success,
                format!("Imported · {}", path.display()),
            );
        }
        Ok(ApplyOutcome::Nothing { message }) => {
            app.toasts.push(crate::toast::ToastKind::Info, message);
        }
        Err(e) => {
            app.toasts.push(
                crate::toast::ToastKind::Error,
                format!("Import failed: {e}"),
            );
        }
    }
}

#[derive(Debug)]
pub(super) enum ApplyOutcome {
    Wrote {
        path: std::path::PathBuf,
        summary: String,
    },
    Nothing {
        message: String,
    },
}

pub(super) fn apply_import_now(
    live: &mut Config,
    project_dir: &Path,
    selected: Option<&[bool]>,
) -> anyhow::Result<ApplyOutcome> {
    let data_dir = Config::data_dir()?;
    let consent = ConsentStore::new(&data_dir);
    if let Err(e) = consent.mark_first_run_asked() {
        tracing::warn!(error = %e, "import first-run mark failed");
    }
    let home =
        whycodes_import::discover::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let found = scan_with_home(&home, &consent);
    approve_new_sources(&consent, &found)?;
    let found = rescan_approved(&home, &consent, None);
    let disk = Config::load().unwrap_or_default();
    let (extracted, mut plan) = preview(&found, &disk, false)?;
    if extracted.is_empty() {
        return Ok(ApplyOutcome::Nothing {
            message: "Nothing approved to import.".into(),
        });
    }
    if let Some(sel) = selected {
        plan.retain_selected(sel);
    }
    if plan.is_empty() {
        return Ok(ApplyOutcome::Nothing {
            message: "Nothing new to write (WhyCodes already has these keys).".into(),
        });
    }
    let path = apply_and_save(&plan)?;
    let summary = plan.summary();
    if let Ok(reloaded) = Config::load_layered(project_dir) {
        *live = reloaded;
    }
    // Merge the plan into the live session even if disk reload raced
    // (tests share `WHYCODES_HOME`) or layered load missed the new file.
    // Existing keys win, so a successful reload is a no-op here.
    whycodes_import::apply(live, &plan);
    Ok(ApplyOutcome::Wrote { path, summary })
}

/// Esc / N on the first-run confirm: remember "asked" so we do not nag.
pub(crate) fn mark_import_declined() {
    // Tests without an isolated home must not write the developer's consent file.
    if cfg!(test) && std::env::var_os("WHYCODES_HOME").is_none() {
        return;
    }
    if let Ok(data_dir) = Config::data_dir() {
        let consent = ConsentStore::new(data_dir);
        if let Err(e) = consent.mark_first_run_asked() {
            tracing::warn!(error = %e, "import decline mark failed");
        }
    }
}
