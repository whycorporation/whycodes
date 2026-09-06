//! Permission prompting for OpenCode-style allow/ask/deny.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::notify::{NotifyHandle, spawn_need_input_wait};

/// Boxed, sendable future returned by [`PermissionPrompter::ask`].
pub type PermissionAskFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Asked before running a tool when permission action is `Ask`.
pub trait PermissionPrompter: Send + Sync {
    /// Return `true` to allow the tool call, `false` to deny.
    fn ask<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionAskFuture<'a>;
}

/// A pending permission request for the TUI (or other UI) to fulfill.
pub struct PermissionRequest {
    pub tool_name: String,
    pub detail: String,
    pub reply: oneshot::Sender<bool>,
}

/// Channel-based prompter: blocks the agent until the UI replies.
pub struct ChannelPermissionPrompter {
    tx: mpsc::UnboundedSender<PermissionRequest>,
    notify: Option<NotifyHandle>,
}

impl ChannelPermissionPrompter {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<PermissionRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, notify: None }, rx)
    }

    pub fn with_notify(mut self, notify: NotifyHandle) -> Self {
        self.notify = Some(notify);
        self
    }
}

impl PermissionPrompter for ChannelPermissionPrompter {
    fn ask<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionAskFuture<'a> {
        Box::pin(async move {
            if let Some(cfg) = self.notify.as_deref() {
                spawn_need_input_wait(cfg, &format!("Permission · `{tool_name}`"), detail);
            }
            let (reply_tx, reply_rx) = oneshot::channel();
            if self
                .tx
                .send(PermissionRequest {
                    tool_name: tool_name.to_string(),
                    detail: detail.to_string(),
                    reply: reply_tx,
                })
                .is_err()
            {
                return false;
            }
            reply_rx.await.unwrap_or(false)
        })
    }
}

/// Auto-approve all asks (non-interactive / CI).
pub struct AutoApprovePrompter;

impl PermissionPrompter for AutoApprovePrompter {
    fn ask<'a>(&'a self, _tool_name: &'a str, _detail: &'a str) -> PermissionAskFuture<'a> {
        Box::pin(async move { true })
    }
}

/// Auto-deny all asks (strict non-interactive).
pub struct AutoDenyPrompter;

impl PermissionPrompter for AutoDenyPrompter {
    fn ask<'a>(&'a self, _tool_name: &'a str, _detail: &'a str) -> PermissionAskFuture<'a> {
        Box::pin(async move { false })
    }
}

/// Stdin y/n prompter for the plain CLI.
#[derive(Default)]
pub struct StdinPrompter {
    notify: Option<NotifyHandle>,
}

impl StdinPrompter {
    pub fn with_notify(mut self, notify: NotifyHandle) -> Self {
        self.notify = Some(notify);
        self
    }
}

impl PermissionPrompter for StdinPrompter {
    fn ask<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionAskFuture<'a> {
        Box::pin(async move {
            use std::io::{self, Write};
            if let Some(cfg) = self.notify.as_deref() {
                spawn_need_input_wait(cfg, &format!("Permission · `{tool_name}`"), detail);
            }
            eprintln!();
            eprintln!("⚠ Permission required for tool `{tool_name}`");
            if !detail.is_empty() {
                eprintln!("  {detail}");
            }
            eprint!("  Allow? [y/N] ");
            let _ = io::stderr().flush();
            let mut line = String::new();
            if io::stdin().read_line(&mut line).is_err() {
                return false;
            }
            permission_line_allows(&line)
        })
    }
}

/// Build a prompter from environment / defaults.
/// - `WHYCODES_AUTO_APPROVE=1` → auto-allow
/// - `WHYCODES_AUTO_DENY=1` → auto-deny
/// - else stdin
pub fn default_prompter() -> Arc<dyn PermissionPrompter> {
    if std::env::var("WHYCODES_AUTO_APPROVE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return Arc::new(AutoApprovePrompter);
    }
    if std::env::var("WHYCODES_AUTO_DENY")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return Arc::new(AutoDenyPrompter);
    }
    // Non-interactive stdin (piped) → auto-deny for safety
    if !atty_stderr() {
        return Arc::new(AutoDenyPrompter);
    }
    Arc::new(StdinPrompter::default())
}

fn atty_stderr() -> bool {
    // Avoid extra dep: heuristic via isatty isn't available on pure std;
    // treat missing TERM or CI as non-interactive.
    if std::env::var_os("CI").is_some() {
        return false;
    }
    true
}

fn permission_line_allows(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "a" | "allow"
    )
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
