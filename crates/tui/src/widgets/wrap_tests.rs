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

#[test]
fn wrap_spans_and_plain_empty_and_newline_edges() {
    let zero = wrap_spans(vec![Span::raw("keep")], 0);
    assert_eq!(zero.len(), 1);
    let empty = wrap_spans(Vec::new(), 10);
    assert_eq!(empty.len(), 1);
    let nl = wrap_plain("hi\n", 10, Style::default());
    assert!(nl.len() >= 2);
    let empty_plain = wrap_plain("", 8, Style::default());
    assert_eq!(empty_plain.len(), 1);
    let only_nl = wrap_plain("\n", 8, Style::default());
    assert!(!only_nl.is_empty());
}
