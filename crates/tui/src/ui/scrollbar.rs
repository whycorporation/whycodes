//! Shared solid-fill scrollbar for list popups and dropdowns.
//!
//! Grok-style: track = solid dark cells, thumb = solid mid-gray (█ with matching
//! bg so the bar fills the cell box — bare fg █ leaves line-gap stripes).

use crate::theme::ThemePalette;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

/// Columns reserved for the scrollbar track when content overflows.
pub const SCROLLBAR_GUTTER: u16 = 1;

/// Track / thumb pair derived from the active palette.
#[derive(Clone, Copy)]
pub struct ScrollbarColors {
    pub track: Color,
    pub thumb: Color,
}

impl ScrollbarColors {
    pub fn from_palette(p: &ThemePalette) -> Self {
        let track = elevate(p.bg, 8);
        let thumb = {
            // Prefer palette.scrollbar, then dim, then a lifted mid-gray.
            if contrast_ok(track, p.scrollbar) {
                p.scrollbar
            } else if contrast_ok(track, p.dim) {
                p.dim
            } else {
                elevate(p.bg, 72)
            }
        };
        Self { track, thumb }
    }
}

/// Classic proportional scrollbar.
///
/// `total` = full item/line count, `visible` = viewport capacity, `offset` =
/// first visible index. No-op when content fits.
pub fn paint_scrollbar(
    buf: &mut Buffer,
    area: Rect,
    total: usize,
    visible: usize,
    offset: usize,
    track: Color,
    thumb: Color,
) {
    if area.width == 0 || area.height == 0 || total <= visible {
        return;
    }
    let h = area.height as usize;
    // Thumb spans at least 1 cell; scales with viewport/content ratio.
    let thumb_len = ((visible * h).div_ceil(total)).max(1).min(h);
    let max_off = total - visible;
    let thumb_pos = if max_off == 0 || h == thumb_len {
        0
    } else {
        (offset * (h - thumb_len) + max_off / 2) / max_off
    };

    let track_style = Style::default().fg(track).bg(track);
    let thumb_style = Style::default().fg(thumb).bg(thumb);
    for row in 0..area.height {
        let y = area.y + row;
        let r = row as usize;
        let on_thumb = r >= thumb_pos && r < thumb_pos + thumb_len;
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol("█");
            cell.set_style(if on_thumb { thumb_style } else { track_style });
        }
    }
}

/// Paint a scrollbar on the right edge of `area` when `total > visible`.
///
/// Returns the content rect (full width if no bar, else width − gutter).
pub fn content_with_scrollbar(
    buf: &mut Buffer,
    area: Rect,
    total: usize,
    visible: usize,
    offset: usize,
    colors: ScrollbarColors,
) -> Rect {
    if area.width == 0 || area.height == 0 || total <= visible {
        return area;
    }
    let content = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(SCROLLBAR_GUTTER),
        height: area.height,
    };
    let sb = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y,
        width: 1,
        height: area.height,
    };
    paint_scrollbar(buf, sb, total, visible, offset, colors.track, colors.thumb);
    content
}

/// Keep `selected` on screen for a window of `visible` rows.
pub fn scroll_to_selected(selected: usize, total: usize, visible: usize) -> usize {
    if total == 0 || visible == 0 {
        return 0;
    }
    let rows = visible.min(total);
    selected
        .saturating_sub(rows.saturating_sub(1))
        .min(total.saturating_sub(rows))
}

/// Thumb length, max scroll offset, and travel distance for a track of `height`.
///
/// Returns `None` when a scrollbar should not be shown (`total <= visible`).
pub fn scrollbar_metrics(
    total: usize,
    visible: usize,
    height: usize,
) -> Option<(
    usize, /* thumb_len */
    usize, /* max_off */
    usize, /* travel */
)> {
    if total <= visible || height == 0 || visible == 0 {
        return None;
    }
    let thumb_len = ((visible * height).div_ceil(total)).max(1).min(height);
    let max_off = total - visible;
    let travel = height.saturating_sub(thumb_len);
    Some((thumb_len, max_off, travel))
}

/// Pixel-row of the thumb top for a given content `offset` (matches [`paint_scrollbar`]).
pub fn thumb_top_for_offset(offset: usize, max_off: usize, travel: usize) -> usize {
    if max_off == 0 || travel == 0 {
        0
    } else {
        (offset * travel + max_off / 2) / max_off
    }
}

/// Content offset for a thumb whose top is at `thumb_top` track rows.
pub fn offset_for_thumb_top(thumb_top: usize, max_off: usize, travel: usize) -> usize {
    if max_off == 0 || travel == 0 {
        0
    } else {
        (thumb_top.min(travel) * max_off + travel / 2) / travel
    }
}

/// Map a pointer row on the scrollbar track to a content scroll offset.
///
/// `grab_in_thumb` is the row within the thumb where the user grabbed (0 = top
/// of thumb). When `None`, the click is treated as positioning the thumb center
/// under the pointer (track click).
pub fn offset_from_pointer_y(
    y: u16,
    track: Rect,
    total: usize,
    visible: usize,
    grab_in_thumb: Option<u16>,
) -> usize {
    let height = track.height as usize;
    let Some((thumb_len, max_off, travel)) = scrollbar_metrics(total, visible, height) else {
        return 0;
    };
    if y < track.y || y >= track.y.saturating_add(track.height) {
        // Clamp to ends when the pointer leaves the track while dragging.
        if y < track.y {
            return 0;
        }
        return max_off;
    }
    let rel = (y - track.y) as usize;
    let thumb_top = match grab_in_thumb {
        Some(grab) => rel.saturating_sub(grab as usize),
        None => rel.saturating_sub(thumb_len / 2),
    };
    offset_for_thumb_top(thumb_top, max_off, travel)
}

/// Selection index that makes [`scroll_to_selected`] yield exactly `offset`.
///
/// Picking the last visible row of the window pins the viewport top to `offset`.
pub fn selection_for_offset(offset: usize, total: usize, visible: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let vis = visible.min(total).max(1);
    let max_off = total.saturating_sub(vis);
    let offset = offset.min(max_off);
    offset
        .saturating_add(vis.saturating_sub(1))
        .min(total.saturating_sub(1))
}

/// Whether `(col, row)` is inside the scrollbar track.
pub fn scrollbar_contains(track: Rect, col: u16, row: u16) -> bool {
    col >= track.x
        && col < track.x.saturating_add(track.width.max(1))
        && row >= track.y
        && row < track.y.saturating_add(track.height)
}

/// Center-ish scroll used by slash suggest (selected near vertical middle).
pub fn scroll_center(selected: usize, total: usize, visible: usize) -> usize {
    if total <= visible || selected < visible / 2 {
        0
    } else if selected + visible / 2 >= total {
        total.saturating_sub(visible)
    } else {
        selected.saturating_sub(visible / 2)
    }
}

pub(crate) fn elevate(c: Color, delta: u8) -> Color {
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
    fn paint_scrollbar_marks_thumb_cells() {
        let area = Rect::new(0, 0, 10, 6);
        let mut buf = Buffer::empty(area);
        let sb = Rect::new(9, 0, 1, 6);
        paint_scrollbar(
            &mut buf,
            sb,
            15,
            6,
            0,
            Color::Rgb(20, 20, 20),
            Color::Rgb(90, 90, 90),
        );
        let thumb_bg = Color::Rgb(90, 90, 90);
        let track_bg = Color::Rgb(20, 20, 20);
        let top = buf.cell((9, 0)).expect("top cell");
        assert_eq!(top.bg, thumb_bg, "top cell should be thumb at offset 0");
        let bottom = buf.cell((9, 5)).expect("bottom cell");
        assert_eq!(bottom.bg, track_bg, "bottom cell should be track");
    }

    #[test]
    fn scroll_to_selected_keeps_cursor_visible() {
        assert_eq!(scroll_to_selected(0, 20, 5), 0);
        assert_eq!(scroll_to_selected(4, 20, 5), 0);
        assert_eq!(scroll_to_selected(5, 20, 5), 1);
        assert_eq!(scroll_to_selected(19, 20, 5), 15);
        assert_eq!(scroll_to_selected(0, 3, 5), 0);
    }

    #[test]
    fn scroll_center_centers_selection() {
        assert_eq!(scroll_center(0, 20, 6), 0);
        assert_eq!(scroll_center(2, 20, 6), 0);
        assert_eq!(scroll_center(10, 20, 6), 7);
        assert_eq!(scroll_center(19, 20, 6), 14);
        assert_eq!(scroll_center(0, 4, 6), 0);
    }

    #[test]
    fn selection_for_offset_pins_viewport_top() {
        // visible=5, offset=3 → selected at last visible row so scroll_to_selected == 3
        let sel = selection_for_offset(3, 20, 5);
        assert_eq!(scroll_to_selected(sel, 20, 5), 3);
        let sel0 = selection_for_offset(0, 20, 5);
        assert_eq!(scroll_to_selected(sel0, 20, 5), 0);
    }

    #[test]
    fn offset_from_pointer_roundtrips_thumb_ends() {
        let track = Rect::new(10, 0, 1, 10);
        // Top of track → offset 0
        assert_eq!(offset_from_pointer_y(0, track, 30, 10, Some(0)), 0);
        // Bottom of track with grab at thumb bottom-ish
        let max_off = 20usize;
        let bottom = offset_from_pointer_y(9, track, 30, 10, Some(0));
        assert!(bottom <= max_off);
        assert!(bottom >= max_off.saturating_sub(2), "near end: {bottom}");
    }

    #[test]
    fn thumb_is_flush_with_track_bottom_when_offset_is_max() {
        // Chat "at bottom" uses top-origin view_start = max_off. Thumb must
        // sit on the last cells of the track — not ~70% like ratatui::Scrollbar.
        let area = Rect::new(0, 0, 1, 20);
        let mut buf = Buffer::empty(area);
        let total = 100usize;
        let visible = 20usize;
        let max_off = total - visible;
        let thumb = Color::Rgb(200, 200, 200);
        let track = Color::Rgb(40, 40, 40);
        paint_scrollbar(&mut buf, area, total, visible, max_off, track, thumb);

        let (thumb_len, _, _) = scrollbar_metrics(total, visible, 20).unwrap();
        // Last cell of track must be thumb
        assert_eq!(
            buf.cell((0, 19)).map(|c| c.bg),
            Some(thumb),
            "bottom cell should be thumb at max offset"
        );
        // Cell just above thumb block should be track (if thumb doesn't fill all)
        if thumb_len < 20 {
            let above = 19u16 - thumb_len as u16;
            assert_eq!(
                buf.cell((0, above)).map(|c| c.bg),
                Some(track),
                "cell above thumb should be track"
            );
        }
        // Top cell should be track
        assert_eq!(buf.cell((0, 0)).map(|c| c.bg), Some(track));
    }

    #[test]
    fn thumb_is_flush_with_track_top_when_offset_is_zero() {
        let area = Rect::new(0, 0, 1, 20);
        let mut buf = Buffer::empty(area);
        paint_scrollbar(
            &mut buf,
            area,
            100,
            20,
            0,
            Color::Rgb(40, 40, 40),
            Color::Rgb(200, 200, 200),
        );
        assert_eq!(
            buf.cell((0, 0)).map(|c| c.bg),
            Some(Color::Rgb(200, 200, 200)),
            "top cell should be thumb at offset 0"
        );
    }
}
