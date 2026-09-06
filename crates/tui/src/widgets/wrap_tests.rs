use super::*;
use ratatui::style::{Color, Modifier};

#[test]
fn wrap_spans_preserves_style_across_rows() {
    let bold = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let spans = vec![
        Span::styled("hello ".to_string(), bold),
        Span::styled("world and friends".to_string(), Style::default()),
    ];
    let lines = wrap_spans(spans, 10);
    assert!(lines.len() >= 2, "expected soft-wrap, got {}", lines.len());
    // First row should still carry the bold "hello" style.
    let first_has_bold = lines[0]
        .spans
        .iter()
        .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
    assert!(first_has_bold);
}

#[test]
fn wrap_plain_breaks_long_words() {
    let lines = wrap_plain("abcdefghij", 4, Style::default());
    assert!(lines.len() >= 2);
}
