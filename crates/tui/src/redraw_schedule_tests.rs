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
    assert!(!event_is_user_interaction(&Event::FocusGained));
    assert!(!event_is_user_interaction(&Event::FocusLost));
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

#[test]
fn unbracketed_paste_is_a_char_flood_not_a_few_keys() {
    let few: Vec<Event> = (0..8)
        .map(|_| Event::Key(KeyEvent::from(KeyCode::Char('a'))))
        .collect();
    assert!(!batch_looks_like_unbracketed_paste(&few));
    let many: Vec<Event> = (0..24)
        .map(|_| Event::Key(KeyEvent::from(KeyCode::Char('a'))))
        .collect();
    assert!(batch_looks_like_unbracketed_paste(&many));
    let mixed = vec![
        Event::Paste("x".into()),
        Event::Key(KeyEvent::from(KeyCode::Char('a'))),
    ];
    assert!(!batch_looks_like_unbracketed_paste(&mixed));
}
