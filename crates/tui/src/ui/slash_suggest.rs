// ── ui/slash_suggest.rs: Command completion above the prompt ───────────────
// Opens on `/` at an empty prompt. Layout mirrors Grok's slash_dropdown:
//   • panel wash (bg_light) with top/bottom hairlines only
//   • selected row wash (bg_visual)
//   • solid-fill scrollbar track/thumb when the list overflows

use crate::app::{BUILTIN_SLASH_COMMANDS, TuiApp};
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{paint_scrollbar, scroll_center};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Clear,
};

/// Visible rows (excluding hairlines). Matches Grok's MAX_VISIBLE_SUGGESTIONS.
const MAX_ROWS: u16 = 6;

/// Columns reserved for gap + scrollbar track when content overflows.
const SCROLLBAR_GUTTER: u16 = 2;

/// Local Grok-like surface colours derived from the active palette.
///
/// GrokNight dropdown uses:
///   bg_light   ≈ #242424  (row / panel)
///   bg_visual  ≈ #363636  (selected)
///   bg_base    ≈ #141414  (hairline bg)
///   gray_dim   ≈ #585858  (thumb solid fill)
///   bg_dark    ≈ #1c1c1c  (track)
/// We synthesise the same relative steps from `palette.bg` so every theme
/// still gets a readable track/thumb pair (the old accent-on-dialog │ bar
/// vanished into the panel on most dark themes).
pub(crate) struct DropdownColors {
    /// Panel + normal row background.
    pub(crate) panel: Color,
    /// Selected row wash.
    pub(crate) selected: Color,
    /// Top/bottom hairline + count label background.
    pub(crate) chrome: Color,
    /// Hairline / count foreground.
    pub(crate) chrome_fg: Color,
    /// Scrollbar track cell background.
    pub(crate) track: Color,
    /// Scrollbar thumb solid fill (painted as bg=fg full block).
    pub(crate) thumb: Color,
    pub(crate) name: Color,
    pub(crate) name_selected: Color,
    pub(crate) hint: Color,
}

impl DropdownColors {
    pub(crate) fn from_palette(p: &ThemePalette) -> Self {
        // Lift the canvas toward white for elevated surfaces; clamp so light
        // themes don't blow out.
        let panel = elevate(p.bg, 26);
        let selected = elevate(p.bg, 44);
        let chrome = p.bg;
        let track = elevate(p.bg, 8);
        // Thumb must read clearly against the track: use dim if it's far
        // enough from track, otherwise a mid-gray step.
        let thumb = {
            let dim = p.dim;
            if contrast_ok(track, dim) {
                dim
            } else {
                elevate(p.bg, 72)
            }
        };
        Self {
            panel,
            selected,
            chrome,
            chrome_fg: elevate(p.bg, 26), // ≈ Grok bg_highlight on hairline
            track,
            thumb,
            name: p.fg,
            name_selected: p.fg,
            hint: p.dim,
        }
    }
}

pub fn render(frame: &mut Frame, prompt_area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if !app.slash_suggest.active || app.slash_suggest.matches.is_empty() {
        app.slash_suggest.list_hit = None;
        app.slash_suggest.hovered = None;
        return;
    }

    let colors = DropdownColors::from_palette(palette);
    let total = app.slash_suggest.matches.len();
    let rows = (total as u16).min(MAX_ROWS);
    // hairline + items + hairline
    let height = rows + 2;
    if prompt_area.y < height || prompt_area.width < 8 {
        app.slash_suggest.list_hit = None;
        return;
    }
    // Sit just above the prompt, full prompt width (Grok anchors to prompt).
    let area = Rect {
        x: prompt_area.x,
        y: prompt_area.y.saturating_sub(height),
        width: prompt_area.width,
        height,
    };

    frame.render_widget(Clear, area);

    let visible = rows as usize;
    let needs_scrollbar = total > visible;
    let start = scroll_center(app.slash_suggest.selected, total, visible);
    app.slash_suggest.list_scroll_start = start;

    let buf = frame.buffer_mut();

    // ── panel wash ──────────────────────────────────────────────────────
    fill_bg(buf, area, colors.panel);

    // ── top / bottom hairlines (Grok: ─ on bg_base, count on the right) ─
    let rule = "─".repeat(area.width as usize);
    let rule_style = Style::default().fg(colors.chrome_fg).bg(colors.chrome);
    set_line(buf, area.x, area.y, &rule, area.width, rule_style);
    set_line(
        buf,
        area.x,
        area.y + height - 1,
        &rule,
        area.width,
        rule_style,
    );
    // Count hint on the top hairline, right-aligned (Grok: gray on bg_base).
    let hint = format!("{total}");
    let hint_w = hint.len() as u16;
    if hint_w + 2 <= area.width {
        let hx = area.x + area.width - hint_w - 1;
        set_line(
            buf,
            hx,
            area.y,
            &hint,
            hint_w,
            Style::default().fg(colors.hint).bg(colors.chrome),
        );
    }

    // ── item rows ───────────────────────────────────────────────────────
    let items_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: rows,
    };
    app.slash_suggest.list_hit = Some(Rect {
        x: items_area.x,
        y: items_area.y,
        width: items_area
            .width
            .saturating_sub(if needs_scrollbar { SCROLLBAR_GUTTER } else { 0 }),
        height: items_area.height,
    });
    let content_w = if needs_scrollbar {
        items_area.width.saturating_sub(SCROLLBAR_GUTTER)
    } else {
        items_area.width
    };

    // Slightly elevated wash for mouse hover (between panel and selected).
    let hover_bg = elevate(palette.bg, 36);

    for vis_row in 0..visible {
        let item_idx = start + vis_row;
        if item_idx >= total {
            break;
        }
        let cmd_idx = app.slash_suggest.matches[item_idx];
        let cmd = &BUILTIN_SLASH_COMMANDS[cmd_idx];
        let selected = item_idx == app.slash_suggest.selected;
        let mouse_hover = app.slash_suggest.hovered == Some(item_idx) && !selected;
        let y = items_area.y + vis_row as u16;
        let row_bg = if selected {
            colors.selected
        } else if mouse_hover {
            hover_bg
        } else {
            colors.panel
        };
        // Full-row wash first so selection reads as a bar even under short text.
        fill_bg(
            buf,
            Rect {
                x: items_area.x,
                y,
                width: content_w,
                height: 1,
            },
            row_bg,
        );
        paint_row(
            buf,
            items_area.x,
            y,
            content_w,
            cmd.name,
            cmd.hint,
            selected,
            row_bg,
            &colors,
        );
    }

    // ── scrollbar (solid Grok thumb; last, so nothing paints over it) ───
    if needs_scrollbar {
        let sb = Rect {
            x: items_area.x + items_area.width.saturating_sub(1),
            y: items_area.y,
            width: 1,
            height: items_area.height,
        };
        paint_scrollbar(buf, sb, total, visible, start, colors.track, colors.thumb);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    name: &str,
    hint: &str,
    selected: bool,
    row_bg: Color,
    colors: &DropdownColors,
) {
    if width == 0 {
        return;
    }
    let bold = if selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let name_style = Style::default()
        .fg(if selected {
            colors.name_selected
        } else {
            colors.name
        })
        .bg(row_bg)
        .add_modifier(bold);
    let hint_style = Style::default().fg(colors.hint).bg(row_bg);
    let pad_style = Style::default().bg(row_bg);

    let prefix = if selected { "❯ " } else { "  " };
    // Fixed name column (14) like before — keeps descriptions aligned.
    let name_col = format!("{name:<14}");
    let mut spans = vec![
        Span::styled(prefix.to_string(), name_style),
        Span::styled(name_col.clone(), name_style),
    ];
    // display width for ascii-only command names / prefix
    let mut used = 2 + name_col.len();
    if (width as usize) > used + 1 {
        spans.push(Span::styled(" ", pad_style));
        used += 1;
        let budget = (width as usize).saturating_sub(used);
        let hint_text = truncate_to(hint, budget);
        used += hint_text.chars().count();
        spans.push(Span::styled(hint_text, hint_style));
    }
    if (width as usize) > used {
        spans.push(Span::styled(" ".repeat(width as usize - used), pad_style));
    }
    let line = Line::from(spans);
    // set_line writes graphemes without restyling the whole rect.
    let _ = buf.set_line(x, y, &line, width);
}

pub(crate) fn fill_bg(buf: &mut Buffer, area: Rect, bg: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Clear is already applied; this paints a uniform wash.
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(Style::default().bg(bg));
            }
        }
    }
}

pub(crate) fn set_line(buf: &mut Buffer, x: u16, y: u16, text: &str, width: u16, style: Style) {
    let line = Line::from(Span::styled(text.to_string(), style));
    let _ = buf.set_line(x, y, &line, width);
}

pub(crate) fn truncate_to(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let keep: String = s.chars().take(max - 1).collect();
    format!("{keep}…")
}

/// Lift an sRGB colour toward white by `delta` per channel (clamped).
pub(crate) fn elevate(c: Color, delta: u8) -> Color {
    let (r, g, b) = to_rgb(c);
    crate::color::paint_rgb(
        r.saturating_add(delta),
        g.saturating_add(delta),
        b.saturating_add(delta),
    )
}

fn contrast_ok(a: Color, b: Color) -> bool {
    let (ar, ag, ab) = to_rgb(a);
    let (br, bg, bb) = to_rgb(b);
    let dr = (ar as i16 - br as i16).unsigned_abs();
    let dg = (ag as i16 - bg as i16).unsigned_abs();
    let db = (ab as i16 - bb as i16).unsigned_abs();
    // At least ~40 luminance steps on one channel — enough to read a bar.
    dr.max(dg).max(db) >= 40
}

fn to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (128, 0, 0),
        Color::Green => (0, 128, 0),
        Color::Yellow => (128, 128, 0),
        Color::Blue => (0, 0, 128),
        Color::Magenta => (128, 0, 128),
        Color::Cyan => (0, 128, 128),
        Color::Gray => (192, 192, 192),
        Color::DarkGray => (128, 128, 128),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (0, 0, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Indexed(i) => {
            // Approximate xterm gray ramp / cube for contrast checks only.
            if (232..=255).contains(&i) {
                let v = (i - 232) * 10 + 8;
                (v, v, v)
            } else if (16..=231).contains(&i) {
                let n = i - 16;
                let f = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                (f(n / 36), f((n % 36) / 6), f(n % 6))
            } else {
                (128, 128, 128)
            }
        }
        _ => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
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
    }
}
