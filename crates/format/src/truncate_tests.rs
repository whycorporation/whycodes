use super::*;

#[test]
fn test_no_truncation() {
    let text = "line1\nline2\nline3";
    let result = truncate(text, 10, 1000);
    assert_eq!(result, text);
}

#[test]
fn test_line_truncation() {
    let text = "a\nb\nc\nd\ne";
    let result = truncate(text, 3, 1000);
    assert!(result.starts_with("a\nb\nc\n"));
    assert!(result.contains("[... 2 lines truncated]"));
}

#[test]
fn test_char_truncation() {
    let text = "aaaaa\nbbbbb\nccccc";
    let result = truncate(text, 100, 12);
    // 12 chars: "aaaaa\n" = 6, "bbbbb\n" = 6 → total 12, fits exactly
    // "ccccc\n" = 6 → would be 18, exceeds
    assert!(result.starts_with("aaaaa\nbbbbb\n"));
    assert!(result.contains("truncated"));
}

#[test]
fn test_single_line_text() {
    let text = "just one line no newlines";
    let result = truncate(text, 100, 5);
    // Single line, 0 lines kept, truncated
    assert!(result.contains("truncated"));
}

#[test]
fn singular_line_truncated_message() {
    let result = truncate("a\nb", 1, 1000);
    assert!(result.contains("[... 1 line truncated]"), "{result}");
    assert!(!result.contains("lines"), "{result}");
}

#[test]
fn later_line_exceeds_char_budget() {
    let result = truncate("aa\nbb\ncc", 10, 7);
    assert!(result.contains("truncated"), "{result}");
    assert!(result.starts_with("aa\n"), "{result}");
}
