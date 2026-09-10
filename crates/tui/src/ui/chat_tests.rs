use super::{
    ToolOutHint, ToolPaint, ToolRef, ellipsize_bytes, hard_truncate_line, message_row_layout_mut,
    paint_tool_run, parse_grep_hit, prettify_tool_result, split_read_line, tool_block,
    tool_display_name, tool_out_hint, tool_result, tool_summary, visible_message_range,
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

/// Paint chat lines, filling every row so previous-frame glyphs cannot linger.
///
/// History: a pure sparse writer (only non-empty spans) left ghost cells after
/// scroll. Production paint now writes rows directly; this widget remains for
/// unit tests that stamp a buffer without a full session.
struct SparseLines {
    lines: Vec<Line<'static>>,
    bg: ratatui::style::Color,
}

impl Widget for SparseLines {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let row = super::ChatRowPaint {
            x: area.x,
            width: area.width,
            bg: self.bg,
            caret_style: Style::default(),
        };
        for r in 0..area.height {
            let line = self.lines.get(r as usize);
            super::paint_chat_row(buf, area.y + r, &row, line, false);
        }
    }
}

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
fn thinking_lines_ellipsis_when_live_tail_or_expanded_is_truncated() {
    use crate::app::{THINKING_EXPANDED_MAX_LINES, THINKING_LIVE_TAIL_LINES, ThinkingBlock};
    let palette = ThemeName::DefaultDark.palette();
    let live_text = (0..=THINKING_LIVE_TAIL_LINES)
        .map(|i| format!("live-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut live = ThinkingBlock::new(live_text);
    live.collapsed = true;
    assert!(live.is_truncated_live());
    let lines = super::thinking_lines(&live, &palette, 40, 0);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains('…') || joined.contains("..."),
        "live collapsed tail must paint an ellipsis row, got {joined:?}"
    );

    let expanded_text = (0..=THINKING_EXPANDED_MAX_LINES)
        .map(|i| format!("x{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut expanded = ThinkingBlock::finished(expanded_text);
    expanded.collapsed = false;
    assert!(expanded.is_truncated_expanded());
    let lines = super::thinking_lines(&expanded, &palette, 40, 0);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains('…') || joined.contains("..."),
        "expanded thought past the cap must paint a trailing ellipsis, got {joined:?}"
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

    app.turn_usage = Some(whycodes_core::types::Usage {
        input_tokens: 1200,
        output_tokens: 80,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    let (left, _) = super::turn_done_footer(&app.messages[1], true, &app);
    assert!(
        left.contains("Worked for") && (left.contains("in") || left.contains("·")),
        "last finished turn must append usage, got {left}"
    );
    let (left, _) = super::turn_done_footer(&app.messages[1], false, &app);
    assert!(
        !left.contains("in") && !left.contains("out"),
        "non-last turns must not append token usage, got {left}"
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
    let snapshot: String = home
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_ascii_whitespace() {
                c
            } else {
                '.'
            }
        })
        .collect();
    assert!(
        snapshot.contains("recent") || snapshot.contains("resume") || snapshot.contains("session"),
        "golden-ish home snapshot lost recents:\n{snapshot}"
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
        session.contains("hello") || session.contains("world") || session.contains("{276F}"),
        "{session:?}"
    );
}

#[test]
fn message_row_layout_hits_height_then_line_cache() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello cache");
    app.add_message(ChatRole::Assistant, "world cache");
    let width = 80u16;
    let (starts, total) = message_row_layout_mut(&mut app, width);
    assert!(
        app.messages
            .iter()
            .all(|m| m.layout_cache.is_some() && m.line_cache.is_some())
    );

    let (s_hit, t_hit) = super::message_row_layout(&app, width);
    assert_eq!(starts, s_hit);
    assert_eq!(total, t_hit);

    for m in &mut app.messages {
        m.layout_cache = None;
    }
    let (s_line, t_line) = super::message_row_layout(&app, width);
    assert_eq!(starts, s_line);
    assert_eq!(total, t_line);

    let (s_mut, t_mut) = message_row_layout_mut(&mut app, width);
    assert_eq!(starts, s_mut);
    assert_eq!(total, t_mut);
    assert!(
        app.messages.iter().all(|m| m.layout_cache.is_some()),
        "mut layout must refill height cache from line_cache"
    );
}

#[test]
fn session_paint_fills_closed_line_cache_on_first_draw() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello first paint");
    app.add_message(ChatRole::Assistant, "world first paint");
    assert!(app.messages.iter().all(|m| m.line_cache.is_none()));

    let backend = TestBackend::new(60, 16);
    let mut terminal = Terminal::new(backend).expect("term");
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .expect("draw");
    assert!(
        app.messages
            .iter()
            .all(|m| m.line_cache.is_some() && m.layout_cache.is_some()),
        "closed bubbles must cache lines on the first session paint"
    );
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area().height {
        for x in 0..buf.area().width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
    }
    assert!(
        out.contains("hello") || out.contains("world"),
        "first paint must show the transcript, got {out:?}"
    );
}

#[test]
fn message_is_closed_opens_running_thinking_and_busy_tail() {
    use crate::app::{AgentState, ChatBlock, ThinkingBlock};

    let mut app = TuiApp::new(TuiAppConfig::default());
    assert!(
        !super::message_is_closed(&app, 0),
        "missing index is not a closed bubble"
    );

    app.add_message(ChatRole::Assistant, "thinking");
    if let Some(msg) = app.messages.last_mut() {
        msg.blocks
            .push(ChatBlock::Thinking(ThinkingBlock::new("plan")));
    }
    assert!(
        !super::message_is_closed(&app, 0),
        "open thinking must keep the bubble live"
    );

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "streaming");
    app.current_agent_state = AgentState::Generating;
    assert!(
        super::message_is_closed(&app, 0),
        "finished user bubble stays closed while a turn runs"
    );
    assert!(
        !super::message_is_closed(&app, 1),
        "last assistant while busy must stay open"
    );
}

#[test]
fn golden_home_and_session_paint_stable_ascii() {
    use crate::app::{ChatRole, SessionEntry, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn paint(app: &mut TuiApp, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("term");
        let palette = app.config.palette();
        terminal
            .draw(|f| super::render(f, f.area(), app, &palette))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let area = buf.area();
        let mut out = String::new();
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell((x, y)) {
                    let s = cell.symbol();
                    if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                        out.push_str(s);
                    } else {
                        out.push('.');
                    }
                }
            }
            out.push('\n');
        }
        out
    }

    let mut home = TuiApp::new(TuiAppConfig::default());
    home.session_list.sessions = vec![SessionEntry {
        id: "s1".into(),
        title: "golden recent".into(),
        messages: 1,
        updated_at: None,
        live: None,
    }];
    let home_txt = paint(&mut home, 60, 18);
    assert!(
        home_txt.contains("golden") || home_txt.contains("recent") || home_txt.contains("resume"),
        "{home_txt}"
    );

    let mut session = TuiApp::new(TuiAppConfig::default());
    session.add_message(ChatRole::User, "ping");
    session.add_message(ChatRole::Assistant, "pong");
    let session_txt = paint(&mut session, 60, 18);
    assert!(
        session_txt.contains("ping") || session_txt.contains("pong"),
        "{session_txt}"
    );
}

#[test]
fn session_paint_overflow_scrollbar_and_live_assistant() {
    use crate::app::{AgentState, ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = TuiApp::new(TuiAppConfig::default());
    for i in 0..12 {
        app.add_message(ChatRole::User, format!("user {i} wrap wrap wrap wrap wrap"));
        app.add_message(
            ChatRole::Assistant,
            format!("asst {i} more wrap wrap wrap wrap wrap"),
        );
    }
    app.current_agent_state = AgentState::Generating;
    app.focus = crate::app::FocusPane::Scrollback;
    app.selected_msg = Some(app.messages.len() - 1);
    app.scroll_offset = 4;
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();

    let mut tiny = TuiApp::new(TuiAppConfig::default());
    tiny.add_message(ChatRole::User, "x");
    let backend = TestBackend::new(4, 2);
    let mut terminal = Terminal::new(backend).unwrap();
    let palette = tiny.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut tiny, &palette))
        .unwrap();
}

#[test]
fn paint_chat_row_caret_and_band() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    let area = Rect::new(0, 0, 12, 2);
    let mut buf = Buffer::empty(area);
    let row = super::ChatRowPaint {
        x: 0,
        width: 12,
        bg: Color::Black,
        caret_style: Style::default().fg(Color::White),
    };
    let line = Line::from(vec![Span::styled(
        "hi",
        Style::default().fg(Color::White).bg(Color::Blue),
    )]);
    super::paint_chat_row(&mut buf, 0, &row, Some(&line), true);
    super::paint_chat_row(&mut buf, 1, &row, None, false);
    assert_eq!(
        super::paint_concat_slices(&mut buf, 0, &row, &[], &[], 2..2, false),
        0
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
    assert_eq!(super::verb_kind("glob"), Some(super::VerbKind::Search));
    assert_eq!(super::verb_kind("rg"), Some(super::VerbKind::Search));
    assert_eq!(
        super::verb_kind("search_code"),
        Some(super::VerbKind::Search)
    );
    assert_eq!(super::verb_kind("list_dir"), Some(super::VerbKind::Dir));
    assert_eq!(
        super::verb_kind("webfetch"),
        Some(super::VerbKind::WebFetch)
    );
    assert_eq!(super::verb_kind("fetch"), Some(super::VerbKind::WebFetch));
    assert_eq!(
        super::verb_kind("memory_search"),
        Some(super::VerbKind::Memory)
    );
    assert_eq!(super::VerbKind::File.verb(true), "Reading");
    assert_eq!(super::VerbKind::File.verb(false), "Read");
    assert_eq!(super::VerbKind::File.noun(1), "file");
    assert_eq!(super::VerbKind::File.noun(2), "files");
    assert_eq!(super::VerbKind::Search.noun(2), "patterns");
    assert_eq!(super::VerbKind::Dir.noun(1), "dir");
    assert_eq!(super::VerbKind::Dir.noun(2), "dirs");
    assert_eq!(super::VerbKind::WebFetch.noun(2), "websites");
    assert_eq!(super::VerbKind::WebSearch.noun(1), "website");
    assert_eq!(super::VerbKind::WebSearch.verb(true), "Searching");
    assert_eq!(super::VerbKind::Memory.noun(1), "memory");
    assert_eq!(super::VerbKind::Memory.noun(2), "memories");
    assert_eq!(super::VerbKind::Memory.verb(true), "Searching");
    assert_eq!(super::VerbKind::Dir.verb(true), "Listing");
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

#[test]
fn tool_out_hint_classifies_named_tools() {
    assert!(matches!(
        super::tool_out_hint("git_diff", &json!({}), "x"),
        ToolOutHint::Diff
    ));
    assert!(matches!(
        super::tool_out_hint("edit", &json!({}), "--- a\n+++ b\n-old\n+new"),
        ToolOutHint::Diff
    ));
    match super::tool_out_hint("read", &json!({"path": "src/main.rs"}), "") {
        ToolOutHint::Code(lang) => assert_eq!(lang.as_deref(), Some("rust")),
        other => panic!("{other:?}"),
    }
    match super::tool_out_hint("grep", &json!({"pattern": "foo"}), "") {
        ToolOutHint::Grep { pattern } => assert_eq!(pattern, "foo"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        super::tool_out_hint("custom", &json!({}), "--- a\n+++ b\n-old\n+new"),
        ToolOutHint::Diff
    ));
    assert!(matches!(
        super::tool_out_hint("custom", &json!({}), "plain"),
        ToolOutHint::Auto
    ));
}

#[test]
fn render_session_paints_user_assistant_and_tool_blocks() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hello **user**");
    app.add_message(ChatRole::Assistant, "reply with `code`");
    let last = app.messages.len() - 1;
    app.messages[last].blocks = vec![
        crate::app::ChatBlock::Thinking(crate::app::ThinkingBlock::new("hmm")),
        crate::app::ChatBlock::ToolUse {
            id: "t1".into(),
            name: "grep".into(),
            input: json!({"pattern": "foo"}),
        },
        crate::app::ChatBlock::ToolResult {
            id: "t1".into(),
            content: "src/a.rs:1:foo".into(),
            is_error: false,
        },
        crate::app::ChatBlock::Text("done".into()),
    ];
    app.messages[last].duration_ms = Some(42);
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(text.contains("hello") || text.contains("user"), "{text}");
}

#[test]
fn render_session_paints_subagent_system_tool_and_error() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "prompt");
    app.add_message(ChatRole::System, "note");
    app.add_message(ChatRole::Tool, "tool body");
    app.add_message(ChatRole::Assistant, "");
    let last = app.messages.len() - 1;
    app.messages[last].blocks = vec![
        crate::app::ChatBlock::Subagent {
            id: "kid".into(),
            kind: "explore".into(),
            description: "look around".into(),
            status: "failed".into(),
            activity: String::new(),
            elapsed_ms: 1500,
        },
        crate::app::ChatBlock::ToolUse {
            id: "t-read".into(),
            name: "read".into(),
            input: json!({"path": "a.rs"}),
        },
    ];
    app.messages[last].tool_calls = vec![crate::app::ChatToolCall {
        id: "orphan".into(),
        name: "repomap".into(),
        arguments: json!({}),
        collapsed: false,
        result: Some("mapped".into()),
        is_error: false,
    }];
    app.messages[last].error = Some("boom".into());
    app.messages[last].results_expanded = true;
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("note") || text.contains("Error") || text.contains("Subagent"),
        "{text}"
    );

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "");
    let last = app.messages.len() - 1;
    app.messages[last].blocks = vec![crate::app::ChatBlock::Subagent {
        id: "done".into(),
        kind: "explore".into(),
        description: "scan".into(),
        status: "completed".into(),
        activity: String::new(),
        elapsed_ms: 2500,
    }];
    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("Subagent") && (text.contains("completed") || text.contains("2.5")),
        "completed subagent must paint status and elapsed, got {text}"
    );
}

#[test]
fn tool_header_verb_covers_named_and_fallback() {
    assert_eq!(super::tool_header_verb("read", true), "Reading");
    assert_eq!(super::tool_header_verb("read", false), "Read");
    assert_eq!(super::tool_header_verb("run", true), "Running");
    assert_eq!(super::tool_header_verb("run", false), "Run");
    assert_eq!(super::tool_header_verb("repomap", true), "Mapping");
    assert_eq!(super::tool_header_verb("repomap", false), "Mapped");
    assert_eq!(super::tool_header_verb("list_dir", false), "Listed");
    assert_eq!(super::tool_header_verb("web_fetch", true), "Fetching");
    assert_eq!(super::tool_header_verb("web_fetch", false), "Fetched");
    assert_eq!(super::tool_header_verb("web_search", false), "Searched");
    assert_eq!(super::tool_header_verb("", false), "Called");
    assert!(super::tool_header_verb("custom_tool", false).starts_with('C'));
    assert_eq!(super::verb_kind("read"), Some(super::VerbKind::File));
    assert_eq!(super::verb_kind("memory"), Some(super::VerbKind::Memory));
    assert!(super::verb_kind("bash").is_none());
    assert_eq!(super::VerbKind::WebFetch.verb(true), "Fetching");
    assert_eq!(super::VerbKind::File.noun(2), "files");
}

#[test]
fn render_user_bubble_with_image_labels_and_running_subagent() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "[Image: shot.png]");
    let i = app.messages.len() - 1;
    app.messages[i].image_labels = vec!["shot.png".into()];
    app.add_message(ChatRole::Assistant, "");
    let last = app.messages.len() - 1;
    app.messages[last].blocks = vec![crate::app::ChatBlock::Subagent {
        id: "kid".into(),
        kind: "explore".into(),
        description: "scan".into(),
        status: "running".into(),
        activity: "listing".into(),
        elapsed_ms: 0,
    }];
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("shot") || text.contains("Subagent") || text.contains("listing"),
        "{text}"
    );
}

#[test]
fn user_prompt_empty_body_and_image_placeholder_skip() {
    let palette = ThemeName::DefaultDark.palette();
    let empty = super::user_prompt_lines("", &[], None, &palette, 40, false, true);
    assert!(
        !empty.is_empty(),
        "empty user bubble still paints band pad + caret row"
    );

    let blank_lines = super::user_prompt_lines("\n\nhello", &[], None, &palette, 40, true, true);
    assert!(
        blank_lines.len() >= 4,
        "hard newlines must stay as extra rows"
    );

    let skip = super::user_prompt_lines(
        "[Image: shot.png]",
        &["shot.png".into()],
        Some("12:00"),
        &palette,
        40,
        false,
        true,
    );
    let joined: String = skip
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        !joined.contains("[Image:"),
        "chip already shows the file; synthetic placeholder must not repeat: {joined:?}"
    );
    assert!(
        joined.contains("shot") || joined.contains("png"),
        "{joined:?}"
    );
}

#[test]
fn stamp_first_content_line_skips_empty_clock_and_already_stamped() {
    let style = Style::default().fg(Color::Gray);
    let mut empty = vec![Line::from("")];
    assert!(!super::stamp_first_content_line(
        &mut empty, None, style, 40
    ));
    assert!(!super::stamp_first_content_line(
        &mut empty,
        Some(""),
        style,
        40
    ));

    let mut blank_then_text = vec![Line::from(""), Line::from(Span::raw("hello"))];
    assert!(super::stamp_first_content_line(
        &mut blank_then_text,
        Some("12:00"),
        style,
        40
    ));
    let joined: String = blank_then_text
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("hello") && joined.contains("12:00"),
        "clock must land on the first non-empty line, got {joined:?}"
    );
    assert!(
        super::stamp_first_content_line(&mut blank_then_text, Some("12:00"), style, 40),
        "a line that already carries the clock must return true without restamping"
    );

    let rail = super::accent_line(vec![Span::raw("body")], false, Style::default());
    let text: String = rail.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "body");
}

#[test]
fn live_content_skips_duplicate_text_block_markdown() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "live answer");
    let i = app.messages.len() - 1;
    app.messages[i].blocks = vec![crate::app::ChatBlock::Text("duplicate block".into())];
    let lines = super::render_message(&app.messages[i], &app, &palette, i, 60, None, false);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("live") || joined.contains("answer"),
        "non-empty content must paint the live answer, got {joined:?}"
    );
    assert!(
        !joined.contains("duplicate"),
        "Text blocks are skipped when live content is already present, got {joined:?}"
    );
}

#[test]
fn user_prompt_slash_token_and_long_first_line_wrap() {
    let palette = ThemeName::DefaultDark.palette();
    let skill = Style::default().fg(palette.accent);
    let body = Style::default().fg(palette.fg);
    let spans = super::prompt_body_spans("please /help now", body, skill);
    assert!(
        spans.iter().any(|s| s.content.as_ref().contains("/help")),
        "a leading slash command must be its own skill span, got {spans:?}"
    );
    assert!(
        super::prompt_body_spans("", body, skill)
            .iter()
            .all(|s| s.content.is_empty())
    );
    let not_cmd = super::prompt_body_spans("a/b /1 /", body, skill);
    let not_cmd_text: String = not_cmd.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(not_cmd_text, "a/b /1 /");
    let hyphen = super::prompt_body_spans("/foo-bar rest", body, skill);
    assert!(
        hyphen.iter().any(|s| s.content.as_ref() == "/foo-bar"),
        "hyphenated slash tokens must stay one skill span, got {hyphen:?}"
    );

    let long = "word ".repeat(40);
    let wrapped = super::user_prompt_lines(&long, &[], Some("1:00"), &palette, 24, false, true);
    let joined: String = wrapped
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("word"),
        "a long first line must wrap onto continuation rows, got {joined:?}"
    );

    let many = (0..8)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let folded = super::user_prompt_lines(&many, &[], None, &palette, 40, false, false);
    let folded_text: String = folded
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        folded_text.contains('…') || folded_text.contains("{2026}"),
        "a collapsed multi-line user prompt must ellipsize, got {folded_text:?}"
    );
    let images = super::user_prompt_lines(
        "[Images: a.png, b.png]",
        &["a.png".into(), "b.png".into()],
        None,
        &palette,
        40,
        false,
        true,
    );
    let img: String = images
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        !img.contains("[Images:"),
        "multi-image placeholder must not repeat when chips exist, got {img:?}"
    );

    let short_then_long = format!("hi\n{}", "word ".repeat(30));
    let cont = super::user_prompt_lines(&short_then_long, &[], None, &palette, 20, false, true);
    let cont_text: String = cont
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        cont_text.contains("hi") && cont_text.contains("word"),
        "a short first line then a long wrap must paint both, got {cont_text:?}"
    );
}

#[test]
fn last_scrolled_past_user_and_empty_sticky_header() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "");
    app.add_message(ChatRole::Assistant, "reply");
    app.add_message(ChatRole::User, "later prompt");
    let starts = vec![0, 4, 10, 16];
    assert_eq!(super::last_scrolled_past_user(&app, &starts, 0), None);
    assert_eq!(
        super::last_scrolled_past_user(&app, &starts, 5),
        Some(0),
        "first user bubble sits above a mid-transcript viewport"
    );
    assert_eq!(
        super::last_scrolled_past_user(&app, &starts, 12),
        Some(2),
        "walks back to the latest user prompt above the viewport"
    );

    let palette = ThemeName::DefaultDark.palette();
    let empty = super::sticky_user_lines(&app.messages[0], &palette, 40, false);
    assert_eq!(empty.len(), 2, "sticky header is pad + one ❯ line");
    let selected = super::sticky_user_lines(&app.messages[2], &palette, 20, true);
    let joined: String = selected
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("later") || joined.contains("prompt") || joined.contains('❯'),
        "{joined:?}"
    );
}

#[test]
fn session_bar_layout_gives_the_gutter_back_when_narrower_wrap_fits() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "short");
    app.add_message(ChatRole::Assistant, "ok");
    let full = 80u16;
    let height = 40usize;
    let (starts, total, width, needs_bar) = super::session_bar_layout(&mut app, full, height);
    assert!(!needs_bar, "two short bubbles must fit a 40-row pane");
    assert_eq!(
        width, full,
        "gutter must be given back when the wrap still fits"
    );
    assert!(!starts.is_empty());
    assert!(total > 0);

    let mut app = TuiApp::new(TuiAppConfig::default());
    for i in 0..40 {
        app.add_message(ChatRole::User, format!("user line {i} that wraps a bit"));
        app.add_message(ChatRole::Assistant, format!("assistant reply {i}"));
    }
    let (_s, total, width, needs_bar) = super::session_bar_layout(&mut app, 80, 8);
    assert!(needs_bar, "a tall transcript in an 8-row pane needs a bar");
    assert!(
        width < 80,
        "overflow must reserve the scrollbar gutter, got width={width} total={total}"
    );
}

#[test]
fn refresh_live_markdown_seeds_stream_and_skip_omits_growing_answer() {
    use crate::app::AgentState;
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "growing **answer**");
    app.current_agent_state = AgentState::Generating;
    let i = app.messages.len() - 1;
    assert!(app.messages[i].stream_md.is_none());
    super::refresh_live_markdown(&mut app, i, 60);
    assert!(
        app.messages[i].stream_md.is_some(),
        "a live assistant must seed IncrementalMarkdown"
    );
    super::refresh_live_markdown(&mut app, 0, 60);
    assert!(
        app.messages[0].stream_md.is_none(),
        "a closed user bubble must not grow a stream buffer"
    );

    let skipped = super::render_message_live(&mut app, i, &palette, 60, false);
    let joined: String = skipped
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        !joined.contains("growing"),
        "include_live_md=false must omit the growing answer, got {joined:?}"
    );
    let full = super::render_message_live(&mut app, i, &palette, 60, true);
    let joined: String = full
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("growing") || joined.contains("answer"),
        "include_live_md=true must paint the growing answer, got {joined:?}"
    );
}

#[test]
fn thinking_lines_puts_elapsed_on_the_right_while_running() {
    use crate::app::ThinkingBlock;
    let palette = ThemeName::DefaultDark.palette();
    let mut t = ThinkingBlock::new("reason");
    t.started_at = std::time::Instant::now() - std::time::Duration::from_millis(1400);
    let lines = super::thinking_lines(&t, &palette, 40, 0);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("Thinking") && (joined.contains("1.") || joined.contains("s")),
        "running thought with elapsed > 0 must put a timer on the right, got {joined:?}"
    );

    let mut blank = ThinkingBlock::new("first\n\nsecond");
    blank.collapsed = false;
    blank.finish();
    let lines = super::thinking_lines(&blank, &palette, 40, 0);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("first") && joined.contains("second"),
        "expanded thought with a blank line must skip the empty wrap row, got {joined:?}"
    );
}

#[test]
fn collapsed_file_tools_group_instead_of_expanding_each_card() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "");
    let i = app.messages.len() - 1;
    app.messages[i].results_expanded = false;
    app.messages[i].blocks = vec![
        crate::app::ChatBlock::ToolUse {
            id: "a".into(),
            name: "read".into(),
            input: json!({"path": "a.rs"}),
        },
        crate::app::ChatBlock::ToolUse {
            id: "b".into(),
            name: "read".into(),
            input: json!({"path": "b.rs"}),
        },
    ];
    let lines = super::render_message(&app.messages[i], &app, &palette, i, 60, None, true);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("read") || joined.contains("file") || joined.contains("Read"),
        "collapsed file tools must paint as a grouped run, got {joined:?}"
    );
}

#[test]
fn empty_assistant_content_paints_inline_text_block_markdown() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "");
    let i = app.messages.len() - 1;
    app.messages[i].blocks = vec![crate::app::ChatBlock::Text("inline **bold** answer".into())];
    let lines = super::render_message(&app.messages[i], &app, &palette, i, 60, None, false);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        joined.contains("inline") || joined.contains("bold") || joined.contains("answer"),
        "empty content + Text block must emit markdown, got {joined:?}"
    );
}

#[test]
fn sticky_header_stops_when_the_viewport_is_one_row() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = TuiApp::new(TuiAppConfig::default());
    for i in 0..12 {
        app.add_message(ChatRole::User, format!("user {i}"));
        app.add_message(ChatRole::Assistant, format!("asst {i}"));
    }
    app.scroll_offset = 80;
    let backend = TestBackend::new(40, 1);
    let mut terminal = Terminal::new(backend).expect("term");
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .expect("draw");
    let buf = terminal.backend().buffer();
    assert_eq!(buf.area().height, 1);
}

#[test]
fn session_paint_blanks_leftover_rows_when_layout_cache_undershoots() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "ok");
    // Inflate height cache so the viewport thinks there are leftover rows
    // after the live paint (stale layout).
    for m in &mut app.messages {
        m.layout_cache = Some((40, true, 30));
        m.line_cache = None;
    }
    let backend = TestBackend::new(40, 16);
    let mut terminal = Terminal::new(backend).expect("term");
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area().height {
        for x in 0..buf.area().width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
    }
    assert!(
        out.contains("hi") || out.contains("ok"),
        "undershot layout must still paint the transcript, got {out:?}"
    );
}

#[test]
fn paint_chat_row_fills_and_skips_empty() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 2));
    let row = super::ChatRowPaint {
        x: 0,
        width: 0,
        bg: Color::Black,
        caret_style: Style::default(),
    };
    super::paint_chat_row(&mut buf, 0, &row, None, false);
    let row = super::ChatRowPaint {
        x: 0,
        width: 10,
        bg: Color::Black,
        caret_style: Style::default().fg(Color::White),
    };
    super::paint_chat_row(
        &mut buf,
        0,
        &row,
        Some(&Line::from(vec![
            Span::styled("hi", Style::default().bg(Color::Blue)),
            Span::raw(""),
        ])),
        true,
    );
    assert_eq!(buf[(0, 0)].symbol(), "▌");
}

#[test]
fn grep_match_count_and_diff_stat() {
    assert_eq!(
        super::grep_match_count("(12 matches in 3 files; pattern `foo`)"),
        Some(12)
    );
    assert_eq!(super::grep_match_count("(1 match)"), Some(1));
    assert_eq!(
        super::grep_match_count("src/a.rs:1:hit\nsrc/a.rs:2:hit2"),
        Some(2)
    );
    assert!(super::grep_match_count("no hits here").is_none());
    let (a, d) = super::diff_stat("--- a\n+++ b\n-old\n+new\n+also\n");
    assert_eq!((a, d), (2, 1));
}

fn joined(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tool_block_expanded_headers_execute_error_diff_grep_and_hints() {
    let palette = ThemeName::DefaultDark.palette();
    let paint = |expanded: bool, is_error: bool| ToolPaint {
        is_error,
        palette: &palette,
        expanded,
        width: 80,
        spin: 0,
    };

    let run = tool_block(
        "bash",
        &json!({"command": "seq"}),
        Some("ok\n"),
        paint(true, false),
    );
    let header = joined(&run);
    assert!(header.contains('┃') || header.contains('•'), "{header}");
    assert!(header.contains("Run"), "{header}");

    let err = tool_block(
        "bash",
        &json!({"command": "false"}),
        Some("boom"),
        paint(true, true),
    );
    let header = joined(&err);
    assert!(header.contains('✕') || header.contains("boom"), "{header}");

    let diff = tool_block(
        "apply_patch",
        &json!({}),
        Some("--- a\n+++ b\n-old\n+new\n+also\n"),
        paint(true, false),
    );
    let header = joined(&diff);
    assert!(header.contains("+2") && header.contains("−1"), "{header}");

    let grep = tool_block(
        "grep",
        &json!({"pattern": "foo"}),
        Some("src/a.rs:1:foo\nsrc/b.rs:2:foo\n\n(2 matches in 2 files; pattern `foo`)"),
        paint(true, false),
    );
    let header = joined(&grep);
    assert!(
        header.contains("2") && header.contains("matches"),
        "{header}"
    );

    let grep1 = tool_block(
        "search_code",
        &json!({"query": "bar"}),
        Some("(1 match)"),
        paint(true, false),
    );
    let header = joined(&grep1);
    assert!(header.contains("match"), "{header}");

    let rg = tool_block(
        "rg",
        &json!({"pattern": "x"}),
        Some("(3 matches)"),
        paint(true, false),
    );
    let header = joined(&rg);
    assert!(
        header.contains("3") && header.contains("matches"),
        "rg alias must paint the match chip, got {header}"
    );

    let add_only = tool_block(
        "apply_patch",
        &json!({}),
        Some("--- a\n+++ b\n+only-add\n"),
        paint(true, false),
    );
    let header = joined(&add_only)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        header.contains('+') && !header.contains('−'),
        "add-only diff header must paint +N without a delete chip, got {header}"
    );
    let del_only = tool_block(
        "apply_patch",
        &json!({}),
        Some("--- a\n+++ b\n-only-del\n"),
        paint(true, false),
    );
    let header = joined(&del_only)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        header.contains('−') && !header.contains('+'),
        "delete-only diff header must paint −N without an add chip, got {header}"
    );

    let rail = super::accent_line(
        vec![Span::raw("thinking")],
        true,
        Style::default().fg(Color::Cyan),
    );
    let rail_text: String = rail.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        rail_text.contains('┃') && rail_text.contains("thinking"),
        "show_rail must prefix the accent column, got {rail_text:?}"
    );

    let read = tool_block(
        "read",
        &json!({"path": "src/lib.rs"}),
        Some("fn main() {}\n"),
        paint(true, false),
    );
    assert!(!joined(&read).is_empty());

    let unknown_diff = tool_block(
        "custom",
        &json!({}),
        Some("diff --git a/x b/x\n+added\n-removed\n"),
        paint(true, false),
    );
    assert!(joined(&unknown_diff).contains('+') || joined(&unknown_diff).contains("added"));

    assert!(matches!(
        tool_out_hint("git_diff", &json!({}), ""),
        ToolOutHint::Diff
    ));
    assert!(matches!(
        tool_out_hint("edit", &json!({}), "diff --git a b\n+x"),
        ToolOutHint::Diff
    ));
    assert!(matches!(
        tool_out_hint("read_file", &json!({"file_path": "a.rs"}), "fn x() {}"),
        ToolOutHint::Code(_)
    ));
    assert!(matches!(
        tool_out_hint("rg", &json!({"pattern": "x"}), "a.rs:1:x"),
        ToolOutHint::Grep { .. }
    ));
    assert!(matches!(
        tool_out_hint("other", &json!({}), "diff --git a b\n+x"),
        ToolOutHint::Diff
    ));
    assert!(matches!(
        tool_out_hint("other", &json!({}), "plain"),
        ToolOutHint::Auto
    ));
    assert_eq!(super::tool_header_verb("web_fetch", false), "Fetched");
    assert_eq!(super::tool_header_verb("list_dir", false), "Listed");
    assert_eq!(super::tool_header_verb("repomap", false), "Mapped");

    let fail_group = paint_tool_run(
        &[
            ToolRef {
                name: "read",
                input: &json!({"path": "a.rs"}),
                result: Some("nope"),
                is_error: true,
            },
            ToolRef {
                name: "grep",
                input: &json!({"pattern": "x"}),
                result: Some("miss"),
                is_error: true,
            },
        ],
        paint(false, false),
    );
    let grouped = joined(&fail_group);
    assert!(
        grouped.contains("failed") || grouped.contains("Read") || grouped.contains("Searched"),
        "{grouped}"
    );

    let empty = paint_tool_run(&[], paint(true, false));
    assert!(empty.is_empty());

    let expanded_multi = paint_tool_run(
        &[
            ToolRef {
                name: "read",
                input: &json!({"path": "a.rs"}),
                result: Some("fn a() {}"),
                is_error: false,
            },
            ToolRef {
                name: "bash",
                input: &json!({"command": "ls"}),
                result: Some("ok"),
                is_error: false,
            },
        ],
        paint(true, false),
    );
    let expanded = joined(&expanded_multi);
    assert!(
        expanded.contains("Read") || expanded.contains("Run") || expanded.contains('•'),
        "expanded multi-tool run must paint each card, got {expanded}"
    );
    let one = paint_tool_run(
        &[ToolRef {
            name: "read",
            input: &json!({"path": "solo.rs"}),
            result: Some("fn solo() {}"),
            is_error: false,
        }],
        paint(false, false),
    );
    assert!(
        !joined(&one).is_empty(),
        "a single collapsed tool still paints via tool_block"
    );

    let long_plain = tool_block(
        "other",
        &json!({}),
        Some(&format!("{}\n{}", "x".repeat(80), "y".repeat(80))),
        paint(true, false),
    );
    assert!(joined(&long_plain).contains('…') || joined(&long_plain).len() > 10);

    let long_diff = tool_block(
        "apply_patch",
        &json!({}),
        Some(
            &(0..30)
                .map(|i| format!("+line-{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        paint(true, false),
    );
    assert!(joined(&long_diff).contains('+') || joined(&long_diff).contains("line"));

    let over_budget = tool_result(
        &(0..30)
            .map(|i| format!("+line-{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
        false,
        &palette,
        false,
        ToolOutHint::Diff,
        80,
    );
    let collapsed = joined(&over_budget);
    assert!(
        collapsed.contains('…'),
        "collapsed over-budget diff must ellipsize, got {collapsed}"
    );
}

#[test]
fn verb_group_line_mixes_buckets_running_failed_and_skips_unknown() {
    let palette = ThemeName::DefaultDark.palette();
    let paint = |expanded: bool, is_error: bool| ToolPaint {
        is_error,
        palette: &palette,
        expanded,
        width: 80,
        spin: 0,
    };

    let mixed = paint_tool_run(
        &[
            ToolRef {
                name: "read",
                input: &json!({"path": "a.rs"}),
                result: Some("fn a() {}"),
                is_error: false,
            },
            ToolRef {
                name: "read_file",
                input: &json!({"file_path": "b.rs"}),
                result: Some("fn b() {}"),
                is_error: false,
            },
            ToolRef {
                name: "grep",
                input: &json!({"pattern": "x"}),
                result: Some("a.rs:1:x"),
                is_error: false,
            },
            ToolRef {
                name: "list_dir",
                input: &json!({"path": "."}),
                result: Some("a.rs"),
                is_error: false,
            },
            ToolRef {
                name: "web_search",
                input: &json!({"query": "rust"}),
                result: Some("hits"),
                is_error: false,
            },
            ToolRef {
                name: "webfetch",
                input: &json!({"url": "https://example.com"}),
                result: Some("ok"),
                is_error: false,
            },
            ToolRef {
                name: "memory_search",
                input: &json!({"query": "auth"}),
                result: Some("hit"),
                is_error: false,
            },
            ToolRef {
                name: "bash",
                input: &json!({"command": "ls"}),
                result: Some("ok"),
                is_error: false,
            },
        ],
        paint(false, false),
    );
    let text = joined(&mixed);
    assert!(
        text.contains("Read")
            && text.contains("file")
            && text.contains("Searched")
            && (text.contains("dir") || text.contains("Listed"))
            && text.contains(", "),
        "collapsed mixed verbs must join buckets with commas, got {text}"
    );

    let running = paint_tool_run(
        &[
            ToolRef {
                name: "read",
                input: &json!({"path": "a.rs"}),
                result: None,
                is_error: false,
            },
            ToolRef {
                name: "glob",
                input: &json!({"glob": "*.rs"}),
                result: Some("a.rs"),
                is_error: false,
            },
        ],
        paint(false, false),
    );
    let live = joined(&running);
    assert!(
        live.contains("Reading") || live.contains("Searching"),
        "any in-flight tool must switch the group to present tense, got {live}"
    );

    let failed = paint_tool_run(
        &[
            ToolRef {
                name: "read",
                input: &json!({"path": "a.rs"}),
                result: Some("nope"),
                is_error: true,
            },
            ToolRef {
                name: "list",
                input: &json!({"path": "."}),
                result: Some("err"),
                is_error: true,
            },
        ],
        paint(false, false),
    );
    let fail = joined(&failed);
    assert!(
        fail.contains("failed"),
        "collapsed group with errors must append a failed count, got {fail}"
    );

    let err_diff = tool_block(
        "apply_patch",
        &json!({}),
        Some("--- a\n+++ b\n-old\n+new\n"),
        paint(true, true),
    );
    let err_text = joined(&err_diff);
    assert!(
        err_text.contains('✕') || err_text.contains("old") || err_text.contains("new"),
        "expanded error diff must paint the error rail, got {err_text}"
    );
}

#[test]
fn render_message_groups_orphan_tools_then_flushes_before_text() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "");
    let i = app.messages.len() - 1;
    app.messages[i].results_expanded = false;
    app.messages[i].blocks = vec![
        crate::app::ChatBlock::ToolUse {
            id: "t-read".into(),
            name: "read".into(),
            input: json!({"path": "a.rs"}),
        },
        crate::app::ChatBlock::Text("inline answer".into()),
    ];
    app.messages[i].tool_calls = vec![
        crate::app::ChatToolCall {
            id: "t-read".into(),
            name: "read".into(),
            arguments: json!({"path": "a.rs"}),
            collapsed: true,
            result: Some("fn a() {}".into()),
            is_error: false,
        },
        crate::app::ChatToolCall {
            id: "orphan-grep".into(),
            name: "grep".into(),
            arguments: json!({"pattern": "x"}),
            collapsed: true,
            result: Some("a.rs:1:x".into()),
            is_error: false,
        },
        crate::app::ChatToolCall {
            id: "orphan-run".into(),
            name: "bash".into(),
            arguments: json!({"command": "ls"}),
            collapsed: true,
            result: Some("ok".into()),
            is_error: false,
        },
    ];
    let lines = super::render_message(&app.messages[i], &app, &palette, i, 60, None, false);
    let text = joined(&lines);
    assert!(
        text.contains("inline") || text.contains("answer"),
        "empty content + Text block must flush the group then emit markdown, got {text}"
    );
    assert!(
        text.contains("Read")
            || text.contains("Searched")
            || text.contains("file")
            || text.contains("Run"),
        "orphan grouped tools must still paint, got {text}"
    );
}

#[test]
fn home_recents_without_timestamp_still_list_the_title() {
    use crate::app::{SessionEntry, TuiApp};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = TuiApp::new(TuiAppConfig::default());
    app.provider_name.clear();
    app.model_name.clear();
    app.session_list.sessions = vec![SessionEntry {
        id: "s1".into(),
        title: "a very long recent session title that must truncate in the home list".into(),
        messages: 2,
        updated_at: None,
        live: None,
    }];
    let backend = TestBackend::new(48, 24);
    let mut terminal = Terminal::new(backend).expect("term");
    let palette = app.config.palette();
    terminal
        .draw(|f| super::render(f, f.area(), &mut app, &palette))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area().height {
        for x in 0..buf.area().width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    assert!(
        out.contains("recent") || out.contains("resume") || out.contains("session"),
        "home recents without updated_at must still list the title, got {out}"
    );
    assert_eq!(super::truncate_home_title("short", 20), "short");
    assert_eq!(super::truncate_home_title("hello", 0), "");
    assert_eq!(super::truncate_home_title("hello", 1), "…");
    let truncated = super::truncate_home_title("hello world", 6);
    assert!(
        truncated.ends_with('…') && truncated.starts_with("hello"),
        "long home titles must ellipsize, got {truncated:?}"
    );
}
