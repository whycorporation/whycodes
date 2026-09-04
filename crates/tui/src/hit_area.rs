//! Sticky hit-test targets (Grok Build `HitArea`).
//!
//! Pattern:
//! - Paint **ends** by setting `rect` from the drawn geometry.
//! - Mouse handlers call [`HitArea::update_hover`] against the **previous**
//!   frame’s rect and flip the sticky `hovered` flag.
//! - Paint **reads** `hovered` — never recompute hover after clearing `rect`
//!   in the same frame (that always returned false).

use ratatui::layout::Rect;

/// Screen region + sticky hover bit for chrome (context meter, [stop], path, …).
#[derive(Debug, Clone, Copy, Default)]
pub struct HitArea {
    pub rect: Option<Rect>,
    pub hovered: bool,
}

impl HitArea {
    /// Update sticky hover from a pointer cell. Returns `true` if the flag flipped.
    pub fn update_hover(&mut self, col: u16, row: u16) -> bool {
        let now = self.contains(col, row);
        let changed = now != self.hovered;
        self.hovered = now;
        changed
    }

    /// Whether `(col, row)` is inside the last painted rect.
    pub fn contains(&self, col: u16, row: u16) -> bool {
        let Some(r) = self.rect else {
            return false;
        };
        col >= r.x
            && col < r.x.saturating_add(r.width)
            && row >= r.y
            && row < r.y.saturating_add(r.height)
    }

    /// Replace the hit rect after paint (does not touch `hovered`).
    pub fn set_rect(&mut self, rect: Option<Rect>) {
        self.rect = rect;
    }

    /// Clear rect and hover (e.g. control not painted this frame).
    pub fn clear(&mut self) {
        self.rect = None;
        self.hovered = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn contains_respects_rect_and_empty() {
        let mut h = HitArea::default();
        assert!(!h.contains(0, 0));
        h.set_rect(Some(rect(2, 3, 4, 2)));
        assert!(h.contains(2, 3));
        assert!(h.contains(5, 4));
        assert!(!h.contains(6, 3));
        assert!(!h.contains(2, 5));
        assert!(!h.contains(1, 3));
    }

    #[test]
    fn update_hover_flips_only_on_change() {
        let mut h = HitArea::default();
        h.set_rect(Some(rect(0, 0, 2, 2)));
        assert!(h.update_hover(0, 0));
        assert!(h.hovered);
        assert!(!h.update_hover(1, 1));
        assert!(h.update_hover(9, 9));
        assert!(!h.hovered);
    }

    #[test]
    fn clear_drops_rect_and_hover() {
        let mut h = HitArea {
            rect: Some(rect(0, 0, 1, 1)),
            hovered: true,
        };
        h.clear();
        assert!(h.rect.is_none());
        assert!(!h.hovered);
        assert!(!h.contains(0, 0));
    }
}
