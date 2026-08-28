//! Comprehensive chat transcript scroll tests (wheel, page, bar, paint).
//!
//! Loaded as `chat::scroll_tests`. Covers the paths users hit on the main
//! message list (no dialog): geometry, clamping, mouse, keyboard page, and
//! ghost-free paint after scroll.

use super::{session_line_count, visible_range};
use crate::app::{AppMode, ChatRole, DialogKind, TuiApp};
use crate::config::TuiAppConfig;
use crate::input::{self, chat_wheel_step};
use crate::keymap::KeymapContext;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

fn cfg() -> TuiAppConfig {
    TuiAppConfig::default()
}

/// Fill the app with enough wrapped lines to overflow a small viewport.
fn fill_overflowing_chat(app: &mut TuiApp, pairs: usize) {
    for i in 0..pairs {
        app.add_message(
            ChatRole::User,
            format!("user turn {i}: enough text to wrap at narrow width word word word word"),
        );
        app.add_message(
            ChatRole::Assistant,
            format!("assistant turn {i}: reply with more words for height word word word"),
        );
    }
}

fn paint_session(app: &mut TuiApp, width: u16, height: u16) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let palette = app.config.palette();
    terminal
        .draw(|f| {
            // Full area as chat viewport (no chrome) — matches unit focus on scroll.
            super::render(f, f.area(), app, &palette);
        })
        .expect("draw");
}

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

// ── Geometry ───────────────────────────────────────────────────────────

#[test]
fn visible_range_bottom_anchored_invariants() {
    let total = 100usize;
    let height = 20usize;
    // Bottom stick
    let (s, e) = visible_range(total, height, 0);
    assert_eq!((s, e), (80, 100));
    assert_eq!(e - s, height);

    // Halfway
    let (s, e) = visible_range(total, height, 40);
    assert_eq!((s, e), (40, 60));
    assert_eq!(e - s, height);

    // Top (max offset)
    let max_off = total - height;
    let (s, e) = visible_range(total, height, max_off);
    assert_eq!((s, e), (0, 20));

    // Over-scroll clamps
    let (s, e) = visible_range(total, height, max_off + 50);
    assert_eq!((s, e), (0, 20));
}

#[test]
fn visible_range_no_overflow_and_empty() {
    assert_eq!(visible_range(5, 20, 0), (0, 5));
    assert_eq!(visible_range(5, 20, 99), (0, 5));
    assert_eq!(visible_range(0, 20, 0), (0, 0));
    assert_eq!(visible_range(10, 0, 0), (0, 0));
}

// ── scroll_rows / page / top / bottom ──────────────────────────────────

#[test]
fn scroll_rows_clamps_and_toggles_auto_scroll() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 15);
    app.chat_content_width = 40;
    app.chat_viewport_rows = 8;
    let total = session_line_count(&app, 40);
    app.chat_scroll_total = total; // simulate post-paint
    let max_off = total.saturating_sub(8);
    assert!(
        max_off > 5,
        "need headroom, max_off={max_off} total={total}"
    );

    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);

    app.scroll_rows(4);
    assert_eq!(app.scroll_offset, 4);
    assert!(!app.auto_scroll);

    // Cannot go past top
    app.scroll_rows(max_off as isize + 100);
    assert_eq!(app.scroll_offset, max_off);
    assert!(!app.auto_scroll);

    // Down toward bottom re-enables auto_scroll at 0
    app.scroll_rows(-(max_off as isize) - 10);
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);
}

#[test]
fn scroll_rows_prefers_paint_total_over_live_layout() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 10);
    app.chat_content_width = 40;
    app.chat_viewport_rows = 5;
    // Lie: paint said only 20 rows (e.g. stale/shrunk). Live layout is larger.
    app.chat_scroll_total = 20;
    let live = session_line_count(&app, 40);
    assert!(live > 20, "live should exceed paint total for this test");

    app.scroll_rows(100);
    // max_off from paint total: 20 - 5 = 15
    assert_eq!(app.scroll_offset, 15);
}

#[test]
fn scroll_to_top_and_bottom_and_page() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 12);
    app.chat_content_width = 40;
    app.chat_viewport_rows = 8;
    let total = session_line_count(&app, 40);
    app.chat_scroll_total = total;
    let max_off = total.saturating_sub(8);
    assert!(max_off > 8);

    app.scroll_to_top();
    assert_eq!(app.scroll_offset, max_off);
    assert!(!app.auto_scroll);
    assert_eq!(app.selected_msg, Some(0));

    app.scroll_page(false); // page toward newer
    assert!(app.scroll_offset < max_off);

    app.scroll_to_bottom();
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);

    app.scroll_page(true); // page toward older
    assert!(app.scroll_offset >= 8.min(max_off));
    assert!(!app.auto_scroll);
}

#[test]
fn clamp_after_paint_when_total_shrinks() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 10);
    app.chat_content_width = 40;
    app.chat_viewport_rows = 8;
    let total = session_line_count(&app, 40);
    app.chat_scroll_total = total;
    app.scroll_to_top();
    let big_off = app.scroll_offset;
    assert!(big_off > 5);

    // Simulate resize/collapse: paint reports a much smaller document.
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 8,
    };
    app.apply_chat_paint(area, None, 12); // max_off = 12-8 = 4
    assert!(
        app.scroll_offset <= 4,
        "offset should clamp, got {}",
        app.scroll_offset
    );
    assert!(app.scroll_offset < big_off);
}

// ── Mouse wheel ────────────────────────────────────────────────────────

#[test]
fn mouse_wheel_scrolls_transcript_both_directions() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 20);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        },
        Some(Rect {
            x: 38,
            y: 0,
            width: 2,
            height: 10,
        }),
        total,
    );
    let step = chat_wheel_step(&app) as usize;
    assert!((3..=12).contains(&step));

    let before = app.scroll_offset;
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollUp, 5, 3)
    ));
    assert_eq!(app.scroll_offset, before + step);
    assert!(!app.auto_scroll);
    assert!(app.needs_redraw);

    let mid = app.scroll_offset;
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollDown, 5, 3)
    ));
    assert_eq!(app.scroll_offset, mid.saturating_sub(step));
}

#[test]
fn mouse_wheel_at_bottom_down_is_noop_up_moves() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 15);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        },
        None,
        total,
    );
    assert_eq!(app.scroll_offset, 0);

    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollDown, 2, 2)
    ));
    assert_eq!(app.scroll_offset, 0, "already at bottom");
    assert!(app.auto_scroll);

    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollUp, 2, 2)
    ));
    assert!(app.scroll_offset > 0);
}

#[test]
fn mouse_wheel_no_overflow_stays_at_zero() {
    let mut app = TuiApp::new(cfg());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "yo");
    // Huge viewport, tiny content
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 40,
        },
        None,
        4,
    );
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollUp, 1, 1)
    ));
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);
}

#[test]
fn mouse_wheel_works_from_prompt_area_coordinates() {
    // Wheel should scroll chat even when pointer is over the prompt (common UX).
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 12);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 12,
        },
        None,
        total,
    );
    // Far below chat_area (simulating prompt row)
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollUp, 5, 50)
    ));
    assert!(app.scroll_offset > 0);
}

// ── Scrollbar drag / track ─────────────────────────────────────────────

#[test]
fn chat_scrollbar_track_click_and_drag_and_release() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 20);
    let total = session_line_count(&app, 40);
    let height = 10u16;
    let area = Rect {
        x: 2,
        y: 1,
        width: 40,
        height,
    };
    let bar = Rect {
        x: 40,
        y: 1,
        width: 2,
        height,
    };
    app.apply_chat_paint(area, Some(bar), total);
    let max_off = total.saturating_sub(height as usize);
    assert!(max_off > 2);

    // Track top → oldest
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::Down(MouseButton::Left), 40, 1)
    ));
    assert_eq!(
        app.scroll_offset, max_off,
        "top of track must be exact document top"
    );
    assert!(app.chat_scrollbar_grab.is_some());
    assert!(app.mouse_sel.is_none(), "bar click must not start text sel");

    // Drag toward bottom of track → exact newest (offset 0), not "almost bottom"
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::Drag(MouseButton::Left), 40, 1 + height - 1)
    ));
    assert_eq!(
        app.scroll_offset, 0,
        "bottom of track must be exact document bottom"
    );
    assert!(app.auto_scroll);

    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::Up(MouseButton::Left), 40, 1 + height - 1)
    ));
    assert!(app.chat_scrollbar_grab.is_none());
}

#[test]
fn chat_scrollbar_bottom_click_is_exact_bottom_even_with_mid_thumb_grab() {
    // Regression: track-click used grab=thumb_len/2, so the last track cell
    // mapped to scroll_offset > 0 and the latest lines never appeared.
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 25);
    let total = session_line_count(&app, 40);
    let height = 12u16;
    let bar = Rect {
        x: 39,
        y: 0,
        width: 2,
        height,
    };
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height,
        },
        Some(bar),
        total,
    );
    let max_off = total.saturating_sub(height as usize);
    assert!(max_off > 5);

    // Start from top so we can observe the jump.
    app.scroll_to_top();
    assert_eq!(app.scroll_offset, max_off);

    // Press last track cell (simulates track click away from current thumb).
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::Down(MouseButton::Left), 39, height - 1)
    ));
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);

    // Full paint: bottom of buffer must show the newest marker-ish content.
    app.add_message(ChatRole::Assistant, "BOTTOM_EXACT_MARKER_ZZZ");
    // Recount after new message
    let total2 = session_line_count(&app, 40);
    app.chat_scroll_total = total2;
    app.scroll_to_bottom();
    let backend = TestBackend::new(40, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("BOTTOM_EXACT") || text.contains("MARKER_ZZZ"),
        "at offset 0 the latest message must be painted: {text:.240}"
    );
}

#[test]
fn chat_scrollbar_hit_is_gutter_wide() {
    let mut app = TuiApp::new(cfg());
    let bar = Rect {
        x: 39,
        y: 0,
        width: 1,
        height: 10,
    };
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        },
        Some(bar),
        50,
    );
    assert!(app.chat_scrollbar_contains(39, 3));
    assert!(!app.chat_scrollbar_contains(38, 3));
}

// ── Dialog / help isolation ────────────────────────────────────────────

#[test]
fn dialog_open_wheel_does_not_scroll_chat() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 10);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        },
        None,
        total,
    );
    app.scroll_rows(6);
    let frozen = app.scroll_offset;

    app.mode = AppMode::Dialog;
    app.key_context = KeymapContext::Dialog;
    app.dialogs.push(DialogKind::SessionList);
    app.session_list.sessions = vec![crate::app::SessionEntry {
        id: "a".into(),
        title: "t".into(),
        messages: 1,
        updated_at: None,
        live: None,
    }];
    app.session_list.selected = 0;

    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollUp, 10, 10)
    ));
    assert_eq!(
        app.scroll_offset, frozen,
        "dialog wheel must not move chat offset"
    );
}

#[test]
fn help_mode_wheel_scrolls_help_not_chat() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 8);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        },
        None,
        total,
    );
    app.scroll_rows(5);
    let chat_off = app.scroll_offset;

    app.mode = AppMode::Help;
    app.help_scroll = 0;
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::ScrollDown, 5, 5)
    ));
    assert_eq!(app.help_scroll, 3);
    assert_eq!(app.scroll_offset, chat_off);
}

#[test]
fn modal_close_button_click_dismisses_help() {
    use crossterm::event::MouseButton;
    let mut app = TuiApp::new(cfg());
    app.mode = AppMode::Help;
    app.key_context = KeymapContext::Help;
    // Simulate last paint: close control on the top border.
    app.apply_modal_chrome(
        Some(Rect {
            x: 50,
            y: 4,
            width: 7,
            height: 1,
        }),
        Rect {
            x: 10,
            y: 4,
            width: 50,
            height: 20,
        },
        None,
    );
    assert!(app.dialog_close_contains(52, 4));
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::Down(MouseButton::Left), 52, 4)
    ));
    assert_eq!(app.mode, AppMode::Normal, " [✗] Down must dismiss Help");
    assert!(!app.modal_is_open());
}

#[test]
fn modal_close_button_click_dismisses_dialog() {
    use crossterm::event::MouseButton;
    let mut app = TuiApp::new(cfg());
    app.mode = AppMode::Dialog;
    app.key_context = KeymapContext::Dialog;
    app.dialogs.push(DialogKind::Confirm {
        title: "Quit".into(),
        message: "Sure?".into(),
        on_confirm: crate::app::ConfirmAction::Quit,
    });
    app.apply_modal_chrome(
        Some(Rect {
            x: 40,
            y: 6,
            width: 7,
            height: 1,
        }),
        Rect {
            x: 5,
            y: 6,
            width: 45,
            height: 12,
        },
        None,
    );
    assert!(input::handle_event(
        &mut app,
        mouse(MouseEventKind::Down(MouseButton::Left), 42, 6)
    ));
    assert!(!app.dialogs.is_open());
    assert_eq!(app.mode, AppMode::Normal);
}

// ── Keyboard page (from either focus) ──────────────────────────────────

#[test]
fn page_up_down_keys_scroll_transcript() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 15);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        },
        None,
        total,
    );

    let page_up = Event::Key(KeyEvent {
        code: KeyCode::PageUp,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    });
    assert!(input::handle_event(&mut app, page_up));
    assert!(app.scroll_offset >= 8.min(total.saturating_sub(8)));

    let page_down = Event::Key(KeyEvent {
        code: KeyCode::PageDown,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    });
    let mid = app.scroll_offset;
    assert!(input::handle_event(&mut app, page_down));
    assert!(app.scroll_offset < mid || mid == 0);
}

// ── Full paint: scroll actually changes buffer content ─────────────────

#[test]
fn full_paint_scroll_changes_buffer_and_leaves_no_ghosts() {
    let mut app = TuiApp::new(cfg());
    // Distinct markers at start vs end of transcript
    app.add_message(ChatRole::User, "AAA_TOP_MARKER unique alpha");
    for i in 0..25 {
        app.add_message(
            ChatRole::User,
            format!("mid filler {i} word word word word"),
        );
        app.add_message(ChatRole::Assistant, format!("mid reply {i} word word word"));
    }
    app.add_message(ChatRole::Assistant, "ZZZ_BOTTOM_MARKER unique omega");

    let w = 48u16;
    let h = 16u16;
    paint_session(&mut app, w, h);
    assert!(app.chat_scroll_total > h as usize);
    assert_eq!(app.scroll_offset, 0);

    // Capture bottom frame fingerprint (should include bottom marker somewhere)
    let bottom_snap = paint_and_snapshot(&mut app, w, h);
    let bottom_text = buffer_text(&bottom_snap);
    assert!(
        bottom_text.contains("ZZZ_BOTTOM") || bottom_text.contains("omega"),
        "bottom view should show latest content, got: {bottom_text:.200}"
    );

    app.scroll_to_top();
    let top_snap = paint_and_snapshot(&mut app, w, h);
    let top_text = buffer_text(&top_snap);
    assert!(
        top_text.contains("AAA_TOP") || top_text.contains("alpha"),
        "top view should show oldest content, got: {top_text:.200}"
    );
    assert_ne!(
        bottom_text, top_text,
        "scrolling must change painted content"
    );

    // Ghost check: after scrolling to top, bottom marker must not remain
    assert!(
        !top_text.contains("ZZZ_BOTTOM"),
        "ghost of bottom content after scroll-to-top"
    );

    app.scroll_to_bottom();
    let back = paint_and_snapshot(&mut app, w, h);
    let back_text = buffer_text(&back);
    assert!(
        !back_text.contains("AAA_TOP"),
        "ghost of top content after scroll-to-bottom"
    );
}

#[test]
fn paint_publishes_scrollbar_when_overflowing() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 20);
    paint_session(&mut app, 40, 12);
    assert!(app.chat_scroll_total > 12);
    assert!(
        app.chat_scrollbar_hit.is_some(),
        "overflowing chat must expose scrollbar hit"
    );
    assert!(app.chat_area.is_some());
    let hit = app.chat_scrollbar_hit.expect("bar");
    assert_eq!(hit.x + hit.width, 40, "bar sits on the right edge");
    assert_eq!(
        app.chat_content_width,
        40 - crate::ui::scrollbar::SCROLLBAR_GUTTER - crate::ui::scrollbar::SCROLLBAR_GAP,
        "text wrap must reserve the scrollbar gap + gutter"
    );
}

#[test]
fn painted_scrollbar_thumb_at_bottom_when_scroll_offset_zero() {
    // Regression: ratatui Scrollbar left the thumb near ~70% at true bottom.
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 30);
    let w = 40u16;
    let h = 16u16;
    app.scroll_to_bottom();
    assert_eq!(app.scroll_offset, 0);

    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();

    let buf = terminal.backend().buffer();
    let bar_x = w - 1;
    let bottom_y = h - 1;
    // Bottom track cell must be the thumb (accent/scrollbar), not empty track-only.
    let bottom = buf.cell((bar_x, bottom_y)).expect("bottom bar cell");
    let top = buf.cell((bar_x, 0)).expect("top bar cell");
    // At bottom, last cell is thumb; top cell should differ (track) when content
    // is long enough that the thumb doesn't fill the whole track.
    assert_eq!(
        bottom.symbol(),
        "█",
        "scrollbar column should paint solid cells"
    );
    if app.chat_scroll_total > (h as usize) * 3 {
        assert_ne!(
            bottom.bg, top.bg,
            "at document bottom, thumb (bottom) and track (top) must differ; both={:?}",
            bottom.bg
        );
    }
}

#[test]
fn overflowing_chat_does_not_paint_text_under_scrollbar() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 20);
    let w = 40u16;
    let h = 12u16;
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();
    let buf = terminal.backend().buffer();
    let bar_x = w - 1;
    for y in 0..h {
        let cell = buf.cell((bar_x, y)).expect("bar cell");
        assert_eq!(
            cell.symbol(),
            "█",
            "rightmost column must be the scrollbar, not transcript text at y={y}"
        );
    }
    let hit = app.chat_scrollbar_hit.expect("bar hit");
    assert_eq!(hit.x, bar_x);
    assert_eq!(hit.width, crate::ui::scrollbar::SCROLLBAR_GUTTER);
    assert_eq!(
        app.chat_content_width,
        w - crate::ui::scrollbar::SCROLLBAR_GUTTER - crate::ui::scrollbar::SCROLLBAR_GAP
    );
    for y in 0..h {
        let gap = buf.cell((bar_x.saturating_sub(1), y)).expect("gap cell");
        assert_eq!(
            gap.symbol(),
            " ",
            "column left of the bar must be the blank gap at y={y}"
        );
    }
}

#[test]
fn paint_home_clears_chat_hits() {
    let mut app = TuiApp::new(cfg());
    // Non-empty first so hits get set, then clear messages
    fill_overflowing_chat(&mut app, 5);
    paint_session(&mut app, 40, 12);
    assert!(app.chat_area.is_some());

    app.messages.clear();
    paint_session(&mut app, 40, 12);
    assert!(app.chat_area.is_none());
    assert!(app.chat_scrollbar_hit.is_none());
    assert_eq!(app.chat_scroll_total, 0);
}

#[test]
fn paint_home_shows_question_mark_without_wordmark() {
    let mut app = TuiApp::new(cfg());
    app.project_label = "whycodes".into();
    let buf = paint_and_snapshot(&mut app, 80, 24);
    let text = buffer_text(&buf);
    assert!(
        text.contains("▄█████▄"),
        "home must paint the landing `?` bowl: {text}"
    );
    assert!(
        text.contains("███▀ ▀███"),
        "home `?` must keep the left-open bowl: {text}"
    );
    assert!(
        !text.contains("█   ██▀▀▀"),
        "home must not paint the WHYCODES block wordmark: {text}"
    );
    assert!(
        !text.contains("█   █ █▀▀▀"),
        "home must not paint a spaced Why Codes wordmark: {text}"
    );
    assert!(
        !text.contains("whycodes"),
        "home body must not repeat the project label under the model: {text}"
    );
}

#[test]
fn scroll_paints_reuse_closed_line_cache() {
    use crate::app::AgentState;
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 12);
    paint_session(&mut app, 40, 12);
    assert!(
        app.messages
            .iter()
            .all(|m| m.line_cache.is_some() && m.layout_cache.is_some()),
        "first paint must fill closed-message caches"
    );
    let before: Vec<usize> = app
        .messages
        .iter()
        .map(|m| m.line_cache.as_ref().map(|(_, _, l)| l.len()).unwrap_or(0))
        .collect();

    app.current_agent_state = AgentState::Generating;
    app.scroll_rows(4);
    paint_session(&mut app, 40, 12);
    app.scroll_rows(-3);
    paint_session(&mut app, 40, 12);

    for (i, msg) in app.messages.iter().enumerate() {
        let last = i + 1 == app.messages.len();
        if last {
            // Live tail while busy: height may remasure, but earlier
            // bubbles must keep the same cached line count.
            continue;
        }
        assert!(msg.line_cache.is_some(), "msg {i} lost line_cache");
        assert_eq!(
            msg.line_cache.as_ref().unwrap().2.len(),
            before[i],
            "msg {i} cache rebuilt during scroll"
        );
    }
}

#[test]
fn coalesce_chat_wheels_sums_notches_into_one_scroll() {
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 20);
    let total = session_line_count(&app, 40);
    app.apply_chat_paint(
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        },
        None,
        total,
    );
    let step = chat_wheel_step(&app);
    let mut events = vec![
        mouse(MouseEventKind::Moved, 4, 4),
        mouse(MouseEventKind::ScrollUp, 4, 4),
        mouse(MouseEventKind::ScrollUp, 4, 4),
        mouse(MouseEventKind::ScrollUp, 4, 4),
        mouse(MouseEventKind::Moved, 5, 5),
        mouse(MouseEventKind::ScrollDown, 5, 5),
    ];
    input::coalesce_chat_wheels(&mut app, &mut events);
    assert_eq!(app.scroll_offset, (step * 2) as usize);
    assert_eq!(events.len(), 2, "only Moved events should remain");
    assert!(
        events
            .iter()
            .all(|e| matches!(e, Event::Mouse(m) if m.kind == MouseEventKind::Moved))
    );
}

#[test]
fn selected_scrollback_paint_reuses_line_cache() {
    use crate::app::FocusPane;
    let mut app = TuiApp::new(cfg());
    fill_overflowing_chat(&mut app, 8);
    paint_session(&mut app, 40, 12);
    let before: Vec<usize> = app
        .messages
        .iter()
        .map(|m| m.line_cache.as_ref().map(|(_, _, l)| l.len()).unwrap_or(0))
        .collect();
    app.focus = FocusPane::Scrollback;
    app.selected_msg = Some(0);
    app.scroll_rows(3);
    paint_session(&mut app, 40, 12);
    for (i, msg) in app.messages.iter().enumerate() {
        assert!(msg.line_cache.is_some(), "msg {i} lost line_cache");
        assert_eq!(
            msg.line_cache.as_ref().unwrap().2.len(),
            before[i],
            "selected paint must not rebuild msg {i}"
        );
    }
}

#[test]
fn wheel_step_scales_with_viewport() {
    let mut app = TuiApp::new(cfg());
    app.chat_viewport_rows = 6;
    assert_eq!(chat_wheel_step(&app), 3); // clamp min
    app.chat_viewport_rows = 30;
    assert_eq!(chat_wheel_step(&app), 10); // 30/3
    app.chat_viewport_rows = 60;
    assert_eq!(chat_wheel_step(&app), 12); // clamp max
}

// ── helpers ────────────────────────────────────────────────────────────

fn paint_and_snapshot(app: &mut TuiApp, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("term");
    let palette = app.config.palette();
    terminal
        .draw(|f| {
            super::render(f, f.area(), app, &palette);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area();
    let mut out = String::new();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}
