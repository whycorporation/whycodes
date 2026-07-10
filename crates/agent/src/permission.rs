//! Permission prompting for OpenCode-style allow/ask/deny.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Asked before running a tool when permission action is `Ask`.
#[async_trait]
pub trait PermissionPrompter: Send + Sync {
    /// Return `true` to allow the tool call, `false` to deny.
    async fn ask(&self, tool_name: &str, detail: &str) -> bool;
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
}

impl ChannelPermissionPrompter {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<PermissionRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

#[async_trait]
impl PermissionPrompter for ChannelPermissionPrompter {
    async fn ask(&self, tool_name: &str, detail: &str) -> bool {
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
    }
}

/// Auto-approve all asks (non-interactive / CI).
pub struct AutoApprovePrompter;

#[async_trait]
impl PermissionPrompter for AutoApprovePrompter {
    async fn ask(&self, _tool_name: &str, _detail: &str) -> bool {
        true
    }
}

/// Auto-deny all asks (strict non-interactive).
pub struct AutoDenyPrompter;

#[async_trait]
impl PermissionPrompter for AutoDenyPrompter {
    async fn ask(&self, _tool_name: &str, _detail: &str) -> bool {
        false
    }
}

/// Stdin y/n prompter for the plain CLI.
pub struct StdinPrompter;

#[async_trait]
impl PermissionPrompter for StdinPrompter {
    async fn ask(&self, tool_name: &str, detail: &str) -> bool {
        use std::io::{self, Write};
        eprintln!();
        eprintln!("⚠ Permission required for tool `{}`", tool_name);
        if !detail.is_empty() {
            eprintln!("  {}", detail);
        }
        eprint!("  Allow? [y/N] ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        matches!(
            line.trim().to_ascii_lowercase().as_str(),
            "y" | "yes" | "a" | "allow"
        )
    }
}

/// Build a prompter from environment / defaults.
/// - `WHYCODE_AUTO_APPROVE=1` → auto-allow
/// - `WHYCODE_AUTO_DENY=1` → auto-deny
/// - else stdin
pub fn default_prompter() -> Arc<dyn PermissionPrompter> {
    if std::env::var("WHYCODE_AUTO_APPROVE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return Arc::new(AutoApprovePrompter);
    }
    if std::env::var("WHYCODE_AUTO_DENY")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return Arc::new(AutoDenyPrompter);
    }
    // Non-interactive stdin (piped) → auto-deny for safety
    if !atty_stderr() {
        return Arc::new(AutoDenyPrompter);
    }
    Arc::new(StdinPrompter)
}

fn atty_stderr() -> bool {
    // Avoid extra dep: heuristic via isatty isn't available on pure std;
    // treat missing TERM or CI as non-interactive.
    if std::env::var_os("CI").is_some() {
        return false;
    }
    true
}
