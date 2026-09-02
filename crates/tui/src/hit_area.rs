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
    #[test]
    fn hit_area_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
