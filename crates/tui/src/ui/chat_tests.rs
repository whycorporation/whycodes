use super::{
    SparseLines, ToolOutHint, ToolPaint, ellipsize_bytes, hard_truncate_line,
    message_row_layout_mut, parse_grep_hit, prettify_tool_result, split_read_line, tool_block,
    tool_display_name, tool_result, tool_summary, visible_message_range,
};
use crate::app::{ChatRole, TuiApp};
use crate::config::TuiAppConfig;
use crate::theme::ThemeName;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use serde_json::json;

#[test]
fn prettify_minified_json_becomes_multiline() {
    let mini = r#"{"exports":{"./config":"./config.js","./schema":"./schema.js"},"license":"MIT"}"#;
    let out = prettify_tool_result(mini);
    assert!(
        out.lines().count() > 3,
        "expected pretty multi-line JSON, got {out:?}"
    );
    assert!(out.contains("\"exports\""));
    assert!(out.contains("\"license\""));
}

#[test]
fn prettify_webfetch_envelope_keeps_headers() {
    let raw = "URL: https://registry.npmjs.org/nuxt/latest\nStatus: 200\nContent-Type: application/json\n\n\
                   {\"name\":\"nuxt\",\"version\":\"3.0.0\",\"exports\":{\"./x\":\"./y.js\"}}";
    let out = prettify_tool_result(raw);
    assert!(out.contains("URL: https://registry.npmjs.org/nuxt/latest"));
    assert!(out.contains("Status: 200"));
    assert!(
        out.lines().count() > 6,
        "headers + pretty body should be multi-line, got:\n{out}"
    );
    assert!(out.contains("\"name\""));
}

#[test]
fn hard_truncate_line_caps_display_width() {
    let s = hard_truncate_line(&"x".repeat(100), 20);
    assert_eq!(s.chars().count(), 20);
    assert!(s.ends_with('…'));
}

#[test]
fn tool_result_minified_json_stays_within_preview_budget() {
    // One giant minified line used to wrap into the entire preview and look
    // like the assistant answer (┃ dump filling the transcript).
    let mini = format!(
        r#"{{"exports":{{"./entry":"{}","license":"MIT","_npmUser":{{"name":"GitHub"}}}}}}"#,
        "z".repeat(800)
    );
    let palette = ThemeName::DefaultDark.palette();
    let lines = tool_result(&mini, false, &palette, false, ToolOutHint::Auto, 80);
    // Preview budget 12 + optional "more lines" footer.
    assert!(
        lines.len() <= 14,
        "minified tool dump must not explode into {} rows",
        lines.len()
    );
    assert!(!lines.is_empty());
    // Rail still present on body rows.
    let body = lines
        .iter()
        .filter(|l| l.spans.iter().any(|s| s.content.as_ref().contains('┃')))
        .count();
    assert!(body >= 1);
}

#[test]
fn tool_result_diff_paints_add_remove_colours() {
    let palette = ThemeName::DefaultDark.palette();
    let body = "Edited src/main.rs\n\n  12|-old\n  12|+new\n";
    let lines = tool_result(body, false, &palette, false, ToolOutHint::Diff, 80);
    let add = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "+")
        .expect("+ marker");
    let rem = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "-")
        .expect("- marker");
    assert_eq!(add.style.fg, Some(palette.diff_add));
    assert_eq!(rem.style.fg, Some(palette.diff_remove));
    // Visible wash background on add/remove rows.
    assert!(add.style.bg.is_some());
    assert!(rem.style.bg.is_some());
    // Full line body stays green/red (not syntax-overwritten).
    let add_body = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "new")
        .expect("+ body");
    let rem_body = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "old")
        .expect("- body");
    assert_eq!(add_body.style.fg, Some(palette.diff_add));
    assert_eq!(rem_body.style.fg, Some(palette.diff_remove));
    // Left line numbers also green/red.
    let line_nos: Vec<_> = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| s.content.as_ref() == "12")
        .collect();
    assert_eq!(line_nos.len(), 2, "expected two line-number spans");
    assert!(
        line_nos
            .iter()
            .any(|s| s.style.fg == Some(palette.diff_add))
    );
    assert!(
        line_nos
            .iter()
            .any(|s| s.style.fg == Some(palette.diff_remove))
    );
}

#[test]
fn parse_grep_hit_match_and_context() {
    let m = parse_grep_hit("crates/tui/src/ui/chat.rs:901:enum ToolOutHint {").unwrap();
    assert_eq!(m.path, "crates/tui/src/ui/chat.rs");
    assert_eq!(m.lineno, "901");
    assert!(m.is_match);
    assert_eq!(m.content, "enum ToolOutHint {");

    let c = parse_grep_hit("src/foo.rs:12-// context line").unwrap();
    assert!(!c.is_match);
    assert_eq!(c.lineno, "12");
    assert_eq!(c.content, "// context line");
}

#[test]
fn split_read_line_accepts_pipe_and_arrow() {
    let (g, code) = split_read_line("    12|fn main()").unwrap();
    assert_eq!(g, "    12|");
    assert_eq!(code, "fn main()");
    let (g2, code2) = split_read_line("     1→use foo;").unwrap();
    assert_eq!(g2, "     1|");
    assert_eq!(code2, "use foo;");
}

#[test]
fn tool_summary_grep_prefers_pattern() {
    let s = tool_summary(
        "grep",
        &json!({"pattern": "ToolOutHint", "path": "crates/tui"}),
    );
    assert!(s.starts_with("ToolOutHint"), "got {s}");
    assert!(s.contains("crates/tui"), "got {s}");
}

#[test]
fn tool_summary_read_is_path() {
    let s = tool_summary("read", &json!({"path": "src/main.rs", "offset": 1}));
    assert_eq!(s, "src/main.rs");
}

#[test]
fn ellipsize_bytes_backs_up_from_mid_codepoint() {
    // 55 ASCII + `ö` (bytes 55..57) — `&s[..56]` panics.
    let s = format!("{}ö{}", "a".repeat(55), "b".repeat(10));
    assert!(!s.is_char_boundary(56));
    let out = ellipsize_bytes(&s, 56);
    assert!(out.ends_with('…'));
    assert_eq!(out.trim_end_matches('…'), "a".repeat(55));
}

#[test]
fn tool_summary_json_fallback_does_not_panic_on_utf8() {
    // Unknown tool, no known string fields → JSON dump, then 56-byte cap.
    // `{"n":"` is 6 bytes + 49 ASCII = 55, then `ö` straddles offset 56.
    let s = tool_summary(
        "custom",
        &json!({"n": format!("{}ö{}", "a".repeat(49), "b".repeat(20))}),
    );
    assert!(s.ends_with('…'));
    assert!(s.is_char_boundary(s.len()));
}

#[test]
fn tool_result_grep_groups_by_path_and_highlights() {
    let palette = ThemeName::DefaultDark.palette();
    let body = "\
crates/tui/src/ui/chat.rs:901:enum ToolOutHint {
crates/tui/src/ui/chat.rs:910:fn tool_out_hint()
crates/tools/src/file/grep.rs:34:        \"grep\"

(3 matches in 2 files; pattern `ToolOutHint`)";
    let lines = tool_result(
        body,
        false,
        &palette,
        false,
        ToolOutHint::Grep {
            pattern: "ToolOutHint".into(),
        },
        100,
    );
    // Path headers should appear (info-coloured).
    let path_spans: Vec<_> = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| {
            s.content.as_ref().contains("chat.rs") || s.content.as_ref().contains("grep.rs")
        })
        .collect();
    assert!(
        path_spans.iter().any(|s| s.style.fg == Some(palette.info)),
        "expected path headers in info colour"
    );
    // Match text highlighted.
    let hit = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "ToolOutHint")
        .expect("highlighted match");
    assert_eq!(hit.style.fg, Some(palette.highlight));
    // Line numbers present.
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.content.as_ref().contains('9') && s.style.fg == Some(palette.dim)),
        "expected dim line numbers"
    );
}

#[test]
fn tool_result_read_skips_path_banner_keeps_line_nos() {
    let palette = ThemeName::DefaultDark.palette();
    let body = "\
# crates/tui/src/ui/chat.rs
# lines 1–3 of 3  |  120 B
     1|fn main() {
     2|    println!(\"hi\");
     3|}
";
    let lines = tool_result(
        body,
        false,
        &palette,
        false,
        ToolOutHint::Code(Some("rust".into())),
        100,
    );
    // Path banner must not reappear as a body row.
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .join("");
    assert!(
        !joined.contains("# crates/tui"),
        "path banner should be stripped, got:\n{joined}"
    );
    // Line-number gutters still painted.
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.content.as_ref().contains("1|") || s.content.as_ref() == "     1|"),
        "expected line-number gutter"
    );
}

#[test]
fn tool_block_grep_header_shows_match_chip() {
    let palette = ThemeName::DefaultDark.palette();
    let body = "src/a.rs:1:foo\nsrc/b.rs:2:foo\n\n(2 matches in 2 files; pattern `foo`)";
    let lines = tool_block(
        "grep",
        &json!({"pattern": "foo"}),
        Some(body),
        ToolPaint {
            is_error: false,
            palette: &palette,
            expanded: false,
            width: 100,
            spin: 0,
        },
    );
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(header.contains('•'), "Grok bullet, got {header}");
    assert!(header.contains("Searched"), "got {header}");
    assert!(header.contains("foo"), "got {header}");
}

#[test]
fn tool_display_name_maps_bash_to_run() {
    assert_eq!(tool_display_name("bash"), "run");
    assert_eq!(tool_display_name("shell"), "run");
    assert_eq!(tool_display_name("read"), "read");
    assert_eq!(tool_display_name("grep"), "grep");
}

#[test]
fn tool_block_run_shows_display_name_and_command() {
    let palette = ThemeName::DefaultDark.palette();
    let lines = tool_block(
        "bash",
        &json!({"command": "cargo test -p whycodes-tui"}),
        Some("ok\n"),
        ToolPaint {
            is_error: false,
            palette: &palette,
            expanded: false,
            width: 100,
            spin: 0,
        },
    );
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !header.contains('┃'),
        "collapsed Run has no accent rail, got {header}"
    );
    assert!(header.contains('•'), "Grok bullet, got {header}");
    assert!(header.contains("Run"), "got {header}");
    assert!(!header.contains("bash"), "got {header}");
    assert!(header.contains("cargo test"), "got {header}");
}

#[test]
fn collapsed_tool_is_header_only() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let body = (0..20)
        .map(|i| format!("line {i} of noisy tool output"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = tool_block(
        "bash",
        &json!({"command": "cargo test"}),
        Some(&body),
        ToolPaint {
            is_error: false,
            palette: &palette,
            expanded: false,
            width: 80,
            spin: 0,
        },
    );
    assert_eq!(
        lines.len(),
        1,
        "Grok collapsed tool is a one-liner: {lines:?}"
    );
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(header.contains('•'), "{header}");
    assert!(
        !header.contains("line 5"),
        "body must not leak into the header: {header}"
    );
    assert!(
        !header.contains('›') && !header.contains('>'),
        "collapsed tool must not trail a chevron: {header}"
    );
}

#[test]
fn collapsed_tools_group_into_one_verb_line() {
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "look");
    app.add_message(ChatRole::Assistant, "");
    app.add_tool_call("t1".into(), "grep".into(), json!({"pattern": "a"}));
    app.add_tool_result("t1", "(1 match)", false);
    app.add_tool_call("t2".into(), "grep".into(), json!({"pattern": "b"}));
    app.add_tool_result("t2", "(1 match)", false);
    app.add_tool_call("t3".into(), "grep".into(), json!({"pattern": "c"}));
    app.add_tool_result("t3", "(1 match)", false);
    app.add_tool_call("t4".into(), "list".into(), json!({"path": "."}));
    app.add_tool_result("t4", "ok", false);
    app.add_tool_call("t5".into(), "read".into(), json!({"path": "CLAUDE.md"}));
    app.add_tool_result("t5", "ok", false);
    let palette = app.config.palette();
    let lines = super::render_message(&app.messages[1], &app, &palette, 1, 80, None, false);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    let joined = texts.join("\n");
    assert!(
        joined.contains("Searched 3 patterns, Listed 1 dir, Read 1 file"),
        "Grok verb-group line, got {texts:?}"
    );
}

#[test]
fn execute_expanded_keeps_head_and_tail() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let body = (0..10)
        .map(|i| format!("exec-line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = tool_block(
        "bash",
        &json!({"command": "seq"}),
        Some(&body),
        ToolPaint {
            is_error: false,
            palette: &palette,
            expanded: true,
            width: 80,
            spin: 0,
        },
    );
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("exec-line-0"), "{text}");
    assert!(text.contains("exec-line-1"), "{text}");
    assert!(text.contains("exec-line-9"), "{text}");
    assert!(text.contains('…'), "{text}");
    assert!(
        !text.contains("exec-line-4"),
        "middle dump must stay hidden: {text}"
    );
}

#[test]
fn last_scrolled_past_user_picks_the_latest_above_the_view() {
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "first");
    app.add_message(ChatRole::Assistant, "a");
    app.add_message(ChatRole::User, "second");
    app.add_message(ChatRole::Assistant, "b");
    let starts = vec![0, 5, 10, 20];
    assert_eq!(super::last_scrolled_past_user(&app, &starts, 12), Some(2));
    assert_eq!(super::last_scrolled_past_user(&app, &starts, 5), Some(0));
    assert_eq!(super::last_scrolled_past_user(&app, &starts, 0), None);
}

#[test]
fn thinking_body_uses_palette_dim_without_sgr_dim() {
    use crate::app::ThinkingBlock;
    use crate::theme::ThemeName;
    use ratatui::style::Modifier;
    let palette = ThemeName::DefaultDark.palette();
    let mut t = ThinkingBlock::new("reason about the patch");
    t.collapsed = false;
    let lines = super::thinking_lines(&t, &palette, 40, 0);
    let body = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("reason")))
        .expect("body line");
    let span = body
        .spans
        .iter()
        .find(|s| s.content.contains("reason"))
        .expect("body span");
    assert_eq!(span.style.fg, Some(palette.dim));
    assert!(
        !span.style.add_modifier.contains(Modifier::DIM),
        "SGR DIM leaks green on Apple Terminal.app"
    );
    let rail = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains('┃') || s.content.contains("┃"))
        .and_then(|s| s.style.fg);
    assert_eq!(rail, Some(palette.thinking));
    assert_ne!(rail, Some(palette.success));
}

#[test]
fn finished_thinking_rail_stays_dim_not_success() {
    use crate::app::ThinkingBlock;
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let t = ThinkingBlock::finished("done thinking");
    let lines = super::thinking_lines(&t, &palette, 40, 0);
    let rail = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains('┃') || s.content.as_ref() == "┃")
        .and_then(|s| s.style.fg);
    assert_eq!(rail, Some(palette.dim));
    assert_ne!(rail, Some(palette.success));
}

#[test]
fn live_thinking_rail_moves_with_the_spinner() {
    use crate::app::ThinkingBlock;
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let t = ThinkingBlock::new("one\ntwo\nthree\nfour");
    let a = super::thinking_lines(&t, &palette, 40, 0);
    let b = super::thinking_lines(&t, &palette, 40, 3);
    let rail_fg = |lines: &[ratatui::text::Line]| {
        lines
            .iter()
            .filter_map(|l| {
                l.spans
                    .iter()
                    .find(|s| s.content.as_ref() == "┃")
                    .and_then(|s| s.style.fg)
            })
            .collect::<Vec<_>>()
    };
    assert_ne!(
        rail_fg(&a),
        rail_fg(&b),
        "live thinking rail must wave across frames"
    );
}

#[test]
fn running_execute_rail_pulses_and_uses_success() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let paint = |spin| ToolPaint {
        is_error: false,
        palette: &palette,
        expanded: true,
        width: 60,
        spin,
    };
    let a = tool_block("bash", &json!({"command": "sleep 1"}), None, paint(0));
    let b = tool_block("bash", &json!({"command": "sleep 1"}), None, paint(2));
    let rail = |lines: &[ratatui::text::Line]| {
        lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "┃")
            .and_then(|s| s.style.fg)
    };
    let fa = rail(&a).expect("running Run paints a rail");
    let fb = rail(&b).expect("running Run paints a rail");
    assert!(
        fa == palette.success || fa == palette.dim,
        "run rail is success or dim rest, got {fa:?}"
    );
    assert_ne!(fa, fb, "running Run rail must blink across frames");
}

#[test]
fn failed_execute_rail_is_error_red() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let lines = tool_block(
        "bash",
        &json!({"command": "false"}),
        Some("exit 1"),
        ToolPaint {
            is_error: true,
            palette: &palette,
            expanded: true,
            width: 60,
            spin: 0,
        },
    );
    let fg = lines[0]
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "┃")
        .and_then(|s| s.style.fg);
    assert_eq!(fg, Some(palette.error), "failed Run rail is error red");
}

#[test]
fn sparse_lines_full_width_diff_band() {
    // Grok paints the whole row green/red — not only the text width.
    let area = Rect::new(0, 0, 20, 1);
    let mut buf = Buffer::empty(area);
    let add_bg = Color::Rgb(20, 80, 40);
    SparseLines {
        lines: vec![Line::from(vec![
            Span::styled(
                "+new".to_string(),
                Style::default().fg(Color::Green).bg(add_bg),
            ),
            Span::styled(" ".to_string(), Style::default().bg(add_bg)),
        ])],
        bg: Color::Black,
    }
    .render(area, &mut buf);

    // Far-right cell (beyond text) must share the band bg.
    let cell = buf.cell((19, 0)).expect("right edge");
    assert_eq!(
        cell.style().bg,
        Some(add_bg),
        "diff wash must fill full row width"
    );
    // Left cell too.
    let left = buf.cell((0, 0)).expect("left edge");
    assert_eq!(left.style().bg, Some(add_bg));
}

#[test]
fn sparse_lines_clears_previous_glyphs_before_paint() {
    let area = Rect::new(0, 0, 12, 3);
    let mut buf = Buffer::empty(area);
    for y in 0..3 {
        buf.set_stringn(0, y, "OLDCONTENT!!", 12, Style::default());
    }

    SparseLines {
        lines: vec![
            Line::from(Span::raw("new")),
            Line::from(""),
            Line::from(Span::raw("ok")),
        ],
        bg: Color::Black,
    }
    .render(area, &mut buf);

    assert_eq!(buf.cell((0, 0)).map(|c| c.symbol()), Some("n"));
    assert_eq!(buf.cell((3, 0)).map(|c| c.symbol()), Some(" "));
    assert_eq!(buf.cell((0, 1)).map(|c| c.symbol()), Some(" "));
    assert_eq!(buf.cell((5, 1)).map(|c| c.symbol()), Some(" "));
    assert_eq!(buf.cell((0, 2)).map(|c| c.symbol()), Some("o"));
}

#[test]
fn selected_concat_slice_marks_first_content_across_halves() {
    let area = Rect::new(0, 0, 8, 2);
    let mut buf = Buffer::empty(area);
    let row = super::ChatRowPaint {
        x: 0,
        width: area.width,
        bg: Color::Black,
        caret_style: Style::default().fg(Color::Yellow),
    };
    let prefix = [Line::from(""), Line::from(Span::raw("prefix"))];
    let body = [Line::from(Span::raw("body"))];

    let next = super::paint_concat_slices(&mut buf, 0, &row, &prefix, &body, 1..3, true);

    assert_eq!(next, 2);
    assert_eq!(buf.cell((0, 0)).map(|c| c.symbol()), Some("▌"));
    assert_eq!(buf.cell((1, 0)).map(|c| c.symbol()), Some("p"));
    assert_eq!(buf.cell((0, 1)).map(|c| c.symbol()), Some("b"));
    assert_eq!(
        super::paint_concat_slices(&mut buf, next, &row, &prefix, &body, 3..3, true),
        next
    );
}

#[test]
fn width_helpers_respect_unicode_and_tiny_budgets() {
    use unicode_width::UnicodeWidthStr;

    assert_eq!(super::truncate_home_title("session", 0), "");
    assert_eq!(super::truncate_home_title("session", 1), "…");
    assert_eq!(super::truncate_home_title("界面 title", 5), "界面…");
    assert_eq!(
        UnicodeWidthStr::width(super::cut_to_width("a界b", 3).as_str()),
        3
    );

    let style = Style::default().fg(Color::Cyan);
    let mut spans = vec![Span::styled("a界".to_string(), style), Span::raw("tail")];
    super::truncate_spans_to(&mut spans, 2);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "a");
    assert_eq!(spans[0].style, style);
}

#[test]
fn image_only_prompt_renders_one_chip_between_band_padding() {
    let palette = ThemeName::DefaultDark.palette();
    let labels = vec!["diagram.png".to_string()];
    let lines = super::user_prompt_lines(
        "[Image: diagram.png]",
        &labels,
        Some("2:32 PM"),
        &palette,
        40,
        false,
        false,
    );

    assert_eq!(lines.len(), 3);
    assert_eq!(line_text(&lines[0]), " ");
    assert_eq!(line_text(&lines[2]), " ");
    let chip = line_text(&lines[1]);
    assert!(chip.contains("🖼 diagram.png"), "{chip:?}");
    assert!(chip.contains("2:32 PM"), "{chip:?}");
    assert!(!chip.contains("[Image:"), "{chip:?}");
}

#[test]
fn visible_message_range_is_binary_search() {
    // starts = prefix sums: msg0@0 h=3, msg1@3 h=5, msg2@8 h=2, total=10
    let starts = [0usize, 3, 8];
    let total = 10;
    assert_eq!(visible_message_range(&starts, total, 0, 3), 0..1);
    assert_eq!(visible_message_range(&starts, total, 2, 4), 0..2);
    assert_eq!(visible_message_range(&starts, total, 3, 8), 1..2);
    assert_eq!(visible_message_range(&starts, total, 7, 10), 1..3);
    assert_eq!(visible_message_range(&starts, total, 8, 10), 2..3);
    assert!(visible_message_range(&starts, total, 10, 12).is_empty());
    assert!(visible_message_range(&[], 0, 0, 5).is_empty());
}

#[test]
fn layout_height_cache_hits_on_second_pass() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello");
    app.add_message(ChatRole::Assistant, "world");
    let width = 80u16;
    let (starts1, total1) = message_row_layout_mut(&mut app, width);
    assert!(app.messages.iter().all(|m| m.layout_cache.is_some()));
    let (starts2, total2) = message_row_layout_mut(&mut app, width);
    assert_eq!(starts1, starts2);
    assert_eq!(total1, total2);
    assert!(
        app.messages
            .iter()
            .all(|m| matches!(m.layout_cache, Some((w, _, _)) if w == width))
    );
    app.append_to_last(" more");
    assert!(app.messages.last().unwrap().layout_cache.is_none());
}

#[test]
fn closed_message_cache_survives_agent_busy_flip() {
    use crate::app::AgentState;
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello");
    app.add_message(ChatRole::Assistant, "# hi\n\n```rs\nfn main() {}\n```");
    let width = 80u16;
    let _ = message_row_layout_mut(&mut app, width);
    assert!(
        app.messages[0].line_cache.is_some(),
        "user bubble must cache after first layout"
    );
    let user_lines = app.messages[0].line_cache.as_ref().unwrap().2.len();
    assert!(
        app.messages[1].line_cache.is_some(),
        "idle assistant must cache before the busy flip"
    );

    app.current_agent_state = AgentState::Generating;
    let _ = message_row_layout_mut(&mut app, width);
    assert!(
        app.messages[0].line_cache.is_some(),
        "finished user bubble must keep its cache when a turn starts"
    );
    assert_eq!(
        app.messages[0].line_cache.as_ref().unwrap().2.len(),
        user_lines
    );
}

#[test]
fn user_prompt_puts_clock_on_the_first_content_line() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let lines = super::user_prompt_lines("hello", &[], Some("2:32 PM"), &palette, 40, false, false);
    let first = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains('\u{276F}')))
        .expect("prompt row");
    let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("hello"), "got {text:?}");
    assert!(
        text.contains("2:32 PM"),
        "clock must sit on the ❯ row, got {text:?}"
    );
    let hello_at = text.find("hello").unwrap();
    let clock_at = text.find("2:32 PM").unwrap();
    assert!(
        clock_at > hello_at,
        "clock must be to the right of the text"
    );
}

#[test]
fn assistant_answer_renders_after_tools() {
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "look");
    app.add_message(ChatRole::Assistant, "AFTER_TOOLS_ANSWER");
    app.add_tool_call(
        "t1".into(),
        "read".into(),
        serde_json::json!({ "path": "a.rs" }),
    );
    app.add_tool_result("t1", "ok", false);
    let palette = app.config.palette();
    let lines = super::render_message(&app.messages[1], &app, &palette, 1, 80, None, false);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    let tool_at = texts
        .iter()
        .position(|t| t.contains('•') || t.contains("Read"));
    let answer_at = texts.iter().position(|t| t.contains("AFTER_TOOLS_ANSWER"));
    assert!(tool_at.is_some(), "tool card missing: {texts:?}");
    assert!(answer_at.is_some(), "answer missing: {texts:?}");
    assert!(
        tool_at.unwrap() < answer_at.unwrap(),
        "answer must sit below tools, got {texts:?}"
    );
}

#[test]
fn assistant_reply_puts_clock_on_the_first_content_line() {
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "hello from the agent");
    app.messages[1].duration_ms = Some(1200);
    let palette = app.config.palette();
    let lines = super::render_message(&app.messages[1], &app, &palette, 1, 80, None, false);
    let answer = lines
        .iter()
        .find(|l| {
            l.spans
                .iter()
                .any(|s| s.content.contains("hello from the agent"))
        })
        .expect("assistant body");
    let text: String = answer.spans.iter().map(|s| s.content.as_ref()).collect();
    let hello_at = text.find("hello").expect("answer text");
    let colon = text.rfind(':').expect("h:mm AM/PM clock on the reply");
    assert!(
        colon > hello_at,
        "Grok puts the clock on the right of the agent line, got {text:?}"
    );
    assert!(
        text.contains("AM") || text.contains("PM"),
        "Grok bubble clock is 12-hour, got {text:?}"
    );
    let footer = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("Worked for")))
        .expect("turn footer");
    let foot: String = footer.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        foot.contains(':'),
        "Worked for line also carries the clock, got {foot:?}"
    );
}

#[test]
fn live_thinking_puts_elapsed_on_the_right() {
    use crate::app::ThinkingBlock;
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let t = ThinkingBlock::new("one\ntwo\nthree");
    let lines = super::thinking_lines(&t, &palette, 60, 0);
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        header.contains("Thinking..."),
        "Grok live header is Thinking..., got {header:?}"
    );
    assert!(
        !header.contains('·'),
        "elapsed sits on the right, not after a mid-dot: {header:?}"
    );
    assert!(
        header.contains('s') || header.contains("Thinking..."),
        "got {header:?}"
    );
}

#[test]
fn collapsed_thinking_has_no_trailing_chevron() {
    use crate::app::ThinkingBlock;
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let mut t = ThinkingBlock::new("secret plan");
    t.finish();
    t.collapsed = true;
    let lines = super::thinking_lines(&t, &palette, 60, 0);
    let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(header.contains("Thought"), "got {header:?}");
    assert!(
        header.contains('┃'),
        "thinking keeps a left accent, got {header:?}"
    );
    assert!(
        !header.contains('›') && !header.contains('>'),
        "folded rows must not trail a chevron, got {header:?}"
    );
    assert!(
        !header.contains("(e expand)"),
        "legacy hint must not remain, got {header:?}"
    );
}

#[test]
fn first_user_bubble_keeps_clock_on_the_right() {
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello from history");
    let palette = app.config.palette();
    let lines = super::render_message(&app.messages[0], &app, &palette, 0, 80, None, false);
    let row = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains('\u{276F}')))
        .expect("first bubble ❯ row");
    let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("hello from history"), "got {text:?}");
    let hello_at = text.find("hello").expect("prompt text");
    let colon = text.rfind(':').expect("h:mm AM/PM clock");
    assert!(
        colon > hello_at,
        "clock must sit to the right, got {text:?}"
    );
    assert!(
        text.contains("AM") || text.contains("PM"),
        "Grok bubble clock is 12-hour, got {text:?}"
    );
}

#[test]
fn long_first_prompt_still_keeps_the_clock() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let prompt = "please look at the first message bubble in chat history \
and tell me if the clock should be on the right side of it like grok does \
with a short 12-hour stamp";
    let lines = super::user_prompt_lines(prompt, &[], Some("2:32 PM"), &palette, 72, false, false);
    let first = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains('\u{276F}')))
        .expect("prompt row");
    let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("2:32 PM"),
        "long first bubble must keep the clock, got {text:?}"
    );
}

#[test]
fn right_align_keeps_clock_at_the_row_end() {
    let line = super::line_with_right(
        vec![Span::raw("❯ hi".to_string())],
        Some("14:32"),
        Style::default(),
        20,
    );
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    // Last column is a gutter (scrollbar); clock sits just left of it.
    assert_eq!(unicode_width::UnicodeWidthStr::width(text.as_str()), 19);
    assert!(text.ends_with("14:32"), "got {text:?}");
}

#[test]
fn right_align_never_drops_the_clock() {
    let line = super::line_with_right(
        vec![Span::raw(format!("❯ {}", "x".repeat(40)))],
        Some("14:32"),
        Style::default(),
        20,
    );
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("14:32"),
        "clock must survive an oversized left side, got {text:?}"
    );
    assert!(
        unicode_width::UnicodeWidthStr::width(text.as_str()) <= 20,
        "row must still fit, got {text:?}"
    );
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn long_user_prompt_collapses_to_three_lines() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let prompt = "one\ntwo\nthree\nfour\nfive";
    let folded = super::user_prompt_lines(prompt, &[], None, &palette, 40, false, false);
    let texts: Vec<String> = folded.iter().map(line_text).collect();
    let content: Vec<&String> = texts.iter().filter(|t| !t.trim().is_empty()).collect();
    assert_eq!(content.len(), 3, "Grok folds to 3 content rows: {texts:?}");
    assert!(
        content.last().is_some_and(|t| t.contains('\u{2026}')),
        "collapsed last row carries …, got {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("four") || t.contains("five")),
        "hidden tail must not paint: {texts:?}"
    );

    let open = super::user_prompt_lines(prompt, &[], None, &palette, 40, false, true);
    let open_text: Vec<String> = open.iter().map(line_text).collect();
    assert!(
        open_text.iter().any(|t| t.contains("four")),
        "expanded shows the tail: {open_text:?}"
    );
    assert!(
        !open_text.iter().any(|t| t.contains('\u{2026}')),
        "expanded has no ellipsis: {open_text:?}"
    );
}

#[test]
fn slash_command_token_uses_accent() {
    use crate::theme::ThemeName;
    let palette = ThemeName::DefaultDark.palette();
    let lines = super::user_prompt_lines("/help please", &[], None, &palette, 40, false, false);
    let row = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("/help")))
        .expect("prompt row");
    let token = row
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "/help")
        .expect("/help span");
    assert_eq!(
        token.style.fg,
        Some(palette.accent),
        "Grok paints /command in the skill accent"
    );
    let rest = row
        .spans
        .iter()
        .find(|s| s.content.contains("please"))
        .expect("args span");
    assert_eq!(rest.style.fg, Some(palette.fg));
}

#[test]
fn home_recents_and_layout_cache_hits() {
    use crate::app::{ChatRole, SessionEntry, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn paint_chat(app: &mut TuiApp, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("term");
        terminal
            .draw(|f| super::render(f, f.area(), app, &app.config.palette()))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let area = buf.area();
        let mut out = String::new();
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.session_list.sessions = vec![SessionEntry {
        id: "s1".into(),
        title: "recent session title".into(),
        messages: 2,
        updated_at: Some(chrono::Utc::now()),
        live: None,
    }];
    let home = paint_chat(&mut app, 80, 24);
    assert!(
        home.contains("recent") || home.contains("resume") || home.contains("session"),
        "{home:?}"
    );

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello cache");
    app.add_message(ChatRole::Assistant, "world cache");
    let width = 80u16;
    let (starts1, total1) = super::message_row_layout(&app, width);
    let (starts2, total2) = super::message_row_layout(&app, width);
    assert_eq!(starts1, starts2);
    assert_eq!(total1, total2);
    let _ = super::session_line_count(&app, width);

    let _ = super::message_row_layout_mut(&mut app, width);
    let _ = super::session_line_count_mut(&mut app, width);
    let session = paint_chat(&mut app, 80, 16);
    assert!(
        session.contains("hello") || session.contains("world") || session.contains('\u{276F}'),
        "{session:?}"
    );
}

#[test]
fn visible_range_clamps_bottom_anchored_scroll() {
    assert_eq!(super::visible_range(0, 10, 0), (0, 0));
    assert_eq!(super::visible_range(10, 0, 0), (0, 0));
    assert_eq!(super::visible_range(100, 20, 0), (80, 100));
    assert_eq!(super::visible_range(100, 20, 30), (50, 70));
    assert_eq!(super::visible_range(10, 20, usize::MAX), (0, 10));
}

#[test]
fn json_and_grep_detection_reject_malformed_inputs() {
    assert_eq!(super::prettify_tool_result(""), "");
    assert_eq!(super::prettify_tool_result("not json"), "not json");
    assert!(super::find_json_value_start("prefix {\"ok\":true}").is_some());
    assert!(super::find_json_value_start("prefix {bad").is_none());
    assert!(super::looks_like_json_body("[1,2]"));
    assert!(!super::looks_like_json_body("[bad"));
    assert!(super::looks_like_grep_body(
        "src/a.rs:1:hit\nsrc/a.rs:2-context"
    ));
    assert!(!super::looks_like_grep_body("ordinary\ntext"));
    assert!(super::parse_grep_hit(":12:no path").is_none());
    assert!(super::parse_grep_hit("file:no:number").is_none());
    assert!(super::split_read_line("not a numbered row").is_none());
}

#[test]
fn grep_match_count_reads_footer_then_hit_lines() {
    assert_eq!(
        super::grep_match_count("(12 matches in 3 files; pattern `foo`)"),
        Some(12)
    );
    assert_eq!(
        super::grep_match_count("src/a.rs:1:hit\nsrc/a.rs:2:also"),
        Some(2)
    );
    assert_eq!(super::grep_match_count("ordinary text"), None);
}

#[test]
fn try_pretty_json_rewrites_minified_and_skips_already_pretty() {
    let pretty = super::try_pretty_json(r#"{"a":1,"b":2}"#).expect("minified");
    assert!(pretty.contains('\n'), "{pretty}");
    assert!(super::try_pretty_json("not json").is_none());
    let already = "{\n  \"a\": 1\n}";
    assert!(super::try_pretty_json(already).is_none());
}

#[test]
fn diff_stat_skips_file_headers() {
    let body = "--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n context\n";
    assert_eq!(super::diff_stat(body), (1, 1));
}

#[test]
fn execute_tool_and_header_verbs_cover_aliases() {
    assert!(super::is_execute_tool("bash"));
    assert!(super::is_execute_tool("run"));
    assert!(!super::is_execute_tool("read"));
    assert_eq!(super::tool_header_verb("read_file", true), "Reading");
    assert_eq!(super::tool_header_verb("read_file", false), "Read");
    assert_eq!(super::tool_header_verb("bash", true), "Running");
    assert_eq!(super::tool_header_verb("bash", false), "Run");
    assert_eq!(super::tool_header_verb("search_code", true), "Searching");
    assert_eq!(super::tool_header_verb("search_code", false), "Searched");
    assert_eq!(super::tool_header_verb("list_dir", true), "Listing");
    assert_eq!(super::tool_header_verb("list_dir", false), "Listed");
    assert_eq!(super::tool_header_verb("write", true), "Editing");
    assert_eq!(super::tool_header_verb("write", false), "Edited");
    assert_eq!(super::tool_header_verb("web_fetch", true), "Fetching");
    assert_eq!(super::tool_header_verb("web_fetch", false), "Fetched");
    assert_eq!(super::tool_header_verb("web_search", true), "Searching");
    assert_eq!(super::tool_header_verb("web_search", false), "Searched");
    assert_eq!(super::tool_header_verb("custom", true), "Calling");
    assert_eq!(super::tool_header_verb("custom", false), "Custom");
    assert_eq!(super::tool_header_verb("", false), "Called");
    assert_eq!(super::verb_kind("read"), Some(super::VerbKind::File));
    assert_eq!(super::verb_kind("grep"), Some(super::VerbKind::Search));
    assert_eq!(super::verb_kind("list"), Some(super::VerbKind::Dir));
    assert_eq!(
        super::verb_kind("web_search"),
        Some(super::VerbKind::WebSearch)
    );
    assert_eq!(
        super::verb_kind("web_fetch"),
        Some(super::VerbKind::WebFetch)
    );
    assert_eq!(super::verb_kind("memory"), Some(super::VerbKind::Memory));
    assert!(super::verb_kind("bash").is_none());
    assert_eq!(super::VerbKind::File.verb(true), "Reading");
    assert_eq!(super::VerbKind::File.verb(false), "Read");
    assert_eq!(super::VerbKind::File.noun(1), "file");
    assert_eq!(super::VerbKind::File.noun(2), "files");
    assert_eq!(super::VerbKind::Search.noun(2), "patterns");
    assert_eq!(super::VerbKind::Dir.noun(1), "dir");
    assert_eq!(super::VerbKind::WebFetch.noun(2), "websites");
    assert_eq!(super::VerbKind::Memory.noun(1), "memory");
}

#[test]
fn callout_kind_classifies_system_notices() {
    use super::CalloutKind;
    assert_eq!(CalloutKind::from_content("error: boom"), CalloutKind::Error);
    assert_eq!(
        CalloutKind::from_content("cannot call tool"),
        CalloutKind::Error
    );
    assert_eq!(
        CalloutKind::from_content("No API key for acme"),
        CalloutKind::Warning
    );
    assert_eq!(CalloutKind::from_content("✓ ready"), CalloutKind::Success);
    assert_eq!(CalloutKind::from_content("hello"), CalloutKind::Info);
    let p = crate::theme::ThemeName::DefaultDark.palette();
    assert_eq!(CalloutKind::Error.accent(&p), p.error);
    assert_eq!(CalloutKind::Warning.accent(&p), p.warning);
    assert_eq!(CalloutKind::Success.accent(&p), p.success);
    assert_eq!(CalloutKind::Info.accent(&p), p.info);
    assert_eq!(CalloutKind::Error.glyph(), "✕");
    assert_eq!(CalloutKind::Warning.glyph(), "!");
    assert_eq!(CalloutKind::Success.glyph(), "✓");
    assert_eq!(CalloutKind::Info.glyph(), "i");
    assert_eq!(CalloutKind::Error.label(), "Error");
    assert_eq!(CalloutKind::Warning.label(), "Setup");
    assert_eq!(CalloutKind::Success.label(), "Ready");
    assert_eq!(CalloutKind::Info.label(), "Note");
}

#[test]
fn tool_result_auto_picks_grep_code_and_plain() {
    let palette = ThemeName::DefaultDark.palette();
    let grep = tool_result(
        "src/a.rs:1:hit\nsrc/a.rs:2:also\n--\n(2 matches)",
        false,
        &palette,
        false,
        ToolOutHint::Auto,
        80,
    );
    assert!(!grep.is_empty());
    let err = tool_result("boom", true, &palette, false, ToolOutHint::Auto, 80);
    assert!(!err.is_empty());
    let empty = tool_result("", false, &palette, false, ToolOutHint::Auto, 80);
    assert!(empty.is_empty());
    let no_matches = tool_result(
        "No matches found.",
        false,
        &palette,
        false,
        ToolOutHint::Grep {
            pattern: "x".into(),
        },
        80,
    );
    assert!(!no_matches.is_empty());
    let grep_err = tool_result(
        "fatal",
        true,
        &palette,
        false,
        ToolOutHint::Grep {
            pattern: "x".into(),
        },
        40,
    );
    assert!(!grep_err.is_empty());
    let code_err = tool_result(
        "fail",
        true,
        &palette,
        false,
        ToolOutHint::Code(Some("rs".into())),
        40,
    );
    assert!(!code_err.is_empty());
    let many: String = (1..=20)
        .map(|i| format!("src/a.rs:{i}:hit{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let truncated = tool_result(
        &many,
        false,
        &palette,
        false,
        ToolOutHint::Grep {
            pattern: "hit".into(),
        },
        60,
    );
    assert!(!truncated.is_empty());
    let ctx = tool_result(
        "src/a.rs:1-context line\nsrc/a.rs:2:hit pattern\n--\n[footer]",
        false,
        &palette,
        true,
        ToolOutHint::Grep {
            pattern: "pattern".into(),
        },
        80,
    );
    assert!(!ctx.is_empty());
    let json_auto = tool_result(
        r#"{"a":1,"b":2,"c":3}"#,
        false,
        &palette,
        false,
        ToolOutHint::Auto,
        40,
    );
    assert!(!json_auto.is_empty());
    let read = tool_result(
        "# crates/tui/src/ui/chat.rs\n# lines 1–40 of 200  |  4.2 KB\n     1|fn main() {\n     2|    println!(\"hi\");\n}",
        false,
        &palette,
        true,
        ToolOutHint::Code(Some("rust".into())),
        20,
    );
    assert!(!read.is_empty());
    let long_body = format!("{}\n{}", "x".repeat(200), vec!["line"; 20].join("\n"));
    let long_plain = tool_result(&long_body, false, &palette, false, ToolOutHint::Auto, 12);
    assert!(!long_plain.is_empty());
}

#[test]
fn tool_summary_covers_named_and_fallback_fields() {
    assert_eq!(
        tool_summary("grep", &json!({"pattern": "foo", "path": "src"})),
        "foo · src"
    );
    assert_eq!(tool_summary("rg", &json!({"query": "bar"})), "bar");
    assert_eq!(
        tool_summary("search_code", &json!({"path": "only"})),
        "only"
    );
    assert_eq!(tool_summary("read", &json!({"file_path": "a.rs"})), "a.rs");
    assert_eq!(
        tool_summary("write", &json!({"target_file": "b.rs"})),
        "b.rs"
    );
    assert_eq!(
        tool_summary("bash", &json!({"description": "build"})),
        "build"
    );
    assert_eq!(tool_summary("shell", &json!({"command": "ls"})), "ls");
    assert_eq!(tool_summary("glob", &json!({"glob": "*.rs"})), "*.rs");
    assert_eq!(tool_summary("custom", &json!({"goal": "x"})), "x");
    assert_eq!(tool_summary("custom", &json!({})), "");
    assert_eq!(tool_summary("custom", &json!(null)), "");
    let long = "y".repeat(80);
    let s = tool_summary("custom", &json!({"command": long}));
    assert!(s.ends_with('…'), "{s}");
}

#[test]
fn paint_grep_match_and_literal_cover_edges() {
    let base = Style::default();
    let hit = Style::default().fg(Color::Yellow);
    assert_eq!(super::paint_grep_literal("abc", "", base, hit).len(), 1);
    let lit = super::paint_grep_literal("xxfooyyfoo", "foo", base, hit);
    assert!(lit.len() >= 3);
    let none = super::paint_grep_match("abc", None, base, hit);
    assert_eq!(none.len(), 1);
    let re = super::compile_grep_highlighter("foo").expect("re");
    let spans = super::paint_grep_match("xxfooyy", Some(&re), base, hit);
    assert!(spans.len() >= 2);
    assert!(super::compile_grep_highlighter("").is_none());
    assert!(super::compile_grep_highlighter("   ").is_none());
}

#[test]
fn looks_like_grep_body_skips_separators() {
    assert!(super::looks_like_grep_body(
        "\n--\n(1 match)\n[info]\nsrc/a.rs:1:hit\nsrc/a.rs:2:hit2"
    ));
    let prefix = super::prettify_tool_result("noise {\"k\":1}");
    assert!(prefix.contains("k") || prefix.contains("noise"));
}

#[test]
fn center_line_pads_and_bolds() {
    let line = super::center_line("hi", 10, Color::White, true);
    assert!(!line.spans.is_empty());
    let empty = super::center_line("", 4, Color::White, false);
    assert!(!empty.spans.is_empty() || empty.width() == 0);
    assert_eq!(super::empty_dash(""), "—");
    assert_eq!(super::empty_dash("x"), "x");
}
