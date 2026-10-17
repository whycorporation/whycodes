//! Permission prompting for allow/ask/deny rules.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::notify::{NotifyHandle, spawn_need_input_wait};

/// Boxed, sendable future returned by [`PermissionPrompter::ask`].
pub type PermissionAskFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Boxed, sendable future returned by [`PermissionPrompter::ask_reply`].
pub type PermissionReplyFuture<'a> = Pin<Box<dyn Future<Output = PermissionReply> + Send + 'a>>;

/// The answer to one permission ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionReply {
    Allow,
    Deny,
    /// Denied because the user typed a message instead of answering. The
    /// message reaches the model as a steering message after the results.
    Replied,
}

impl PermissionReply {
    pub fn allowed(self) -> bool {
        self == Self::Allow
    }
}

impl From<bool> for PermissionReply {
    fn from(allow: bool) -> Self {
        if allow { Self::Allow } else { Self::Deny }
    }
}

/// Asked before running a tool when permission action is `Ask`.
pub trait PermissionPrompter: Send + Sync {
    /// Return `true` to allow the tool call, `false` to deny.
    fn ask<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionAskFuture<'a>;

    /// [`ask`](Self::ask) with the reason for a denial. Prompters that only
    /// answer yes or no keep this default.
    fn ask_reply<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionReplyFuture<'a> {
        Box::pin(async move { PermissionReply::from(self.ask(tool_name, detail).await) })
    }
}

/// A pending permission request for the TUI (or other UI) to fulfill.
pub struct PermissionRequest {
    pub tool_name: String,
    pub detail: String,
    pub reply: oneshot::Sender<PermissionReply>,
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
        Box::pin(async move { self.ask_reply(tool_name, detail).await.allowed() })
    }

    fn ask_reply<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionReplyFuture<'a> {
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
                return PermissionReply::Deny;
            }
            reply_rx.await.unwrap_or(PermissionReply::Deny)
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

/// Where a stdin prompter reads its answer line. Tests pass an empty reader
/// so they never read the process stdin: the read blocks the runtime thread,
/// a timeout cannot cancel it, and with stdin left open (a terminal or a
/// pipe) the whole agent suite hung behind the stdin lock.
pub type AnswerInput = fn() -> Box<dyn std::io::BufRead>;

/// The process stdin. Locking does not block; only reading does.
pub fn stdin_input() -> Box<dyn std::io::BufRead> {
    Box::new(std::io::stdin().lock())
}

/// Stdin y/n prompter for the plain CLI.
pub struct StdinPrompter {
    notify: Option<NotifyHandle>,
    input: AnswerInput,
}

impl Default for StdinPrompter {
    fn default() -> Self {
        Self {
            notify: None,
            input: stdin_input,
        }
    }
}

impl StdinPrompter {
    /// Read answers from `input` instead of the process stdin.
    pub fn with_input(mut self, input: AnswerInput) -> Self {
        self.input = input;
        self
    }

    pub fn with_notify(mut self, notify: NotifyHandle) -> Self {
        self.notify = Some(notify);
        self
    }
}

impl PermissionPrompter for StdinPrompter {
    fn ask<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionAskFuture<'a> {
        Box::pin(async move {
            use std::io;
            if let Some(cfg) = self.notify.as_deref() {
                spawn_need_input_wait(cfg, &format!("Permission · `{tool_name}`"), detail);
            }
            eprintln!();
            eprintln!("⚠ Permission required for tool `{tool_name}`");
            if !detail.is_empty() {
                eprintln!("  {detail}");
            }
            eprint!("  Allow? [y/N] ");
            flush_permission_prompt(&mut io::stderr());
            let mut line = String::new();
            let read = (self.input)().read_line(&mut line);
            permission_from_read(read, &line)
        })
    }
}

/// Build a prompter from environment / defaults.
/// - `WHYCODES_AUTO_APPROVE=1` → auto-allow
/// - `WHYCODES_AUTO_DENY=1` → auto-deny
/// - else stdin, when it is a terminal; otherwise auto-deny
pub fn default_prompter() -> Arc<dyn PermissionPrompter> {
    prompter_for(
        env_flag("WHYCODES_AUTO_APPROVE"),
        env_flag("WHYCODES_AUTO_DENY"),
        atty_stderr(),
    )
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// `auto_approve` wins over `auto_deny`; without either, a non-interactive
/// stdin auto-denies for safety.
pub(crate) fn prompter_for(
    auto_approve: bool,
    auto_deny: bool,
    interactive: bool,
) -> Arc<dyn PermissionPrompter> {
    if auto_approve {
        return Arc::new(AutoApprovePrompter);
    }
    if auto_deny || !interactive {
        return Arc::new(AutoDenyPrompter);
    }
    Arc::new(StdinPrompter::default())
}

fn atty_stderr() -> bool {
    use std::io::IsTerminal;
    interactive_stdin(
        std::env::var_os("CI").is_some(),
        std::io::stdin().is_terminal(),
    )
}

/// A `[y/N]` prompt only makes sense when a person can answer it: not under
/// `CI`, and not when stdin is a pipe or a file (an empty pipe would deny,
/// but a piped `y` would approve anything).
fn interactive_stdin(ci: bool, stdin_is_terminal: bool) -> bool {
    !ci && stdin_is_terminal
}

fn permission_from_read(result: std::io::Result<usize>, line: &str) -> bool {
    if result.is_err() {
        return false;
    }
    permission_line_allows(line)
}

fn permission_line_allows(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "a" | "allow"
    )
}

fn flush_permission_prompt(out: &mut dyn std::io::Write) {
    match out.flush() {
        Ok(()) => {}
        Err(e) => tracing::debug!(error = %e, "permission prompt flush skipped"),
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
