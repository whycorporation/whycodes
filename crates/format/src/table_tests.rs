use super::*;

#[test]
fn test_simple_table() {
    let headers = &["Name", "Age", "City"];
    let rows = &[
        vec!["Alice".to_string(), "30".to_string(), "NYC".to_string()],
        vec!["Bob".to_string(), "25".to_string(), "LA".to_string()],
    ];
    let result = format_table(headers, rows);
    assert!(result.contains("Alice"), "{result}");
    assert!(result.contains("Bob"), "{result}");
    assert!(result.contains('┌'), "{result}");
    assert!(result.contains('│'), "{result}");
}

#[test]
fn test_empty_rows() {
    let headers = &["Col"];
    let result = format_table(headers, &[]);
    assert!(result.contains("Col"), "{result}");
    assert!(result.contains('┌'), "{result}");
}

#[test]
fn test_empty_headers() {
    let result = format_table(&[], &[]);
    assert_eq!(result, "");
}

#[test]
fn right_align_pads_left() {
    let rows = &[vec!["42".to_string()]];
    let aligns = [TableAlign::Right];
    // Longer header forces pad on the short right-aligned cell.
    let headers = &["Num"];
    let result = format_table_aligned(headers, rows, &aligns);
    assert!(result.contains(" 42"), "{result}");
}

#[test]
fn column_widths_respect_cap() {
    let headers = vec!["abcdefghij".into(), "xyz".into()];
    let rows = vec![vec!["1".into(), "2".into()]];
    let w = column_widths(&headers, &rows, Some(20));
    let chrome = 1 + 3 * 2;
    assert!(w.iter().sum::<usize>() + chrome <= 20 + 2); // small slack
    assert_eq!(w.len(), 2);
}

#[test]
fn column_widths_empty_and_shrink_to_floor() {
    assert!(column_widths(&[], &[], Some(10)).is_empty());
    let headers = vec!["abcdefghij".into(), "xyz".into()];
    let w = column_widths(&headers, &[], Some(8));
    assert!(w.iter().all(|n| *n >= 1));
    let uncapped = column_widths(&headers, &[], None);
    assert_eq!(uncapped.len(), 2);
}

#[test]
fn display_width_treats_cjk_as_double() {
    assert_eq!(display_width("a"), 1);
    assert_eq!(display_width("中"), 2);
    assert_eq!(display_width("🙂"), 2);
}

#[test]
fn pad_cell_center_and_truncate() {
    let c = pad_cell("x", 5, TableAlign::Center);
    assert_eq!(c.len(), 5);
    assert!(c.contains('x'));
    let t = pad_cell("toolong", 3, TableAlign::Left);
    assert!(t.contains('…'), "{t}");
    assert_eq!(truncate_to_width("abc", 0), "");
    assert_eq!(truncate_to_width("ab", 10), "ab");
    assert_eq!(truncate_to_width("ab", 1), "…");
    let wide = truncate_to_width("中中中", 3);
    assert!(wide.contains('…'), "{wide}");
    let padded = truncate_to_width("中中中", 4);
    assert!(padded.contains('…'), "{padded}");
    assert!(display_width(&padded) >= 3, "{padded}");
}
