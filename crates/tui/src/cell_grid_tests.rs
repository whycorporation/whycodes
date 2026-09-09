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
