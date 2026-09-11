use super::*;
use crate::app::{BUILTIN_SLASH_COMMANDS, TuiApp};
use crate::config::TuiAppConfig;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn palette() -> ThemePalette {
    ThemeName::DefaultDark.palette()
}

fn app_with_matches(n: usize) -> TuiApp {
    let mut app = TuiApp::new(TuiAppConfig::default());
    let n = n.min(BUILTIN_SLASH_COMMANDS.len());
    app.slash_suggest.active = true;
    app.slash_suggest.matches = (0..n).collect();
    app.slash_suggest.selected = 0;
    app
}

/// Render `f` into a fresh terminal and return the painted buffer text.
fn paint<F>(width: u16, height: u16, f: F) -> String
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
    out
}

#[test]
fn truncate_to_short_and_long() {
    assert_eq!(truncate_to("hello", 10), "hello");
    assert_eq!(truncate_to("hello world", 8), "hello w…");
    assert_eq!(truncate_to("ab", 1), "…");
    assert_eq!(truncate_to("ab", 0), "");
    assert_eq!(truncate_to("xy", 2), "xy");
    assert_eq!(truncate_to("xyz", 2), "x…");
}

#[test]
fn elevate_brightens_dark_bg() {
    let _g = crate::color::push_color_mode(crate::color::ColorMode::TrueColor);
    let c = elevate(Color::Rgb(20, 20, 20), 26);
    assert_eq!(c, Color::Rgb(46, 46, 46));
}

#[test]
fn elevate_saturates_near_white() {
    let _g = crate::color::push_color_mode(crate::color::ColorMode::TrueColor);
    assert_eq!(
        elevate(Color::Rgb(250, 250, 250), 26),
        Color::Rgb(255, 255, 255)
    );
}

#[test]
fn elevate_resolves_named_and_indexed_colors() {
    let _g = crate::color::push_color_mode(crate::color::ColorMode::TrueColor);
    // Named ANSI → fixed sRGB, then +1 per channel.
    assert_eq!(elevate(Color::Black, 1), Color::Rgb(1, 1, 1));
    assert_eq!(elevate(Color::Red, 1), Color::Rgb(129, 1, 1));
    assert_eq!(elevate(Color::Green, 1), Color::Rgb(1, 129, 1));
    assert_eq!(elevate(Color::Yellow, 1), Color::Rgb(129, 129, 1));
    assert_eq!(elevate(Color::Blue, 1), Color::Rgb(1, 1, 129));
    assert_eq!(elevate(Color::Magenta, 1), Color::Rgb(129, 1, 129));
    assert_eq!(elevate(Color::Cyan, 1), Color::Rgb(1, 129, 129));
    assert_eq!(elevate(Color::Gray, 1), Color::Rgb(193, 193, 193));
    assert_eq!(elevate(Color::DarkGray, 1), Color::Rgb(129, 129, 129));
    assert_eq!(elevate(Color::LightRed, 0), Color::Rgb(255, 0, 0));
    assert_eq!(elevate(Color::LightGreen, 0), Color::Rgb(0, 255, 0));
    assert_eq!(elevate(Color::LightYellow, 0), Color::Rgb(255, 255, 0));
    assert_eq!(elevate(Color::LightBlue, 0), Color::Rgb(0, 0, 255));
    assert_eq!(elevate(Color::LightMagenta, 0), Color::Rgb(255, 0, 255));
    assert_eq!(elevate(Color::LightCyan, 0), Color::Rgb(0, 255, 255));
    assert_eq!(elevate(Color::White, 0), Color::Rgb(255, 255, 255));
    // Indexed: gray ramp, cube, and the leftover 0–15 bucket.
    assert_eq!(elevate(Color::Indexed(232), 0), Color::Rgb(8, 8, 8));
    assert_eq!(elevate(Color::Indexed(16), 0), Color::Rgb(0, 0, 0));
    assert_eq!(elevate(Color::Indexed(21), 0), Color::Rgb(0, 0, 255)); // 16 + 5
    assert_eq!(elevate(Color::Indexed(0), 0), Color::Rgb(128, 128, 128));
    // Reset / other → (0,0,0).
    assert_eq!(elevate(Color::Reset, 2), Color::Rgb(2, 2, 2));
}

#[test]
fn dropdown_colors_use_dim_when_contrast_is_ok() {
    let p = palette();
    let colors = DropdownColors::from_palette(&p);
    // Default dark dim is far from the lifted track → thumb is palette.dim.
    assert_eq!(colors.thumb, p.dim);
    assert_eq!(colors.name, p.fg);
    assert_eq!(colors.hint, p.dim);
    assert_eq!(colors.chrome, p.bg);
}

#[test]
fn dropdown_colors_lift_thumb_when_dim_blends_into_track() {
    let mut p = palette();
    // dim ≈ track (bg+8) → contrast_ok fails → elevate(bg, 72).
    p.dim = elevate(p.bg, 8);
    let colors = DropdownColors::from_palette(&p);
    assert_eq!(colors.thumb, elevate(p.bg, 72));
}

#[test]
fn fill_bg_washes_cells_and_skips_empty() {
    let area = Rect::new(0, 0, 4, 2);
    let mut buf = Buffer::empty(area);
    fill_bg(&mut buf, Rect::new(0, 0, 0, 2), Color::Red);
    fill_bg(&mut buf, Rect::new(0, 0, 2, 0), Color::Red);
    fill_bg(&mut buf, Rect::new(1, 0, 2, 1), Color::Rgb(9, 9, 9));
    assert_eq!(buf.cell((1, 0)).unwrap().bg, Color::Rgb(9, 9, 9));
    assert_eq!(buf.cell((2, 0)).unwrap().bg, Color::Rgb(9, 9, 9));
    assert_ne!(buf.cell((0, 0)).unwrap().bg, Color::Rgb(9, 9, 9));
}

#[test]
fn set_line_writes_styled_text() {
    let area = Rect::new(0, 0, 10, 1);
    let mut buf = Buffer::empty(area);
    set_line(&mut buf, 0, 0, "hi", 10, Style::default().fg(Color::White));
    assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "h");
    assert_eq!(buf.cell((1, 0)).unwrap().symbol(), "i");
}

#[test]
fn paint_row_skips_zero_width_and_marks_selection() {
    let colors = DropdownColors::from_palette(&palette());
    let area = Rect::new(0, 0, 40, 2);
    let mut buf = Buffer::empty(area);
    paint_row(
        &mut buf,
        0,
        0,
        0,
        "/help",
        "hint",
        true,
        colors.selected,
        &colors,
    );
    assert_eq!(buf.cell((0, 0)).unwrap().symbol(), " ");

    paint_row(
        &mut buf,
        0,
        0,
        40,
        "/help",
        "Keybinding cheatsheet",
        true,
        colors.selected,
        &colors,
    );
    let row: String = (0..40)
        .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
        .collect();
    assert!(row.contains("❯"), "{row}");
    assert!(row.contains("/help"), "{row}");
    assert!(row.contains("Keybinding"), "{row}");

    paint_row(
        &mut buf,
        0,
        1,
        40,
        "/exit",
        "Quit the TUI",
        false,
        colors.panel,
        &colors,
    );
    let row: String = (0..40)
        .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
        .collect();
    assert!(!row.contains('❯'), "{row}");
    assert!(row.contains("/exit"), "{row}");
}

#[test]
fn paint_row_omits_hint_when_the_name_column_fills_the_width() {
    let colors = DropdownColors::from_palette(&palette());
    let area = Rect::new(0, 0, 16, 1);
    let mut buf = Buffer::empty(area);
    // prefix (2) + name_col (14) == 16 → no room for the hint spacer.
    paint_row(
        &mut buf,
        0,
        0,
        16,
        "/help",
        "SHOULD-NOT-APPEAR",
        false,
        colors.panel,
        &colors,
    );
    let row: String = (0..16)
        .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
        .collect();
    assert!(row.contains("/help"), "{row}");
    assert!(!row.contains("SHOULD"), "{row}");
}

#[test]
fn render_inactive_or_empty_clears_hit_and_hover() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.slash_suggest.active = false;
    app.slash_suggest.matches = vec![0];
    app.slash_suggest.list_hit = Some(Rect::new(0, 0, 1, 1));
    app.slash_suggest.hovered = Some(0);
    let p = palette();
    let _ = paint(40, 12, |f| {
        render(f, Rect::new(0, 10, 40, 2), &mut app, &p);
    });
    assert!(app.slash_suggest.list_hit.is_none());
    assert!(app.slash_suggest.hovered.is_none());

    app.slash_suggest.active = true;
    app.slash_suggest.matches.clear();
    app.slash_suggest.list_hit = Some(Rect::new(0, 0, 1, 1));
    app.slash_suggest.hovered = Some(0);
    let _ = paint(40, 12, |f| {
        render(f, Rect::new(0, 10, 40, 2), &mut app, &p);
    });
    assert!(app.slash_suggest.list_hit.is_none());
    assert!(app.slash_suggest.hovered.is_none());
}

#[test]
fn render_skips_when_prompt_leaves_no_room() {
    let mut app = app_with_matches(3);
    let p = palette();
    // height = 3 rows + 2 hairlines = 5; y=3 < 5.
    let _ = paint(40, 8, |f| {
        render(f, Rect::new(0, 3, 40, 2), &mut app, &p);
    });
    assert!(app.slash_suggest.list_hit.is_none());

    app.slash_suggest.list_hit = Some(Rect::new(0, 0, 1, 1));
    let _ = paint(40, 12, |f| {
        render(f, Rect::new(0, 10, 4, 2), &mut app, &p);
    });
    assert!(app.slash_suggest.list_hit.is_none());
}

#[test]
fn render_paints_commands_count_and_selected_marker() {
    let mut app = app_with_matches(3);
    app.slash_suggest.selected = 1;
    let p = palette();
    let text = paint(60, 16, |f| {
        render(f, Rect::new(0, 12, 60, 2), &mut app, &p);
    });
    assert!(text.contains("/exit"), "{text}");
    assert!(text.contains("/help"), "{text}");
    assert!(text.contains("/new"), "{text}");
    assert!(text.contains('❯'), "selected marker: {text}");
    assert!(text.contains('3'), "count on hairline: {text}");
    assert!(app.slash_suggest.list_hit.is_some());
    let hit = app.slash_suggest.list_hit.unwrap();
    // 3 items fit in MAX_ROWS → no scrollbar gutter reserved.
    assert_eq!(hit.width, 60);
    assert_eq!(hit.height, 3);
}

#[test]
fn render_overflow_reserves_scrollbar_and_honours_hover() {
    let n = BUILTIN_SLASH_COMMANDS.len();
    assert!(n > MAX_ROWS as usize, "need overflow to paint a scrollbar");
    let mut app = app_with_matches(n);
    app.slash_suggest.selected = 0;
    app.slash_suggest.hovered = Some(1);
    let p = palette();
    let text = paint(70, 20, |f| {
        render(f, Rect::new(0, 16, 70, 2), &mut app, &p);
    });
    assert!(text.contains("/exit"), "{text}");
    assert!(text.contains(&n.to_string()), "count: {text}");
    let hit = app.slash_suggest.list_hit.expect("list hit");
    assert_eq!(hit.width, 70 - SCROLLBAR_GUTTER);
    assert_eq!(hit.height, MAX_ROWS);
    assert_eq!(app.slash_suggest.list_scroll_start, 0);

    app.slash_suggest.selected = n.saturating_sub(1);
    let _ = paint(70, 20, |f| {
        render(f, Rect::new(0, 16, 70, 2), &mut app, &p);
    });
    assert!(app.slash_suggest.list_scroll_start > 0);
}
