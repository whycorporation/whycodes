//! Redraw cadence — ported from jcode `redraw_schedule`.
//!
//! jcode measured a real lag bug: a static status notice ("Swarm plan
//! synced") pulled the whole client to animation FPS (~180 full frames
//! for a 3s toast). Toasts and notices are text-only; they appear on an
//! event (already `mark_dirty`) and expire on a multi-second timer.
//! Treating them as "live" burns CPU and queues keystrokes behind empty
//! paints.
//!
//! Deep idle (no user input for 30s, nothing animating) crawls to 5s so
//! a forgotten TUI does not keep waking the CPU twice a second.

use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseEventKind};

/// Quiet session: prune toasts, adopt fuzzy results.
pub const REDRAW_IDLE: Duration = Duration::from_millis(500);
/// Forgotten session: no keys/clicks for [`REDRAW_DEEP_IDLE_AFTER`].
pub const REDRAW_DEEP_IDLE: Duration = Duration::from_millis(5000);
pub const REDRAW_DEEP_IDLE_AFTER: Duration = Duration::from_secs(30);
/// Spinner / streaming / just-dirty.
pub const REDRAW_ANIMATE: Duration = Duration::from_millis(40);
/// `@file` matcher workers mid-rematch.
pub const REDRAW_FUZZY: Duration = Duration::from_millis(16);

/// Inputs the loop already has; no `TuiApp` so this stays unit-testable.
#[derive(Debug, Clone, Copy)]
pub struct RedrawNeed {
    pub agent_busy: bool,
    pub running_subagents: bool,
    pub awaiting_matches: bool,
    pub needs_redraw: bool,
    /// Visible toasts. Must **not** force animation cadence.
    pub toasts_visible: bool,
    pub since_user_input: Duration,
}

/// True when something on screen actually changes every frame (spinner,
/// streaming bubble, live subagent rail). Static chrome does not qualify.
pub fn is_animating(need: &RedrawNeed) -> bool {
    need.agent_busy || need.running_subagents
}

/// How long `event::poll` should wait before the next loop iteration.
pub fn poll_interval(need: &RedrawNeed) -> Duration {
    if need.awaiting_matches {
        return REDRAW_FUZZY;
    }
    if is_animating(need) || need.needs_redraw {
        return REDRAW_ANIMATE;
    }
    if need.since_user_input >= REDRAW_DEEP_IDLE_AFTER && !need.toasts_visible {
        REDRAW_DEEP_IDLE
    } else {
        REDRAW_IDLE
    }
}

/// Key / paste / click / wheel / resize. Mouse *move* is hover tracking and
/// must not keep a forgotten session out of deep idle (terminals flood it).
pub fn event_is_user_interaction(ev: &Event) -> bool {
    match ev {
        Event::Key(_) | Event::Paste(_) | Event::Resize(_, _) => true,
        Event::Mouse(m) => !matches!(m.kind, MouseEventKind::Moved),
        _ => false,
    }
}

/// Events that can dump glyphs onto the PTY *outside* ratatui's buffer diff.
///
/// Bracketed paste is the usual case: the emulator echoes the payload at the
/// cursor (or scrolls the alt-screen) before `Event::Paste` is delivered.
/// Breathing-room rows around the prompt stay spaces in both ratatui frames,
/// so the diff never overwrites the echo. Resize and focus-restore desync
/// the same way. The event loop must `terminal.clear()` before the next draw.
pub fn event_needs_full_clear(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Paste(_) | Event::Resize(_, _) | Event::FocusGained
    )
}

/// True when a drained batch looks like a paste delivered as a flood of
/// `Key` events (no `Event::Paste` — hosts that skip bracketed paste).
///
/// The emulator still echoes the payload at the cursor, so the leftover
/// sits left of the centered home prompt. A handful of typed chars must
/// not trip this (that would `terminal.clear()` on every word).
pub fn batch_looks_like_unbracketed_paste(batch: &[Event]) -> bool {
    const MIN_CHARS: usize = 24;
    if batch.iter().any(|e| matches!(e, Event::Paste(_))) {
        return false;
    }
    let chars = batch
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::Key(k)
                    if k.kind == KeyEventKind::Press && matches!(k.code, KeyCode::Char(_))
            )
        })
        .count();
    chars >= MIN_CHARS
}

#[cfg(test)]
#[path = "redraw_schedule_tests.rs"]
mod tests;
