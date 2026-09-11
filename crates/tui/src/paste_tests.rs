use super::*;

#[test]
fn short_single_line_stays_inline() {
    assert!(!should_collapse("hello world"));
    assert!(!should_collapse("short"));
    // Single line under the char threshold stays editable.
    assert!(!should_collapse(&"x".repeat(COLLAPSE_MIN_CHARS - 1)));
}

#[test]
fn two_or_more_lines_collapse() {
    assert!(should_collapse("a\nb"));
    assert!(should_collapse("a\nb\nc"));
}

#[test]
fn long_single_line_collapses() {
    let s = "x".repeat(COLLAPSE_MIN_CHARS);
    assert!(should_collapse(&s));
    assert!(!should_collapse(&"x".repeat(COLLAPSE_MIN_CHARS - 1)));
}

#[test]
fn placeholder_roundtrip_parse() {
    let p = placeholder(7, 42);
    assert_eq!(p, "[pasted #7 ~ 42 lines]");
    let spans = find_placeholders(&format!("fix {p} please"));
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].id, 7);
    assert_eq!(&format!("fix {p} please")[spans[0].start..spans[0].end], p);
}

#[test]
fn singular_line_unit() {
    assert_eq!(placeholder(1, 1), "[pasted #1 ~ 1 line]");
    let spans = find_placeholders("[pasted #1 ~ 1 line]");
    assert_eq!(spans.len(), 1);
}

#[test]
fn expand_replaces_known_blocks() {
    let blocks = vec![PastedBlock {
        id: 3,
        content: "one\ntwo\nthree".into(),
    }];
    let buf = format!("see {} end", placeholder(3, 3));
    assert_eq!(expand(&buf, &blocks), "see one\ntwo\nthree end");
}

#[test]
fn expand_keeps_unknown_token() {
    let buf = placeholder(99, 5);
    assert_eq!(expand(&buf, &[]), buf);
}

#[test]
fn prune_drops_orphans() {
    let mut blocks = vec![
        PastedBlock {
            id: 1,
            content: "a".into(),
        },
        PastedBlock {
            id: 2,
            content: "b".into(),
        },
    ];
    let buf = placeholder(2, 1);
    prune_unused(&mut blocks, &buf);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].id, 2);
}

#[test]
fn placeholder_at_cursor_inside() {
    let p = placeholder(1, 10);
    let buf = format!("x{p}y");
    let start = 1;
    let end = 1 + p.len();
    assert!(placeholder_at(&buf, start).is_some());
    assert!(placeholder_at(&buf, start + 3).is_some());
    assert!(placeholder_at(&buf, end).is_none()); // on boundary → outside
    assert_eq!(placeholder_ending_at(&buf, end).map(|s| s.id), Some(1));
    assert_eq!(placeholder_starting_at(&buf, start).map(|s| s.id), Some(1));
}

#[test]
fn line_count_counts_trailing_newline() {
    assert_eq!(line_count("a\nb"), 2);
    assert_eq!(line_count("a\nb\n"), 3);
    assert_eq!(line_count("solo"), 1);
    assert_eq!(line_count(""), 0);
    let block = PastedBlock {
        id: 1,
        content: "a\nb\n".into(),
    };
    assert_eq!(block.line_count(), 3);
}

#[test]
fn parse_placeholder_rejects_malformed_tokens() {
    assert!(find_placeholders("no token").is_empty());
    assert!(find_placeholders("[pasted #]").is_empty());
    assert!(find_placeholders("[pasted #x ~ 1 line]").is_empty());
    assert!(find_placeholders("[pasted #1~ 1 line]").is_empty());
    assert!(find_placeholders("[pasted #1 ~ line]").is_empty());
    assert!(find_placeholders("[pasted #1 ~ 1 foo]").is_empty());
    assert_eq!(find_placeholders("[pasted #2 ~ 1 line]").len(), 1);
}
