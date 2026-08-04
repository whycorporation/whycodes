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
struct DropdownColors {
    /// Panel + normal row background.
    panel: Color,
    /// Selected row wash.
    selected: Color,
    /// Top/bottom hairline + count label background.
    chrome: Color,
    /// Hairline / count foreground.
    chrome_fg: Color,
    /// Scrollbar track cell background.
    track: Color,
    /// Scrollbar thumb solid fill (painted as bg=fg full block).
    thumb: Color,
    name: Color,
    name_selected: Color,
    hint: Color,
}

impl DropdownColors {
    fn from_palette(p: &ThemePalette) -> Self {
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

pub fn render(frame: &mut Frame, prompt_area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let suggest = &app.slash_suggest;
    if !suggest.active || suggest.matches.is_empty() {
        return;
    }

    let colors = DropdownColors::from_palette(palette);
    let total = suggest.matches.len();
    let rows = (total as u16).min(MAX_ROWS);
    // hairline + items + hairline
    let height = rows + 2;
    if prompt_area.y < height || prompt_area.width < 8 {
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
    let start = scroll_center(suggest.selected, total, visible);

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
    let content_w = if needs_scrollbar {
        items_area.width.saturating_sub(SCROLLBAR_GUTTER)
    } else {
        items_area.width
    };

    for vis_row in 0..visible {
        let item_idx = start + vis_row;
        if item_idx >= total {
            break;
        }
        let cmd_idx = suggest.matches[item_idx];
        let cmd = &BUILTIN_SLASH_COMMANDS[cmd_idx];
        let selected = item_idx == suggest.selected;
        let y = items_area.y + vis_row as u16;
        let row_bg = if selected {
            colors.selected
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

fn fill_bg(buf: &mut Buffer, area: Rect, bg: Color) {
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

fn set_line(buf: &mut Buffer, x: u16, y: u16, text: &str, width: u16, style: Style) {
    let line = Line::from(Span::styled(text.to_string(), style));
    let _ = buf.set_line(x, y, &line, width);
}

fn truncate_to(s: &str, max: usize) -> String {
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
fn elevate(c: Color, delta: u8) -> Color {
    let (r, g, b) = to_rgb(c);
    Color::Rgb(
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

    #[test]
    fn truncate_to_short_and_long() {
        assert_eq!(truncate_to("hello", 10), "hello");
        assert_eq!(truncate_to("hello world", 8), "hello w…");
        assert_eq!(truncate_to("ab", 1), "…");
        assert_eq!(truncate_to("ab", 0), "");
    }

    #[test]
    fn elevate_brightens_dark_bg() {
        let c = elevate(Color::Rgb(20, 20, 20), 26);
        assert_eq!(c, Color::Rgb(46, 46, 46));
    }
}
