//! Packed terminal cell grid for selection → clipboard.
//!
//! Grok-style binary layout: one UTF-8 buffer + a `u32` offset table
//! instead of `width × height` heap `String`s (a 200×60 frame used to
//! allocate ~12k strings every drag frame).

use ratatui::buffer::Buffer;

/// Screen snapshot: cell `(x, y)` is `data[off[i] .. off[i + 1]]`
/// where `i = y * width + x`.
#[derive(Debug, Clone, Default)]
pub struct CellGrid {
    width: u16,
    height: u16,
    data: String,
    /// `width * height + 1` entries; last is `data.len()`.
    off: Vec<u32>,
}

impl CellGrid {
    pub fn is_empty(&self) -> bool {
        self.height == 0 || self.width == 0
    }

    pub fn clear(&mut self) {
        self.width = 0;
        self.height = 0;
        self.data.clear();
        self.off.clear();
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    /// Symbol at column `x` of row `y`, or `""` if out of range.
    pub fn get(&self, x: u16, y: u16) -> &str {
        if x >= self.width || y >= self.height {
            return "";
        }
        let i = y as usize * self.width as usize + x as usize;
        let start = self.off[i] as usize;
        let end = self.off[i + 1] as usize;
        self.data.get(start..end).unwrap_or("")
    }

    /// Flatten a ratatui buffer into the packed grid.
    pub fn from_buffer(buf: &Buffer) -> Self {
        let a = buf.area();
        let n = a.width as usize * a.height as usize;
        let mut data = String::with_capacity(n);
        let mut off = Vec::with_capacity(n + 1);
        off.push(0);
        for y in a.y..a.y.saturating_add(a.height) {
            for x in a.x..a.x.saturating_add(a.width) {
                data.push_str(buf[(x, y)].symbol());
                off.push(data.len() as u32);
            }
        }
        Self {
            width: a.width,
            height: a.height,
            data,
            off,
        }
    }

    /// Test helper: each inner vec is one row of cell symbols.
    pub fn from_rows(rows: Vec<Vec<String>>) -> Self {
        if rows.is_empty() {
            return Self::default();
        }
        let height = rows.len() as u16;
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0) as u16;
        let n = width as usize * height as usize;
        let mut data = String::new();
        let mut off = Vec::with_capacity(n + 1);
        off.push(0);
        for row in &rows {
            for x in 0..width as usize {
                data.push_str(row.get(x).map(String::as_str).unwrap_or(" "));
                off.push(data.len() as u32);
            }
        }
        Self {
            width,
            height,
            data,
            off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn from_buffer_roundtrips_symbols() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        buf[(0, 0)].set_symbol("a");
        buf[(1, 1)].set_symbol("字");
        let g = CellGrid::from_buffer(&buf);
        assert_eq!(g.get(0, 0), "a");
        assert_eq!(g.get(1, 1), "字");
        assert_eq!(g.get(1, 0), " ");
        assert_eq!(g.get(2, 0), "");
    }
}
