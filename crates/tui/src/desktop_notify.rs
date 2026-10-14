//! Desktop notifications through the terminal (`[notify] desktop = true`).
//!
//! OSC 9 (iTerm2, WezTerm, Ghostty, …) or OSC 777 (rxvt, foot), wrapped for
//! tmux passthrough. Raised only while the terminal window is unfocused, so
//! focus reporting is turned on only when this is configured.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::Event;
use whycodes_config::{NotifyConfig, NotifyEvent};

/// Longest body sent to the terminal, in chars.
const MAX_BODY: usize = 180;

static TURN_DONE: AtomicBool = AtomicBool::new(false);
static NEED_INPUT: AtomicBool = AtomicBool::new(false);
static FOCUSED: AtomicBool = AtomicBool::new(true);

/// Record which events raise a desktop notification (boot, once per process).
///
/// Only switches events on: a second boot with a default config (tests share
/// the process) never silences one that is already configured.
pub(crate) fn configure(cfg: &NotifyConfig) {
    TURN_DONE.fetch_or(cfg.desktop_for(NotifyEvent::TurnDone), Ordering::Relaxed);
    NEED_INPUT.fetch_or(cfg.desktop_for(NotifyEvent::NeedInput), Ordering::Relaxed);
}

fn wanted(event: NotifyEvent) -> bool {
    match event {
        NotifyEvent::TurnDone => TURN_DONE.load(Ordering::Relaxed),
        NotifyEvent::NeedInput => NEED_INPUT.load(Ordering::Relaxed),
    }
}

/// Whether to ask the terminal for focus in/out reports.
pub(crate) fn wants_focus_reports() -> bool {
    wanted(NotifyEvent::TurnDone) || wanted(NotifyEvent::NeedInput)
}

/// Track the last focus change in a drained event batch.
pub(crate) fn note_focus_events(batch: &[Event]) {
    let last = batch.iter().rev().find_map(|e| match e {
        Event::FocusGained => Some(true),
        Event::FocusLost => Some(false),
        _ => None,
    });
    if let Some(focused) = last {
        FOCUSED.store(focused, Ordering::Relaxed);
    }
}

/// Raise a notification for `event` when configured and the window is unfocused.
pub(crate) fn notify(event: NotifyEvent, body: &str) {
    if !wanted(event) || FOCUSED.load(Ordering::Relaxed) {
        return;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    let tmux = std::env::var_os("TMUX").is_some();
    write_to(&mut std::io::stdout().lock(), &term, tmux, body);
}

fn write_to(out: &mut impl Write, term: &str, tmux: bool, body: &str) {
    let seq = sequence(term, tmux, "WhyCodes", body);
    if let Err(e) = out.write_all(seq.as_bytes()).and_then(|()| out.flush()) {
        tracing::debug!(error = %e, "desktop notification write failed");
    }
}

/// The escape sequence for this terminal.
fn sequence(term: &str, tmux: bool, title: &str, body: &str) -> String {
    let body = clean(body);
    let osc = if term.starts_with("rxvt") || term.starts_with("foot") {
        // Fields are `;`-separated; the body is the rest of the string.
        format!(
            "\x1b]777;notify;{};{body}\x07",
            clean(title).replace(';', ",")
        )
    } else {
        format!("\x1b]9;{}: {body}\x07", clean(title))
    };
    if tmux {
        // DCS passthrough: every ESC inside is doubled.
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    }
}

/// One line, no control characters, at most [`MAX_BODY`] chars.
fn clean(text: &str) -> String {
    let flat: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if flat.chars().count() > MAX_BODY {
        let mut short: String = flat.chars().take(MAX_BODY - 1).collect();
        short.push('…');
        short
    } else {
        flat
    }
}

/// Turn-finished body: the reply's first line, or a fallback.
pub(crate) fn turn_done_body(reply: &str) -> String {
    let first = reply.lines().map(str::trim).find(|l| !l.is_empty());
    match first {
        Some(line) => format!("Done: {line}"),
        None => "Turn finished".to_string(),
    }
}

/// Permission dialog body.
pub(crate) fn permission_body(tool: &str) -> String {
    format!("Approve {tool}?")
}

/// `question` tool body: the first question, or a fallback.
pub(crate) fn question_body(first_prompt: Option<&str>) -> String {
    match first_prompt.map(str::trim).filter(|p| !p.is_empty()) {
        Some(prompt) => format!("Question: {prompt}"),
        None => "Has a question for you".to_string(),
    }
}

#[cfg(test)]
#[path = "desktop_notify_tests.rs"]
mod tests;
