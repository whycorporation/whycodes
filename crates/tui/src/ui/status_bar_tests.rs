use super::*;
use ratatui::layout::Rect;
use ratatui::style::Color;

#[test]
fn total_width_one_and_two_items() {
    let mut bar = StatusBar::new(Style::default());
    bar.push("a", Line::from("AA"));
    assert_eq!(bar.total_width(), 2);

    let mut bar = StatusBar::new(Style::default());
    bar.push("a", Line::from("AA"));
    bar.push("b", Line::from("BBB"));
    assert_eq!(bar.total_width(), 8);
}

#[test]
fn render_places_context_on_right() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(20, 1);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        let mut bar = StatusBar::new(Style::default().fg(Color::DarkGray));
        bar.push("context", Line::from("1.2k / 200k"));
        let areas = bar.render(f, f.area());
        let r = areas.get("context").expect("context hit");
        assert_eq!(r.width, 11);
        assert_eq!(r.x + r.width, 20);
    })
    .unwrap();
}

#[test]
fn empty_bar_is_a_no_op() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let bar = StatusBar::new(Style::default());
    assert_eq!(bar.total_width(), 0);
    let backend = TestBackend::new(10, 1);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        let areas = StatusBar::new(Style::default()).render(f, f.area());
        assert!(areas.is_empty());
        let areas = StatusBar::new(Style::default()).render(f, Rect::new(0, 0, 0, 1));
        assert!(areas.is_empty());
    })
    .unwrap();
}
