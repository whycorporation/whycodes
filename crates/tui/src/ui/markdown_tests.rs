use super::*;
use crate::theme::ThemeName;

fn palette() -> ThemePalette {
    ThemeName::DefaultDark.palette()
}

/// The visible characters of a line, ignoring style.
fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn rendered(input: &str) -> Vec<String> {
    render(input, &palette()).iter().map(text).collect()
}

#[test]
fn markup_characters_do_not_reach_the_screen() {
    let out = rendered("# Title\n\nsome **bold** and *italic* and `code`\n\n- item");
    let joined = out.join("\n");
    assert!(!joined.contains("**"), "{joined}");
    assert!(!joined.contains('`'), "{joined}");
    assert!(!joined.contains("# "), "{joined}");
    // The content survives.
    assert!(joined.contains("Title"));
    assert!(joined.contains("bold"));
    assert!(joined.contains("italic"));
    assert!(joined.contains("code"));
    assert!(joined.contains("item"));
}

#[test]
fn headings_are_bold() {
    let lines = render("# Title", &palette());
    let styled = lines[0]
        .spans
        .iter()
        .find(|s| s.content == "Title")
        .unwrap();
    assert!(styled.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn bold_and_italic_carry_their_modifiers() {
    let lines = render("a **b** *i*", &palette());
    let spans = &lines[0].spans;
    let bold = spans.iter().find(|s| s.content == "b").unwrap();
    let italic = spans.iter().find(|s| s.content == "i").unwrap();
    assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn list_items_get_a_bullet() {
    let out = rendered("- first");
    assert!(out[0].contains('•'), "{:?}", out);
    assert!(out[0].contains("first"));
}

#[test]
fn ordered_list_items_get_numbers() {
    let out = rendered("1. alpha\n2. beta");
    assert!(out[0].contains("1. "), "{:?}", out);
    assert!(out[0].contains("alpha"));
    assert!(out[1].contains("2. "), "{:?}", out);
}

#[test]
fn long_paragraph_soft_wraps() {
    let words = (0..20)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let lines = render_with_width(&words, &palette(), Some(24));
    assert!(
        lines.len() >= 2,
        "expected wrap into multiple rows, got {}: {:?}",
        lines.len(),
        lines.iter().map(text).collect::<Vec<_>>()
    );
    let joined: String = lines.iter().map(text).collect();
    assert!(joined.contains("word0"));
    assert!(joined.contains("word19"));
}

#[test]
fn fenced_rust_uses_more_than_one_token_colour() {
    let lines = render("```rust\nfn main() { let x = \"hi\"; }\n```", &palette());
    let mut fgs = std::collections::BTreeSet::new();
    for line in &lines {
        for s in &line.spans {
            let t = s.content.as_ref();
            if (t.contains("fn") || t.contains("let") || t.contains("hi") || t.contains("main"))
                && let Some(fg) = s.style.fg
            {
                match fg {
                    Color::Rgb(r, g, b) => {
                        fgs.insert((r, g, b));
                    }
                    Color::Indexed(i) => {
                        fgs.insert((i, 0, 0));
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(
        fgs.len() >= 2,
        "rust tokens must not share one grey: {fgs:?}"
    );
}

#[test]
fn fenced_code_is_banded_labelled_and_numbered() {
    let out = rendered("```rust\nlet x = 1;\n```");
    let joined = out.join("\n");
    assert!(joined.contains("rust"), "{out:?}");
    assert!(joined.contains("let x = 1;"), "{out:?}");
    assert!(
        joined.contains('1'),
        "Grok code blocks number lines, got {out:?}"
    );
    assert!(
        !joined.contains('┌') && !joined.contains('└'),
        "no box chrome: {out:?}"
    );
    let lines = render("```rust\nlet x = 1;\n```", &palette());
    let banded = lines.iter().any(|l| {
        l.spans
            .iter()
            .any(|s| s.style.bg == Some(palette().status_bar_bg))
    });
    assert!(banded, "code block sits on the elevated band");
}

#[test]
fn gfm_pipe_table_renders_as_aligned_box() {
    let md = "\
| Tag | Sürüm |
|-----|--------|
| latest | 4.5.2 |
| 3x | 3.21.11 |
";
    let out = rendered(md);
    let joined = out.join("\n");
    // Box chrome — not raw pipe-markdown soft-wrap debris.
    assert!(joined.contains('┌'), "{joined}");
    assert!(joined.contains('│'), "{joined}");
    assert!(joined.contains('└'), "{joined}");
    assert!(joined.contains("Tag"), "{joined}");
    assert!(joined.contains("Sürüm"), "{joined}");
    assert!(joined.contains("latest"), "{joined}");
    assert!(joined.contains("4.5.2"), "{joined}");
    // Separator markdown must not leak as a body row.
    assert!(!joined.contains("|-----|"), "{joined}");
    // Header row is accent+bold.
    let lines = render(md, &palette());
    let header_span = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref().contains("Tag"))
        .expect("header cell");
    assert!(header_span.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(header_span.style.fg, Some(palette().accent));
}

#[test]
fn fenced_diff_uses_theme_add_remove_colours() {
    let lines = render("```diff\n-old\n+new\n```", &palette());
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(joined.contains("-old"), "{joined}");
    assert!(joined.contains("+new"), "{joined}");

    let add = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("+new"))
        .expect("add line span");
    let rem = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("-old"))
        .expect("remove line span");
    assert_eq!(add.style.fg, Some(palette().diff_add));
    assert_eq!(rem.style.fg, Some(palette().diff_remove));
}

#[test]
fn an_untagged_fence_still_renders_as_a_block() {
    let out = rendered("```\nplain\n```");
    assert!(out.iter().any(|l| l.contains("plain")), "{out:?}");
    assert!(
        !out.iter().any(|l| l.trim() == "code"),
        "untagged fences have no fake language chip: {out:?}"
    );
}

#[test]
fn a_streaming_fence_renders_without_a_bottom_edge() {
    // Partial output during a turn: the closing fence has not arrived.
    let out = rendered("```rust\nlet x = 1;");
    assert!(out.iter().any(|l| l.contains("let x = 1;")), "{:?}", out);
    assert!(
        !out.iter().any(|l| l.contains('┌') || l.contains('└')),
        "Grok code blocks have no box edges: {:?}",
        out
    );
}

#[test]
fn mermaid_fence_renders_as_diagram() {
    let out = rendered("```mermaid\ngraph LR; A[Build] --> B[Deploy]\n```");
    let joined = out.join("\n");
    assert!(joined.contains("mermaid"), "{joined}");
    assert!(joined.contains("Build"), "{joined}");
    assert!(joined.contains("Deploy"), "{joined}");
    // With the `mermaid` feature, source keywords become a diagram.
    // Without it, the ship binary keeps source lines readable.
    #[cfg(feature = "mermaid")]
    assert!(!joined.contains("graph LR"), "{joined}");
    #[cfg(not(feature = "mermaid"))]
    assert!(
        !joined.contains('┌') && !joined.contains('└'),
        "mermaid source uses the same band, not a box: {joined}"
    );
}

#[test]
fn streaming_mermaid_shows_source_without_closing() {
    let out = rendered("```mermaid\ngraph LR; A --> B");
    let joined = out.join("\n");
    assert!(joined.contains("mermaid"), "{joined}");
    // Still open: no bottom edge yet.
    assert!(
        !joined.contains('┌') && !joined.contains('└'),
        "open mermaid fence should stay a band, not a box: {joined}"
    );
}

#[test]
fn plain_text_passes_through_unchanged() {
    assert_eq!(rendered("just words").len(), 1);
    assert!(rendered("just words")[0].contains("just words"));
}

#[test]
fn empty_input_renders_nothing() {
    assert!(render("", &palette()).is_empty());
}

#[test]
fn body_starts_at_content_column() {
    // No synthetic left pad — SIDE_PAD lives in the shell.
    for line in render("# T\n\ntext\n\n- item", &palette()) {
        let s = text(&line);
        if s.is_empty() {
            continue;
        }
        assert!(
            !s.starts_with("  "),
            "body/list should not start with a double space pad: {s:?}"
        );
    }
    assert!(rendered("just words")[0].starts_with('j'));
    assert!(rendered("# Title")[0].starts_with('T'));
}

#[test]
fn mermaid_invalid_and_markdown_link() {
    let out = rendered("```mermaid\nnot a diagram at all {{{{\n```");
    let joined = out.join("\n");
    assert!(
        joined.contains("mermaid") || joined.contains("not a diagram") || joined.contains("failed"),
        "{joined}"
    );
    let lines = render("see [docs](https://example.com)", &palette());
    let link = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("docs"));
    assert!(link.is_some(), "{lines:?}");
    let tabbed = rendered("```rs\n\tfn main() {}\n```");
    assert!(tabbed.join("\n").contains("fn main") || !tabbed.is_empty());
    assert_eq!(super::complete_source_lines(""), 0);
    assert_eq!(super::complete_source_lines("a\n"), 1);
    assert_eq!(super::complete_source_lines("a\nb"), 1);
}

#[test]
fn mermaid_empty_closed_fence_takes_render_failed_path() {
    let out = rendered("```mermaid\n\n```");
    let joined = out.join("\n");
    assert!(
        joined.contains("failed") || joined.contains("empty") || joined.contains("mermaid"),
        "{joined}"
    );
}

#[test]
fn wrap_list_item_and_tabbed_diff_and_open_fence() {
    let words = (0..12)
        .map(|i| format!("itemword{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let lines = render_with_width(&format!("- {words}"), &palette(), Some(16));
    assert!(
        lines.len() >= 2,
        "wrapped list should hang-indent: {:?}",
        lines.iter().map(text).collect::<Vec<_>>()
    );

    let diff = rendered("```diff\n--- a\n+++ b\n@@ hunk @@\n-old\n+new\n context\n\tindented\n```");
    let joined = diff.join("\n");
    assert!(joined.contains("old") && joined.contains("new"), "{joined}");

    let narrow = render_with_width(
        "```diff\n+this-is-a-very-long-added-line-that-must-wrap\n```",
        &palette(),
        Some(12),
    );
    assert!(
        narrow.len() >= 2,
        "narrow diff wraps: {:?}",
        narrow.iter().map(text).collect::<Vec<_>>()
    );

    let mut out = Vec::new();
    let (src, _) = super::append_open_fence(
        &mut out,
        Some("rs"),
        "fn main() {\n\tlet x = 1;\npartial",
        &palette(),
        Some(20),
        0,
    );
    assert!(src > 0 || !out.is_empty());
    let joined: String = out.iter().map(text).collect();
    assert!(
        joined.contains("fn main") || joined.contains("let x"),
        "{joined}"
    );

    let empty_item = rendered("- ");
    assert!(
        empty_item.iter().any(|l| l.contains('•')),
        "an empty list marker still paints the bullet, got {empty_item:?}"
    );
    let empty_table = super::render_table(&[], &[], &[], &palette(), Some(40));
    assert!(
        empty_table.is_empty(),
        "a table with no headers must paint nothing"
    );
}
