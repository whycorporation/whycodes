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
    assert!(!g.is_empty());
    let mut g = g;
    g.clear();
    assert!(g.is_empty());
    assert_eq!(g.width(), 0);
    assert_eq!(g.height(), 0);
    let empty = CellGrid::from_rows(Vec::new());
    assert!(empty.is_empty());
}

#[test]
fn from_buffer_masks_invisible_fill_cells_as_pad() {
    use ratatui::style::Color;
    let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
    buf[(0, 0)].set_symbol("a");
    // Scrollbar-style invisible fill: solid glyph painted fg == bg.
    buf[(2, 0)].set_symbol("█");
    buf[(2, 0)].fg = Color::Rgb(40, 40, 40);
    buf[(2, 0)].bg = Color::Rgb(40, 40, 40);
    let g = CellGrid::from_buffer(&buf);
    assert_eq!(g.get(2, 0), " ", "fg==bg fill must snapshot as pad");

    // A visible glyph (fg != bg) survives, as does default Reset/Reset.
    buf[(1, 0)].set_symbol("█");
    buf[(1, 0)].fg = Color::Rgb(200, 200, 200);
    buf[(1, 0)].bg = Color::Rgb(40, 40, 40);
    let g = CellGrid::from_buffer(&buf);
    assert_eq!(g.get(0, 0), "a");
    assert_eq!(g.get(1, 0), "█");
}
