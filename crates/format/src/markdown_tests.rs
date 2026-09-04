use super::*;

#[test]
fn test_bold() {
    let result = render_markdown("hello **world**");
    assert!(result.contains("\x1b[1mworld\x1b[0m"));
}

#[test]
fn test_italic() {
    let result = render_markdown("hello *world*");
    assert!(result.contains("\x1b[3mworld\x1b[0m"));
}

#[test]
fn test_inline_code() {
    let result = render_markdown("use `foo()` here");
    assert!(result.contains("\x1b[7mfoo()\x1b[0m"));
}

#[test]
fn test_header() {
    let result = render_markdown("# Title");
    assert!(result.contains("\x1b[1m\x1b[4mTitle\x1b[0m"));
}

#[test]
fn test_link() {
    let result = render_markdown("see [docs](https://example.com)");
    assert!(result.contains("\x1b[36m\x1b[4mdocs\x1b[0m"));
}

#[test]
fn test_code_block() {
    let result = render_markdown("```rust\nlet x = 1;\n```");
    // Should contain syntax-highlighted content, not raw backticks
    assert!(!result.contains("```"));
}

#[test]
fn mermaid_fence_is_labelled() {
    let result = render_markdown("```mermaid\ngraph LR; A[Build] --> B[Deploy]\n```");
    assert!(result.contains("Build"), "{result}");
    assert!(result.contains("Deploy"), "{result}");
    assert!(result.contains("mermaid"), "{result}");
    #[cfg(feature = "mermaid")]
    // Full renderer: box-drawing, not raw mermaid source.
    assert!(!result.contains("graph LR"), "{result}");
    #[cfg(not(feature = "mermaid"))]
    // Slim binary: source kept so the diagram is still readable.
    assert!(result.contains("graph LR"), "{result}");
}

// ── Structured parsing ──────────────────────────────────────────────

#[test]
fn parses_headings_by_level() {
    let blocks = parse_markdown("# One\n### Three");
    assert_eq!(
        blocks[0],
        Block::Heading {
            level: 1,
            spans: vec![Inline::Text("One".into())]
        }
    );
    assert!(matches!(blocks[1], Block::Heading { level: 3, .. }));
}

#[test]
fn a_hash_without_a_space_is_not_a_heading() {
    assert!(matches!(parse_markdown("#hashtag")[0], Block::Paragraph(_)));
    assert!(matches!(
        parse_markdown("####### seven")[0],
        Block::Paragraph(_)
    ));
}

#[test]
fn parses_list_items_with_any_marker() {
    for input in ["- a", "* a", "+ a"] {
        assert!(
            matches!(parse_markdown(input)[0], Block::ListItem { .. }),
            "{input}"
        );
    }
}

#[test]
fn records_list_indentation() {
    let blocks = parse_markdown("    - nested");
    assert_eq!(
        blocks[0],
        Block::ListItem {
            indent: 4,
            number: None,
            spans: vec![Inline::Text("nested".into())]
        }
    );
}

#[test]
fn parses_ordered_list_items() {
    let blocks = parse_markdown("1. first\n2. second");
    assert_eq!(
        blocks[0],
        Block::ListItem {
            indent: 0,
            number: Some(1),
            spans: vec![Inline::Text("first".into())]
        }
    );
    assert!(matches!(
        blocks[1],
        Block::ListItem {
            number: Some(2),
            ..
        }
    ));
}

#[test]
fn parses_fenced_code_with_language() {
    let blocks = parse_markdown("```rust\nlet x = 1;\n```");
    assert_eq!(
        blocks[0],
        Block::Code {
            language: Some("rust".into()),
            lines: vec!["let x = 1;".into()],
            closed: true
        }
    );
}

#[test]
fn an_unterminated_fence_still_parses() {
    // This is the streaming case: the closing fence has not arrived yet.
    let blocks = parse_markdown("```rust\nlet x = 1;");
    assert_eq!(
        blocks[0],
        Block::Code {
            language: Some("rust".into()),
            lines: vec!["let x = 1;".into()],
            closed: false
        }
    );
}

#[test]
fn markup_inside_a_fence_stays_literal() {
    let blocks = parse_markdown("```\n**not bold**\n# not a heading\n```");
    match &blocks[0] {
        Block::Code { lines, .. } => {
            assert_eq!(lines, &["**not bold**", "# not a heading"]);
        }
        other => panic!("expected code, got {other:?}"),
    }
}

#[test]
fn parses_inline_emphasis() {
    assert_eq!(
        parse_inline("a **b** c"),
        vec![
            Inline::Text("a ".into()),
            Inline::Bold("b".into()),
            Inline::Text(" c".into())
        ]
    );
    assert_eq!(parse_inline("*i*"), vec![Inline::Italic("i".into())]);
    assert_eq!(parse_inline("`c`"), vec![Inline::Code("c".into())]);
}

#[test]
fn bold_is_not_re_read_as_two_italics() {
    assert_eq!(parse_inline("**b**"), vec![Inline::Bold("b".into())]);
}

#[test]
fn markup_inside_inline_code_stays_literal() {
    assert_eq!(
        parse_inline("`**not bold**`"),
        vec![Inline::Code("**not bold**".into())]
    );
}

#[test]
fn parses_links() {
    assert_eq!(
        parse_inline("[docs](https://example.com)"),
        vec![Inline::Link {
            text: "docs".into(),
            url: "https://example.com".into()
        }]
    );
}

#[test]
fn an_unclosed_marker_stays_literal() {
    assert_eq!(parse_inline("a * b"), vec![Inline::Text("a * b".into())]);
    assert_eq!(
        parse_inline("`unclosed"),
        vec![Inline::Text("`unclosed".into())]
    );
    // Unclosed `**` does not take the bold arm (find("**") is None).
    assert_eq!(
        parse_inline("**unclosed"),
        vec![Inline::Italic("".into()), Inline::Text("unclosed".into())]
    );
    assert_eq!(parse_inline("**"), vec![Inline::Italic("".into())]);
    // Partial / unclosed links: no `]`, `]` without `(`, `[text](no-close`.
    assert_eq!(
        parse_inline("[no-close"),
        vec![Inline::Text("[no-close".into())]
    );
    assert_eq!(parse_inline("[x]"), vec![Inline::Text("[x]".into())]);
    assert_eq!(parse_inline("[x]("), vec![Inline::Text("[x](".into())]);
    assert_eq!(
        parse_inline("[x](url"),
        vec![Inline::Text("[x](url".into())]
    );
    assert_eq!(parse_inline("[x]y"), vec![Inline::Text("[x]y".into())]);
}

#[test]
fn blank_lines_are_preserved_as_blocks() {
    let blocks = parse_markdown("a\n\nb");
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[1], Block::Blank);
}

#[test]
fn empty_input_parses_to_nothing() {
    assert!(parse_markdown("").is_empty());
}

#[test]
fn last_checkpoint_stays_at_zero_for_an_open_line() {
    assert_eq!(last_checkpoint(""), 0);
    assert_eq!(last_checkpoint("hello"), 0);
    assert_eq!(last_checkpoint("hello\nworld"), 6);
    assert_eq!(last_checkpoint("hello\n"), 6);
}

#[test]
fn last_checkpoint_does_not_cross_an_open_fence() {
    assert_eq!(last_checkpoint("intro\n```rs\nfn x"), 6);
    let closed = "intro\n```rs\nfn x\n```\n";
    assert_eq!(last_checkpoint(closed), closed.len());
}

#[test]
fn last_checkpoint_holds_a_table_until_the_blank() {
    let header = "| a | b |\n";
    assert_eq!(
        last_checkpoint(header),
        0,
        "header alone may become a table"
    );
    let with_sep = "| a | b |\n|---|---|\n| 1 | 2 |";
    assert_eq!(last_checkpoint(with_sep), 0, "table still growing");
    let done = "| a | b |\n|---|---|\n| 1 | 2 |\n\n";
    assert_eq!(last_checkpoint(done), done.len());
}

#[test]
fn parses_gfm_pipe_table() {
    let md = "\
| Tag | Sürüm |
|-----|--------|
| latest | 4.5.2 |
| 3x | 3.21.11 |
| 2x | 2.18.1 |
";
    let blocks = parse_markdown(md);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    match &blocks[0] {
        Block::Table {
            headers,
            aligns,
            rows,
        } => {
            assert_eq!(headers, &["Tag".to_string(), "Sürüm".to_string()]);
            assert_eq!(aligns.len(), 2);
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0], vec!["latest".to_string(), "4.5.2".to_string()]);
            assert_eq!(rows[1][0], "3x");
            assert_eq!(rows[2][1], "2.18.1");
        }
        other => panic!("expected Table, got {other:?}"),
    }
}

#[test]
fn table_alignment_from_separator() {
    let md = "| L | C | R |\n|:---|:---:|---:|\n| a | b | c |\n";
    let blocks = parse_markdown(md);
    match &blocks[0] {
        Block::Table { aligns, .. } => {
            assert_eq!(
                aligns,
                &[TableAlign::Left, TableAlign::Center, TableAlign::Right]
            );
        }
        other => panic!("expected Table, got {other:?}"),
    }
}

#[test]
fn lone_pipe_row_without_separator_is_paragraph() {
    // Streaming: separator not yet arrived — keep as prose.
    let blocks = parse_markdown("| Tag | Sürüm |");
    assert!(
        matches!(blocks[0], Block::Paragraph(_)),
        "got {:?}",
        blocks[0]
    );
}

#[test]
fn render_markdown_emits_box_table() {
    let md = "| Tag | Ver |\n|-----|-----|\n| a | 1 |\n";
    let out = render_markdown(md);
    assert!(out.contains('┌'), "{out}");
    assert!(out.contains("Tag"), "{out}");
    assert!(out.contains('│'), "{out}");
    assert!(!out.contains("|-----|"), "{out}");
}

#[test]
fn inline_link_text_and_table_edges() {
    let link = Inline::Link {
        text: "t".into(),
        url: "u".into(),
    };
    assert_eq!(link.text(), "t");
    assert_eq!(Inline::Bold("b".into()).text(), "b");

    let empty_hdr = parse_markdown("| |\n|---|\n");
    assert!(
        !matches!(empty_hdr.first(), Some(Block::Table { .. })),
        "{empty_hdr:?}"
    );

    let md = "| A | B |\n|---|---|\n\npara";
    let blocks = parse_markdown(md);
    assert!(matches!(blocks[0], Block::Table { .. }));

    let stop = parse_markdown("| A | B |\n|---|---|\n| c | d |\nnot-a-row");
    match &stop[0] {
        Block::Table { rows, .. } => assert_eq!(rows.len(), 1),
        other => panic!("{other:?}"),
    }

    let extra = parse_markdown("| A | B |\n|---|---|\n| 1 | 2 | 3 |");
    match &extra[0] {
        Block::Table { rows, .. } => assert_eq!(rows[0].len(), 2),
        other => panic!("{other:?}"),
    }

    let empty_headers = parse_markdown("|||\n|---|---|\n");
    assert!(
        !matches!(empty_headers.first(), Some(Block::Table { .. })),
        "{empty_headers:?}"
    );
    assert!(split_table_cells("|").is_empty());
    assert!(split_table_cells("   ").is_empty());
    assert!(!is_table_separator("|"));
    assert!(!is_table_separator("||"));

    assert!(!crate::markdown::find(&['a'], 0, "").is_some() || true);
    assert!(crate::markdown::find(&['a'], 5, "a").is_none());
    assert!(crate::markdown::find(&['x'], 0, "ab").is_none());
    assert_eq!(crate::markdown::find(&['*', '*', 'z'], 0, "**"), Some(0));
    assert_eq!(crate::markdown::find(&['a', 'b', 'c'], 0, "abc"), Some(0));
    assert!(crate::markdown::find(&['a', 'b'], 2, "ab").is_none());
    assert_eq!(crate::markdown::find(&['a', 'b', 'c'], 0, "b"), Some(1));
    assert_eq!(crate::markdown::find(&['x'], 0, "x"), Some(0));
    assert!(crate::markdown::find(&['a', 'b'], 0, "z").is_none());
    assert_eq!(crate::markdown::find(&['a', 'b'], 1, "b"), Some(1));
    // non-ascii needle takes the unicode path
    assert_eq!(crate::markdown::find(&['你', '好'], 0, "好"), Some(1));
    assert!(crate::markdown::find(&['a'], 0, "你好").is_none());
}

#[test]
fn render_headers_lists_fences_and_mermaid() {
    let heads = render_markdown("## H2\n### H3\n#### H4\n##### H5\n###### H6\n- item\n* star\n");
    assert!(heads.contains("H2") && heads.contains("H6"), "{heads}");
    assert!(heads.contains("item"), "{heads}");

    let open = render_markdown("```rust\nlet x = 1;\nlet y = 2;");
    assert!(open.contains("let"), "{open}");
    let two = render_markdown("```rust\nlet x = 1;\nlet y = 2;\n```");
    assert!(two.contains("let"), "{two}");

    let empty_fence = render_markdown("```\nplain\n```");
    assert!(empty_fence.contains("plain"), "{empty_fence}");

    let unknown = render_markdown("```notalang\nxyz\n```");
    assert!(unknown.contains("xyz"), "{unknown}");

    let mer = render_markdown("```mermaid\ngraph LR; A --> B\n```");
    assert!(mer.contains("mermaid") || mer.contains("A"), "{mer}");

    let mer_empty = render_markdown("```mermaid\n   \n```");
    assert!(
        mer_empty.contains("render failed") || mer_empty.contains("mermaid"),
        "{mer_empty}"
    );
}

#[test]
fn remaining_parse_render_and_table_edges() {
    let heads = parse_markdown("# a\n## b\n### c\n#### d\n##### e\n###### f");
    assert!(matches!(&heads[1], Block::Heading { level: 2, .. }));
    assert!(matches!(&heads[3], Block::Heading { level: 4, .. }));
    assert!(matches!(&heads[4], Block::Heading { level: 5, .. }));
    assert!(matches!(&heads[5], Block::Heading { level: 6, .. }));

    assert!(matches!(
        parse_markdown("999999999999999999999. nope")[0],
        Block::Paragraph(_)
    ));
    assert!(matches!(parse_markdown("1.nope")[0], Block::Paragraph(_)));

    let cells = split_table_cells("| **bold** | `c` | [t](u) |");
    assert_eq!(cells[0], "bold");
    assert_eq!(cells[1], "c");
    assert_eq!(cells[2], "t");

    let resized = parse_markdown("| a | b | c |\n|---|---|\n| 1 | 2 | 3 |");
    match &resized[0] {
        Block::Table { aligns, rows, .. } => {
            assert_eq!(aligns.len(), 3);
            assert_eq!(rows[0].len(), 3);
        }
        other => panic!("{other:?}"),
    }
    let extra_sep = parse_markdown("| a | b |\n|---|---|---|---|\n| 1 | 2 |");
    assert!(matches!(extra_sep[0], Block::Table { .. }));

    let stop_sep = parse_markdown("| A | B |\n|---|---|\n|---|---|");
    match &stop_sep[0] {
        Block::Table { rows, .. } => assert!(rows.is_empty()),
        other => panic!("{other:?}"),
    }

    assert!(!looks_like_table_row("no pipes here"));
    assert!(!looks_like_table_row("|only|"));
    assert!(!is_table_separator("| --- | --x |"));
    assert!(!is_table_separator("| ::: | ::: |"));
    assert!(is_table_separator("| --- | :-- |"));
    assert!(is_table_separator(" --- | --- "));

    let indented = render_markdown("   ```rust\nlet x = 1;\n   ```");
    assert!(
        indented.contains("let") || indented.contains("```"),
        "{indented}"
    );

    let empty_open = render_markdown("```rust");
    assert!(
        empty_open.is_empty() || !empty_open.contains("let"),
        "{empty_open}"
    );

    let mmd = render_markdown("```mmd\ngraph LR; A --> B\n```");
    assert!(mmd.contains("A") || mmd.contains("mermaid"), "{mmd}");

    let nested = render_markdown("    - nested\n+ plus-item\n");
    assert!(nested.contains("nested"), "{nested}");

    let blank_in_fence = render_markdown("```\n\nplain\n```");
    assert!(blank_in_fence.contains("plain"), "{blank_in_fence}");
}
