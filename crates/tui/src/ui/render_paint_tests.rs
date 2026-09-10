use super::*;
use crate::app::{ChatBlock, ThinkingBlock, TuiApp};
use crate::config::TuiAppConfig;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

fn app() -> TuiApp {
    TuiApp::new(TuiAppConfig::default())
}

fn paint<F>(width: u16, height: u16, f: F) -> (ratatui::buffer::Buffer, String)
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(f).expect("draw");
    let buf = terminal.backend().buffer().clone();
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
    (buf, out)
}

#[test]
fn recovered_frame_paints_the_banner() {
    let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
    let (_, text) = paint(40, 4, |f| {
        paint_recovered_frame(f, payload.as_ref());
    });
    assert!(text.contains("rendering error recovered"), "{text:?}");
    assert!(text.contains("boom"), "{text:?}");

    let owned: Box<dyn std::any::Any + Send> = Box::new("owned boom".to_string());
    let (_, owned_text) = paint(40, 4, |f| {
        paint_recovered_frame(f, owned.as_ref());
    });
    assert!(
        owned_text.contains("owned boom"),
        "String panic payloads must paint, got {owned_text:?}"
    );

    let unknown: Box<dyn std::any::Any + Send> = Box::new(42u32);
    let (_, unknown_text) = paint(40, 4, |f| {
        paint_recovered_frame(f, unknown.as_ref());
    });
    assert!(
        unknown_text.contains("unknown panic") || unknown_text.contains("recovered"),
        "non-string payloads fall back to unknown panic, got {unknown_text:?}"
    );

    let (_, empty) = paint(0, 0, |f| {
        paint_recovered_frame(f, payload.as_ref());
    });
    assert!(
        empty.trim().is_empty(),
        "a 0×0 recovered frame must skip paint, got {empty:?}"
    );
}

#[test]
fn home_shell_renders_prompt_and_records_chat_viewport() {
    let mut a = app();
    a.input_buffer = "deterministic draft".into();
    a.input_cursor = a.input_buffer.len();

    let (buffer, text) = paint_full_shell(&mut a, 80, 24);

    assert!(text.contains("deterministic draft"), "{text:?}");
    assert!(text.contains('╭') && text.contains('╯'), "{text:?}");
    assert!(
        text.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))),
        "home shows product version: {text:?}"
    );
    assert!(a.chat_viewport_rows > 0);
    assert_eq!(a.chat_content_width, buffer.area().width.saturating_sub(2));
}

#[test]
fn turn_status_generating_shows_spinner_label_tokens_and_stop() {
    let mut a = app();
    a.current_agent_state = AgentState::Generating;
    a.spinner_frame = 2;
    a.status_message = "Running tool `read`…".into();
    a.turn_usage = Some(whycodes_core::types::Usage {
        input_tokens: 1_200,
        output_tokens: 80,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    let palette = a.config.palette();
    let (buf, text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    assert!(text.contains("generating"), "{text}");
    assert!(text.contains("tool: read"), "{text}");
    assert!(text.contains("⇣1.3k"), "tokens shown: {text}");
    assert!(text.contains("[stop]"), "{text}");
    // [stop] right-aligned with its hit rect recorded.
    let rect = a.turn_stop_hit.rect.expect("stop hit rect");
    assert_eq!(rect.y, 0);
    assert_eq!(rect.x + rect.width, 100, "stop sits at the right edge");
    // Spinner glyph painted.
    let _ = buf;
}

#[test]
fn turn_status_thinking_uses_running_block_elapsed() {
    let mut a = app();
    let mut tb = ThinkingBlock::new("reasoning");
    tb.collapsed = false;
    a.messages.push(chat_message_with_thinking(tb));
    a.current_agent_state = AgentState::Thinking;
    a.spinner_frame = 4;
    let palette = a.config.palette();
    let (_buf, text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    assert!(text.contains("thinking"), "{text}");
    assert!(text.contains("0.0s") || text.contains("thinking"), "{text}");
}

#[test]
fn turn_status_waiting_and_error_labels() {
    let palette = app().config.palette();

    let mut a = app();
    a.current_agent_state = AgentState::WaitingForPermission;
    let (_buf, text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    assert!(text.contains("waiting for permission"), "{text}");

    let mut a = app();
    a.current_agent_state = AgentState::WaitingForQuestion;
    let (_buf, text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    assert!(text.contains("waiting for answer"), "{text}");

    let mut a = app();
    a.current_agent_state = AgentState::Error("boom".into());
    let (_buf, text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    assert!(text.contains("error"), "{text}");

    let mut a = app();
    a.current_agent_state = AgentState::Idle;
    let (_buf, text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    assert!(text.contains("ready"), "{text}");
}

#[test]
fn turn_status_truncates_long_detail_and_stops() {
    let mut a = app();
    a.current_agent_state = AgentState::Generating;
    a.status_message = format!("long status {}", "x".repeat(120));
    let palette = a.config.palette();
    let (_buf, text) = paint(60, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    // Detail truncated to fit the left budget, [stop] still visible.
    assert!(text.contains("…"), "{text}");
    assert!(text.contains("[stop]"), "{text}");
}

#[test]
fn turn_status_hovered_stop_underlines() {
    let mut a = app();
    a.current_agent_state = AgentState::Generating;
    a.turn_stop_hit.hovered = true;
    let palette = a.config.palette();
    let (buf, _text) = paint(100, 1, |f| {
        render_turn_status(f, f.area(), &mut a, &palette);
    });
    // Find the [stop] cell and check it is underlined when hovered.
    let found = (0..100u16).any(|x| {
        let cell = buf.cell((x, 0)).unwrap();
        cell.symbol() == "[" && cell.style().add_modifier.contains(Modifier::UNDERLINED)
    });
    assert!(found, "hovered stop must be underlined");
}

fn chat_message_with_thinking(tb: ThinkingBlock) -> crate::app::ChatMessage {
    crate::app::ChatMessage {
        role: crate::app::ChatRole::Assistant,
        content: String::new(),
        blocks: vec![ChatBlock::Thinking(tb)],
        results_expanded: false,
        tool_calls: vec![],
        error: None,
        duration_ms: None,
        image_labels: vec![],
        created_at: None,
        layout_cache: None,
        line_cache: None,
        stream_md: None,
    }
}

#[test]
fn paint_selection_reverses_selected_cells() {
    let mut a = app();
    // Selection across the first five columns of row 0.
    a.mouse_sel = Some(crate::app::MouseSelection {
        anchor_x: 0,
        anchor_y: 0,
        focus_x: 4,
        focus_y: 0,
        dragging: true,
    });
    let (buf, _) = paint(20, 3, |f| {
        // Paint some text first, then apply the selection.
        f.render_widget(
            ratatui::widgets::Paragraph::new("hello world"),
            Rect::new(0, 0, 20, 3),
        );
        paint_selection(f, &a);
    });
    for x in 0..=4u16 {
        assert!(
            buf.cell((x, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "col {x} must be reversed"
        );
    }
    // Outside the selection → untouched.
    assert!(
        !buf.cell((6, 0))
            .unwrap()
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "col 6 outside selection"
    );
}

#[test]
fn paint_selection_multi_row_and_clip() {
    let mut a = app();
    a.mouse_sel = Some(crate::app::MouseSelection {
        anchor_x: 0,
        anchor_y: 0,
        focus_x: 3,
        focus_y: 1,
        dragging: true,
    });
    // Modal clip covers only row 0..0 (row 1 excluded from selection).
    a.dialog_modal_hit = Some(Rect::new(0, 0, 20, 1));
    let (buf, _) = paint(20, 3, |f| {
        f.render_widget(
            ratatui::widgets::Paragraph::new("abcd\nefgh"),
            Rect::new(0, 0, 20, 3),
        );
        paint_selection(f, &a);
    });
    assert!(
        buf.cell((1, 0))
            .unwrap()
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "row 0 inside clip"
    );
    assert!(
        !buf.cell((1, 1))
            .unwrap()
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "row 1 clipped out"
    );
}

#[test]
fn paint_selection_uses_screen_cells_when_available() {
    let mut a = app();
    a.mouse_sel = Some(crate::app::MouseSelection {
        anchor_x: 0,
        anchor_y: 0,
        focus_x: 2,
        focus_y: 0,
        dragging: true,
    });
    // Non-empty snapshot → clipboard paint_ranges_clipped path.
    a.screen_cells = crate::cell_grid::CellGrid::from_rows(vec![vec![
        "a".into(),
        "b".into(),
        "c".into(),
        "d".into(),
    ]]);
    let (buf, _) = paint(20, 3, |f| {
        f.render_widget(
            ratatui::widgets::Paragraph::new("hello world"),
            Rect::new(0, 0, 20, 3),
        );
        paint_selection(f, &a);
    });
    assert!(
        buf.cell((0, 0))
            .unwrap()
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "screen-cells path paints selection"
    );
}

#[test]
fn paint_selection_none_is_noop() {
    let mut a = app();
    a.mouse_sel = None;
    let (buf, _) = paint(20, 3, |f| {
        f.render_widget(
            ratatui::widgets::Paragraph::new("hello"),
            Rect::new(0, 0, 20, 3),
        );
        paint_selection(f, &a);
    });
    assert!(
        !buf.cell((1, 0))
            .unwrap()
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "no selection → nothing reversed"
    );
}

fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    let area = buf.area();
    let mut out = String::new();
    for x in area.x..area.x.saturating_add(area.width) {
        if let Some(cell) = buf.cell((x, y)) {
            out.push_str(cell.symbol());
        }
    }
    out
}

fn session_with_overflow() -> TuiApp {
    let mut a = app();
    a.provider_name = "anthropic".into();
    a.model_name = "claude".into();
    a.project_label = "whycodes".into();
    // Unique markers so we can see which row they land on.
    a.add_message(
        crate::app::ChatRole::User,
        "SAFEAREA_TOP_MARKER unique user prompt",
    );
    a.add_message(
        crate::app::ChatRole::Assistant,
        "SAFEAREA_ASSIST unique assistant reply that wraps a bit more text",
    );
    for i in 0..12 {
        a.add_message(crate::app::ChatRole::User, format!("later user turn {i}"));
        a.add_message(
            crate::app::ChatRole::Assistant,
            format!("later assistant turn {i} filler words for height"),
        );
    }
    a
}

fn paint_full_shell(app: &mut TuiApp, w: u16, h: u16) -> (ratatui::buffer::Buffer, String) {
    paint(w, h, |f| {
        crate::ui::render(f, app);
    })
}

#[test]
fn session_chat_stays_below_header_and_safe_area() {
    let mut a = session_with_overflow();
    let (buf, _text) = paint_full_shell(&mut a, 100, 24);

    let safe_top = row_text(&buf, 0);
    let header = row_text(&buf, layout::SAFE_TOP);

    // Terminal edge (SAFE_TOP) is empty of chrome and of chat.
    assert!(
        !safe_top.contains("why"),
        "safe-area row must not hold the header: {safe_top:?}"
    );
    assert!(
        !safe_top.contains("SAFEAREA_TOP_MARKER"),
        "safe-area row must not hold chat: {safe_top:?}"
    );

    // Status header sits on the first inset row and is not overwritten.
    assert!(
        header.contains("whycodes"),
        "header brand missing: {header:?}"
    );
    assert!(
        !header.contains("why codes"),
        "wordmark must be one word: {header:?}"
    );
    assert!(
        !header.contains("SAFEAREA_TOP_MARKER"),
        "chat spilled into the header: {header:?}"
    );
    assert!(
        !header.contains('\u{276F}'),
        "user-prompt arrow spilled into the header: {header:?}"
    );

    // TOP_PAD blank rows between header and transcript.
    for dy in 1..=layout::TOP_PAD {
        let gap = row_text(&buf, layout::SAFE_TOP + dy);
        assert!(
            !gap.contains("SAFEAREA_TOP_MARKER"),
            "chat spilled into header gap row {dy}: {gap:?}"
        );
        assert!(
            !gap.contains('\u{276F}'),
            "user-prompt arrow spilled into header gap row {dy}: {gap:?}"
        );
    }

    let chat = a.chat_area.expect("session publishes a chat hit rect");
    assert!(
        chat.y >= layout::SAFE_TOP + layout::HEADER_H + layout::TOP_PAD,
        "chat.y={chat:?} must sit below header + TOP_PAD"
    );
    assert!(
        chat.y > layout::SAFE_TOP,
        "chat must not overlap the header row"
    );
}

#[test]
fn session_scrolled_to_top_still_clears_header() {
    let mut a = session_with_overflow();
    // First paint publishes viewport metrics; then pin to oldest lines.
    let (_buf, _) = paint_full_shell(&mut a, 100, 24);
    a.scroll_offset = usize::MAX;
    a.clamp_chat_scroll();
    let (buf, _) = paint_full_shell(&mut a, 100, 24);

    let header = row_text(&buf, layout::SAFE_TOP);
    assert!(
        header.contains("whycodes"),
        "header lost after scroll: {header:?}"
    );
    assert!(
        !header.contains("SAFEAREA_TOP_MARKER"),
        "scrolled chat spilled into the header: {header:?}"
    );
    assert!(
        !header.contains('\u{276F}'),
        "scrolled user band spilled into the header: {header:?}"
    );

    for dy in 1..=layout::TOP_PAD {
        let gap = row_text(&buf, layout::SAFE_TOP + dy);
        assert!(
            !gap.contains("SAFEAREA_TOP_MARKER"),
            "scrolled chat spilled into header gap row {dy}: {gap:?}"
        );
    }

    // Oldest user prompt is visible somewhere below the gap, not above it.
    let mut found = false;
    for y in (layout::SAFE_TOP + layout::HEADER_H + layout::TOP_PAD)..buf.area().height {
        if row_text(&buf, y).contains("SAFEAREA_TOP_MARKER") {
            found = true;
            break;
        }
    }
    assert!(found, "expected oldest user prompt in the chat body");
}

#[test]
fn first_user_bubble_shows_clock_on_the_right() {
    let mut a = app();
    a.add_message(crate::app::ChatRole::User, "FIRST_BUBBLE_MARKER hello");
    let (buf, _) = paint_full_shell(&mut a, 100, 24);
    let mut found = None;
    for y in 0..buf.area().height {
        let row = row_text(&buf, y);
        if row.contains("FIRST_BUBBLE_MARKER") {
            found = Some(row);
            break;
        }
    }
    let row = found.expect("first user bubble should be painted");
    assert!(
        row.contains('\u{276F}'),
        "first bubble must be the ❯ prompt row: {row:?}"
    );
    let marker_at = row.find("FIRST_BUBBLE_MARKER").expect("marker");
    let clock_at = row.rfind(':').expect("HH:MM clock on the first bubble");
    assert!(
        clock_at > marker_at,
        "clock must sit to the right of the first bubble, got {row:?}"
    );
}

#[test]
fn assistant_reply_shows_clock_on_the_right() {
    let mut a = app();
    a.add_message(crate::app::ChatRole::User, "ask");
    a.add_message(
        crate::app::ChatRole::Assistant,
        "AGENT_REPLY_MARKER the answer",
    );
    a.messages.last_mut().unwrap().duration_ms = Some(2100);
    let (buf, _) = paint_full_shell(&mut a, 100, 24);
    let mut found = None;
    for y in 0..buf.area().height {
        let row = row_text(&buf, y);
        if row.contains("AGENT_REPLY_MARKER") {
            found = Some(row);
            break;
        }
    }
    let row = found.expect("agent reply should be painted");
    let marker_at = row.find("AGENT_REPLY_MARKER").expect("marker");
    let clock_at = row.rfind(':').expect("HH:MM clock on the agent reply");
    assert!(
        clock_at > marker_at,
        "clock must sit to the right of the reply, got {row:?}"
    );
}

#[test]
fn fenced_rust_keeps_token_colours_on_the_shell() {
    let mut a = app();
    a.add_message(crate::app::ChatRole::User, "show code");
    a.add_message(
        crate::app::ChatRole::Assistant,
        "```rust\nfn main() { let x = \"hi\"; }\n```",
    );
    let (buf, _) = paint_full_shell(&mut a, 100, 24);
    let mut fgs = std::collections::BTreeSet::new();
    for y in 0..buf.area().height {
        let row = row_text(&buf, y);
        if !row.contains("fn") && !row.contains("let") && !row.contains("hi") {
            continue;
        }
        for x in 0..buf.area().width {
            if let Some(cell) = buf.cell((x, y)) {
                let sym = cell.symbol();
                if matches!(sym, "f" | "n" | "l" | "e" | "t" | "h" | "i" | "\"")
                    && let Some(Color::Rgb(r, g, b)) = cell.style().fg
                {
                    fgs.insert((r, g, b));
                }
            }
        }
    }
    assert!(
        fgs.len() >= 2,
        "painted rust fence must keep token colours, got {fgs:?}"
    );
}

#[test]
fn session_leaves_gap_between_chat_and_stop() {
    let mut a = session_with_overflow();
    a.current_agent_state = AgentState::Generating;
    let (buf, _) = paint_full_shell(&mut a, 100, 24);

    let chat = a.chat_area.expect("session publishes a chat hit rect");
    let stop = a.turn_stop_hit.rect.expect("busy turn must publish [stop]");
    assert!(
        stop.y
            >= chat
                .y
                .saturating_add(chat.height)
                .saturating_add(layout::CHAT_GAP),
        "chat.bottom={} stop.y={} CHAT_GAP={}",
        chat.y.saturating_add(chat.height),
        stop.y,
        layout::CHAT_GAP
    );
    // The reserved gap row is empty of transcript glyphs.
    let gap_y = chat.y.saturating_add(chat.height);
    let gap = row_text(&buf, gap_y);
    assert!(
        !gap.contains('\u{276F}') && !gap.contains("[stop]"),
        "safezone row must be empty of chat/stop: {gap:?}"
    );
}

#[test]
fn prompt_gap_clears_stained_glyphs() {
    // Bracketed paste echo (or a previously taller box) dumps text into
    // the breathing-room rows above ╭─╮. Those rows must be rewritten.
    let mut a = session_with_overflow();
    a.current_agent_state = AgentState::Generating;
    a.input_buffer.clear();
    a.input_cursor = 0;
    let (buf, _) = paint(100, 24, |f| {
        let stain: Vec<Line> = (0..24).map(|_| Line::from("Z".repeat(100))).collect();
        f.render_widget(Paragraph::new(Text::from(stain)), f.area());
        crate::ui::render(f, &mut a);
    });
    let mut top_y = None;
    for y in 0..24u16 {
        for x in 0..100u16 {
            if buf[(x, y)].symbol() == "╭" {
                top_y = Some(y);
                break;
            }
        }
    }
    let top_y = top_y.expect("prompt top border ╭");
    assert!(
        top_y >= 2,
        "expected OUTER_TOP_GAP rows above the box, top_y={top_y}"
    );
    for dy in 1..=2u16 {
        let y = top_y - dy;
        let row = row_text(&buf, y);
        assert!(
            !row.contains('Z'),
            "gap row {y} still has paste stain: {row:?}"
        );
    }
}

#[test]
fn session_chat_has_side_margin() {
    let mut a = session_with_overflow();
    let (_buf, _) = paint_full_shell(&mut a, 100, 24);
    let chat = a.chat_area.expect("session publishes a chat hit rect");
    let expected_x = layout::SAFE_LEFT + layout::side_pad(100);
    assert_eq!(
        chat.x, expected_x,
        "bubble column must sit SIDE_PAD inside the safe area"
    );
}

#[test]
fn narrow_session_uses_tighter_side_margin() {
    let mut a = session_with_overflow();
    let (_buf, _) = paint_full_shell(&mut a, 40, 24);
    let chat = a.chat_area.expect("session publishes a chat hit rect");
    let expected_x = layout::SAFE_LEFT + layout::SIDE_PAD_NARROW;
    assert_eq!(
        chat.x, expected_x,
        "portrait width must not spend 8 columns on SIDE_PAD"
    );
}

#[test]
fn split_pane_hides_sidebar_so_chat_keeps_the_width() {
    let mut a = session_with_overflow();
    a.sidebar.visible = true;
    // Half of a typical 80-col PTY — old gate (`>= 36`) still drew a 24-col rail.
    let (_buf, _) = paint_full_shell(&mut a, 40, 24);
    let chat = a.chat_area.expect("session publishes a chat hit rect");
    assert!(
        chat.width >= 30,
        "half-pane chat must not shrink for a sidebar: width={}",
        chat.width
    );
}

#[test]
fn overflow_session_pins_a_user_prompt_at_the_chat_top() {
    let mut a = session_with_overflow();
    let (buf, _) = paint_full_shell(&mut a, 100, 24);
    let chat = a.chat_area.expect("session publishes a chat hit rect");
    let mut top = String::new();
    for dy in 0..3u16 {
        top.push_str(&row_text(&buf, chat.y + dy));
    }
    assert!(
        top.contains('\u{276F}'),
        "sticky header must be a user ❯ band: {top:?}"
    );
    // Absolute clock on the pinned prompt (same as a live user bubble).
    assert!(
        top.contains(':'),
        "sticky header must keep the clock: {top:?}"
    );
}

fn first_non_space_x(buf: &ratatui::buffer::Buffer, y: u16) -> Option<u16> {
    let area = buf.area();
    (area.x..area.x.saturating_add(area.width)).find(|&x| {
        buf.cell((x, y))
            .is_some_and(|c| c.symbol() != " " && !c.symbol().is_empty())
    })
}

#[test]
fn sticky_todo_panel_aligns_with_chat_and_leaves_gaps() {
    let mut a = session_with_overflow();
    a.replace_todos(vec![whycodes_core::TodoItem::new(
        "a",
        "ALIGN_TODO_MARKER pending work",
        whycodes_core::TodoStatus::Pending,
    )]);
    let (buf, _text) = paint_full_shell(&mut a, 100, 24);

    let header = a.todos_hit.rect.expect("todo header hit");
    let chat = a.chat_area.expect("chat hit");
    let bar = a.chat_scrollbar_hit.expect("overflowing chat paints a bar");

    let header_row = row_text(&buf, header.y);
    assert!(
        header_row.contains("Todos"),
        "todo header missing: {header_row:?}"
    );
    let todo_x = first_non_space_x(&buf, header.y).expect("todo header text");
    assert_eq!(
        todo_x, chat.x,
        "todo header must start on the chat body column (todo_x={todo_x} chat.x={})",
        chat.x
    );

    assert!(
        chat.y >= header.y + 2 + layout::PANEL_GAP,
        "chat.y={} must sit below todo panel + PANEL_GAP (header.y={})",
        chat.y,
        header.y
    );
    let gap_y = chat.y.saturating_sub(1);
    let gap = row_text(&buf, gap_y);
    assert!(
        !gap.contains("Todos") && !gap.contains("ALIGN_TODO_MARKER") && !gap.contains('❯'),
        "blank row between todos and transcript: {gap:?}"
    );
    assert!(
        gap.chars().all(|c| c == ' ' || c == '\n'),
        "panel gap row must be empty: {gap:?}"
    );

    assert_eq!(
        bar.x + bar.width,
        chat.x + chat.width,
        "bar on last chat col"
    );
    let gap_col = bar.x.saturating_sub(1);
    for y in chat.y..chat.y.saturating_add(chat.height) {
        let cell = buf.cell((gap_col, y)).expect("gap col");
        assert_eq!(
            cell.symbol(),
            " ",
            "column left of scrollbar must be blank at y={y}"
        );
    }
}

#[test]
fn sticky_tasks_panel_aligns_with_chat_and_leaves_gaps() {
    let mut a = session_with_overflow();
    a.upsert_subagent(crate::app::SubagentUpdate {
        id: "task-1".into(),
        kind: "explore".into(),
        description: "ALIGN_TASK_MARKER scan".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    });
    let (buf, _text) = paint_full_shell(&mut a, 100, 24);

    let header = a.tasks_hit.rect.expect("tasks header hit");
    let chat = a.chat_area.expect("chat hit");

    let header_row = row_text(&buf, header.y);
    assert!(
        header_row.contains("Tasks"),
        "tasks header missing: {header_row:?}"
    );
    let task_x = first_non_space_x(&buf, header.y).expect("tasks header text");
    assert_eq!(
        task_x, chat.x,
        "tasks header must start on the chat body column (task_x={task_x} chat.x={})",
        chat.x
    );

    assert!(
        chat.y >= header.y + 2 + layout::PANEL_GAP,
        "chat.y={} must sit below tasks panel + PANEL_GAP (header.y={})",
        chat.y,
        header.y
    );
    let gap_y = chat.y.saturating_sub(1);
    let gap = row_text(&buf, gap_y);
    assert!(
        !gap.contains("Tasks") && !gap.contains("ALIGN_TASK_MARKER") && !gap.contains('❯'),
        "blank row between tasks and transcript: {gap:?}"
    );
    assert!(
        gap.chars().all(|c| c == ' ' || c == '\n'),
        "panel gap row must be empty: {gap:?}"
    );
}

#[test]
fn tiny_body_drops_panel_gap_to_keep_the_transcript() {
    let mut a = session_with_overflow();
    a.upsert_subagent(crate::app::SubagentUpdate {
        id: "task-1".into(),
        kind: "explore".into(),
        description: "scan".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    });
    // 100x10 → 4 body rows after safe insets, header/footer, and TOP_PAD:
    // tasks header(1) + PANEL_GAP would leave the transcript under its
    // 3-row minimum, so the gap row must be dropped.
    let (_buf, _) = paint_full_shell(&mut a, 100, 10);
    let header = a.tasks_hit.rect.expect("tasks header painted");
    let chat = a.chat_area.expect("chat hit");
    assert_eq!(
        chat.y,
        header.y + 1,
        "tiny body must not spend a row on PANEL_GAP (header.y={} chat.y={})",
        header.y,
        chat.y
    );
}
