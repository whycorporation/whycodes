use super::*;
use crate::app::{ChatBlock, ChatRole, ThinkingBlock};
use crate::theme::ThemeName;

fn palette() -> ThemePalette {
    ThemeName::DefaultDark.palette()
}

fn line_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn widget(role: ChatRole, content: &str, blocks: Vec<ChatBlock>) -> MessageWidget {
    MessageWidget {
        role,
        content: content.to_string(),
        blocks,
        results_expanded: true,
        error: None,
    }
}

#[test]
fn role_headers_use_prefix_and_role_style() {
    let p = palette();
    for (role, prefix, color) in [
        (ChatRole::User, "▸ ", p.user_msg),
        (ChatRole::Assistant, "○ ", p.assistant_msg),
        (ChatRole::System, "⚙ ", p.system_msg),
        (ChatRole::Tool, "  ↪ ", p.tool_msg),
    ] {
        let w = widget(role.clone(), "", vec![]);
        let lines = w.to_lines(&p);
        assert_eq!(
            line_text(&lines[0]),
            format!("{prefix}{}", role.as_str()),
            "{role:?}"
        );
        // Role header carries the role color + bold.
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(color), "{role:?}");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        // Trailing blank separator line.
        assert_eq!(line_text(lines.last().unwrap()), "", "{role:?}");
    }
}

#[test]
fn content_lines_are_indented() {
    let w = widget(ChatRole::User, "first\nsecond", vec![]);
    let lines = w.to_lines(&palette());
    assert_eq!(line_text(&lines[1]), "  first");
    assert_eq!(line_text(&lines[2]), "  second");
    // Regular content uses the neutral foreground.
    assert_eq!(lines[1].spans[0].style.fg, Some(palette().fg));
}

#[test]
fn text_and_tool_use_blocks() {
    let w = widget(
        ChatRole::Assistant,
        "",
        vec![
            ChatBlock::Text("block line".into()),
            ChatBlock::ToolUse {
                id: "t1".into(),
                name: "read".into(),
                input: serde_json::json!({ "path": "a.rs" }),
            },
        ],
    );
    let lines = w.to_lines(&palette());
    let all: Vec<String> = lines.iter().map(line_text).collect();
    assert!(all.iter().any(|l| l == "  block line"), "{all:?}");
    assert!(
        all.iter()
            .any(|l| l.contains("🔧 read") && l.contains("\"path\": \"a.rs\"")),
        "tool use shows name + pretty JSON: {all:?}"
    );
}

#[test]
fn tool_result_truncated_and_error_colored() {
    let p = palette();
    // Long content is capped with a notice.
    let long = "x".repeat(600);
    let w = widget(
        ChatRole::Tool,
        "",
        vec![ChatBlock::ToolResult {
            id: "r1".into(),
            content: long.clone(),
            is_error: false,
        }],
    );
    let lines = w.to_lines(&p);
    let result_line = lines[1].clone();
    let text = line_text(&result_line);
    assert!(text.starts_with("    ↳ "), "{text}");
    assert!(text.ends_with("... (truncated)"), "len={}", text.len());
    assert!(text.len() < 600, "capped, not full content");
    // 499 ASCII + `ö` (bytes 499..501) — `&content[..500]` panics.
    let utf8 = format!("{}ö{}", "x".repeat(499), "y".repeat(20));
    assert!(!utf8.is_char_boundary(500));
    let w = widget(
        ChatRole::Tool,
        "",
        vec![ChatBlock::ToolResult {
            id: "r-utf8".into(),
            content: utf8,
            is_error: false,
        }],
    );
    let text = line_text(&w.to_lines(&p)[1]);
    assert!(text.ends_with("... (truncated)"), "{text}");
    assert!(text.is_char_boundary(text.len()));
    // Error results use the error color.
    let w = widget(
        ChatRole::Tool,
        "",
        vec![ChatBlock::ToolResult {
            id: "r2".into(),
            content: "boom".into(),
            is_error: true,
        }],
    );
    let lines = w.to_lines(&p);
    assert_eq!(
        lines[1].spans[0].style.fg,
        Some(p.error),
        "error result colored"
    );
    assert!(
        !lines[1].spans[0].style.add_modifier.contains(Modifier::DIM),
        "SGR DIM leaks green on Apple Terminal.app"
    );
}

#[test]
fn error_line_appended_with_error_color() {
    let mut w = widget(ChatRole::Assistant, "hi", vec![]);
    w.error = Some("call failed".into());
    let lines = w.to_lines(&palette());
    let err_line = &lines[lines.len() - 2];
    assert_eq!(line_text(err_line), "  ✗ call failed");
    assert_eq!(
        err_line.spans[0].style.fg,
        Some(palette().error),
        "error line colored"
    );
}

#[test]
fn thinking_widget_body_is_palette_dim_not_sgr_dim() {
    let p = palette();
    let mut t = ThinkingBlock::new("widget body");
    t.collapsed = false;
    let lines = thinking_widget_lines(&t, &p);
    let span = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("widget body"))
        .expect("body");
    assert_eq!(span.style.fg, Some(p.dim));
    assert!(!span.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn thinking_running_shows_live_tail() {
    let p = palette();
    let t = ThinkingBlock::new("one\ntwo\nthree\nfour\nfive\nsix");
    // Running + collapsed → header + last 3 lines as the live tail.
    let lines = thinking_widget_lines(&t, &p);
    let all: Vec<String> = lines.iter().map(line_text).collect();
    assert!(all[0].contains("Thinking"), "{all:?}");
    assert!(all.iter().any(|l| l.contains("four")), "{all:?}");
    assert!(all.iter().any(|l| l.contains("six")), "{all:?}");
    assert!(!all.iter().any(|l| l.contains("one")), "tail only: {all:?}");
    // Rail glyph on every painted line.
    assert!(all.iter().all(|l| l.contains('┃')), "{all:?}");
}

#[test]
fn thinking_finished_collapsed_has_no_trailing_chevron() {
    let p = palette();
    let mut t = ThinkingBlock::new("body line");
    t.finish();
    let lines = thinking_widget_lines(&t, &p);
    let all: Vec<String> = lines.iter().map(line_text).collect();
    assert!(all[0].contains("Thought for"), "{all:?}");
    assert!(
        !all[0].contains('›') && !all[0].contains('>'),
        "no trailing chevron: {all:?}"
    );
    assert_eq!(all.len(), 1, "no body when collapsed: {all:?}");
}

#[test]
fn thinking_finished_expanded_shows_full_body() {
    let p = palette();
    let mut t = ThinkingBlock::new("line a\nline b");
    t.collapsed = false;
    t.finish();
    let lines = thinking_widget_lines(&t, &p);
    let all: Vec<String> = lines.iter().map(line_text).collect();
    assert!(all[0].contains("Thought for"), "{all:?}");
    assert!(!all[0].contains('›'), "expanded: {all:?}");
    assert!(all.iter().any(|l| l.contains("line a")), "{all:?}");
    assert!(all.iter().any(|l| l.contains("line b")), "{all:?}");
}

#[test]
fn thinking_header_label_matches_lifecycle() {
    let mut t = ThinkingBlock::new("x");
    // Freshly started → "Thinking…" (0.0s not worth showing).
    assert!(
        t.header_label().starts_with("Thinking"),
        "{}",
        t.header_label()
    );
    t.finish();
    assert!(
        t.header_label().starts_with("Thought for"),
        "{}",
        t.header_label()
    );
}
