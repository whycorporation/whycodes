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

use crossterm::event::{Event, MouseEventKind};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};

    fn idle() -> RedrawNeed {
        RedrawNeed {
            agent_busy: false,
            running_subagents: false,
            awaiting_matches: false,
            needs_redraw: false,
            toasts_visible: false,
            since_user_input: Duration::from_secs(1),
        }
    }

    #[test]
    fn a_visible_toast_does_not_force_animation_cadence() {
        let mut need = idle();
        need.toasts_visible = true;
        assert!(
            !is_animating(&need),
            "jcode: static chrome must not count as live animation"
        );
        assert_eq!(
            poll_interval(&need),
            REDRAW_IDLE,
            "toast expiry is handled by prune()+mark_dirty, not 40ms paints"
        );
    }

    #[test]
    fn streaming_keeps_the_fast_cadence_even_with_a_toast() {
        let mut need = idle();
        need.agent_busy = true;
        need.toasts_visible = true;
        assert_eq!(poll_interval(&need), REDRAW_ANIMATE);
    }

    #[test]
    fn dirty_flag_is_a_one_shot_fast_tick() {
        let mut need = idle();
        need.needs_redraw = true;
        assert_eq!(poll_interval(&need), REDRAW_ANIMATE);
    }

    #[test]
    fn deep_idle_after_thirty_quiet_seconds() {
        let mut need = idle();
        need.since_user_input = REDRAW_DEEP_IDLE_AFTER;
        assert_eq!(poll_interval(&need), REDRAW_DEEP_IDLE);
        need.toasts_visible = true;
        assert_eq!(
            poll_interval(&need),
            REDRAW_IDLE,
            "a live toast still needs a 500ms prune tick"
        );
    }

    #[test]
    fn fuzzy_workers_win_over_everything() {
        let mut need = idle();
        need.awaiting_matches = true;
        need.agent_busy = true;
        assert_eq!(poll_interval(&need), REDRAW_FUZZY);
    }

    #[test]
    fn mouse_move_is_not_user_interaction() {
        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        let click = Event::Mouse(MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!event_is_user_interaction(&moved));
        assert!(event_is_user_interaction(&click));
        assert!(event_is_user_interaction(&Event::Key(KeyEvent::from(
            KeyCode::Char('a')
        ))));
        assert!(event_is_user_interaction(&Event::Paste("x".into())));
        assert!(event_is_user_interaction(&Event::Resize(80, 24)));
    }

    #[test]
    fn paste_resize_and_focus_need_a_full_terminal_clear() {
        assert!(event_needs_full_clear(&Event::Paste("long\ntext".into())));
        assert!(event_needs_full_clear(&Event::Resize(80, 24)));
        assert!(event_needs_full_clear(&Event::FocusGained));
        assert!(!event_needs_full_clear(&Event::FocusLost));
        assert!(!event_needs_full_clear(&Event::Key(KeyEvent::from(
            KeyCode::Char('a')
        ))));
    }
}
