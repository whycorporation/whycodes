use super::*;

#[test]
fn test_simple_diff() {
    let old = "line1\nline2\nline3\n";
    let new = "line1\nline2_modified\nline3\nline4\n";
    let result = render_diff(old, new);
    assert!(result.contains("\x1b[31m- line2\x1b[0m"));
    assert!(result.contains("\x1b[32m+ line2_modified\x1b[0m"));
    assert!(result.contains("\x1b[32m+ line4\x1b[0m"));
}

#[test]
fn test_diff_unified() {
    let diff = "@@ -1,3 +1,4 @@\n context\n-old\n+new\n more";
    let result = render_diff_unified(diff);
    assert!(result.contains("\x1b[36m@@ -1,3 +1,4 @@\x1b[0m"));
    assert!(result.contains("\x1b[31m-old\x1b[0m"));
    assert!(result.contains("\x1b[32m+new\x1b[0m"));
}

#[test]
fn edit_preview_shapes_single_line_swap() {
    let p = format_edit_preview("src/a.rs", "old", "new", 1);
    assert!(p.contains("Edited src/a.rs"));
    assert!(p.contains("-old"));
    assert!(p.contains("+new"));
    assert!(looks_like_diff(&p));
    assert_eq!(preview_file_path(&p), Some("src/a.rs"));
}

#[test]
fn edit_preview_includes_line_numbers() {
    let p = format_edit_preview_at("src/a.rs", "old", "new", 1, Some(42));
    assert!(p.contains("  42|-old"), "{p}");
    assert!(p.contains("  42|+new"), "{p}");
    let parts = parse_diff_line("  42|-old");
    assert_eq!(parts.line_no, Some("42"));
    assert_eq!(parts.marker, Some('-'));
    assert_eq!(parts.body, "old");
}

#[test]
fn write_preview_is_one_sided_diff() {
    let p = format_write_preview("src/a.rs", "fn main() {}\n");
    assert!(p.contains("Wrote src/a.rs"));
    assert!(p.contains("+fn main() {}"));
    assert!(looks_like_diff(&p));
    assert_eq!(preview_file_path(&p), Some("src/a.rs"));
    assert!(p.contains("   1|+fn main() {}"), "{p}");
}

#[test]
fn looks_like_diff_rejects_plain_lists() {
    assert!(!looks_like_diff("- only removals\n- more"));
    assert!(!looks_like_diff("hello\nworld"));
    assert!(looks_like_diff("diff --git a/x b/x\n"));
}

#[test]
fn first_line_number_counts_newlines() {
    let hay = "a\nb\nc\n";
    assert_eq!(first_line_number(hay, "c"), Some(3));
    assert_eq!(first_line_number(hay, "a"), Some(1));
    assert_eq!(first_line_number(hay, ""), Some(1));
    assert_eq!(first_line_number(hay, "nope"), None);
}

#[test]
fn edit_preview_multi_replace_and_one_sided() {
    let many = format_edit_preview("f.rs", "old", "new", 3);
    assert!(many.contains("3 replacements"), "{many}");
    let add = format_edit_preview("f.rs", "", "only-new", 1);
    assert!(add.contains("+only-new"), "{add}");
    let del = format_edit_preview("f.rs", "only-old", "", 1);
    assert!(del.contains("-only-old"), "{del}");
}

#[test]
fn edit_preview_multiline_and_truncation() {
    let old = (0..45)
        .map(|i| format!("o{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = (0..45)
        .map(|i| format!("n{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let p = format_edit_preview("big.rs", &old, &new, 1);
    assert!(p.contains("more removed"), "{p}");
    assert!(p.contains("more added"), "{p}");
    let short = format_edit_preview("s.rs", "a\nb", "c\nd", 1);
    assert!(short.contains("-a"), "{short}");
    assert!(short.contains("+c"), "{short}");
}

#[test]
fn write_preview_empty_and_truncated() {
    let empty = format_write_preview("e.rs", "");
    assert!(empty.contains("(empty file)"), "{empty}");
    let body = (0..45)
        .map(|i| format!("l{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let p = format_write_preview("w.rs", &body);
    assert!(p.contains("more lines"), "{p}");
}

#[test]
fn parse_diff_line_headers_and_numbered() {
    for hdr in ["+++ a/x", "--- a/x", "@@ -1 +1 @@"] {
        let p = parse_diff_line(hdr);
        assert_eq!(p.marker, None, "{hdr}");
        assert_eq!(p.body, hdr);
    }
    let n = parse_diff_line("  12|-body");
    assert_eq!(n.line_no, Some("12"));
    assert_eq!(n.marker, Some('-'));
    let plus_hdr = parse_diff_line("12|+++");
    assert_eq!(plus_hdr.marker, None);
    let minus_hdr = parse_diff_line("12|---");
    assert_eq!(minus_hdr.marker, None);
    assert_eq!(parse_diff_line("plain").marker, None);
    assert_eq!(parse_diff_line("+add").marker, Some('+'));
}

#[test]
fn render_diff_remaining_sides_and_unified_headers() {
    let only_new = render_diff("", "fresh\n");
    assert!(only_new.contains("+ fresh"), "{only_new}");
    let only_old = render_diff("gone\n", "");
    assert!(only_old.contains("- gone"), "{only_old}");
    let uni = render_diff_unified("+++ a\n--- b\n context");
    assert!(uni.contains("\x1b[1m+++ a\x1b[0m"), "{uni}");
    assert!(looks_like_diff("+++ file\n"));
    assert!(looks_like_diff("Edited x\n-old\n+new"));
    assert!(preview_file_path("Edited   \n").is_none());
}
