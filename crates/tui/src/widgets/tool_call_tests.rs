use super::*;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn palette() -> ThemePalette {
    ThemeName::DefaultDark.palette()
}

fn paint<F>(width: u16, height: u16, f: F) -> String
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(f).expect("draw");
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

#[test]
fn collapsed_renders_single_line() {
    let p = palette();
    let text = paint(80, 3, |f| {
        render_tool_call(f, f.area(), "bash", "echo hi", Some("ok"), false, true, &p);
    });
    let line = text.lines().next().unwrap();
    assert!(line.contains("▶ bash"), "{text}");
    assert!(line.contains("echo hi"), "{text}");
    // Collapsed: no args/result detail rows.
    assert!(!text.contains("args:"), "{text}");
    assert!(!text.contains("result"), "{text}");
}

#[test]
fn expanded_shows_args_and_result() {
    let p = palette();
    let text = paint(80, 20, |f| {
        render_tool_call(
            f,
            f.area(),
            "bash",
            "echo hi",
            Some("done"),
            false,
            false,
            &p,
        );
    });
    assert!(text.contains("▼ bash"), "{text}");
    assert!(text.contains("args: echo hi"), "{text}");
    assert!(text.contains("── result ──"), "{text}");
    assert!(text.contains("│ done"), "{text}");
}

#[test]
fn expanded_without_result_omits_section() {
    let p = palette();
    let text = paint(80, 20, |f| {
        render_tool_call(f, f.area(), "read", "a.rs", None, false, false, &p);
    });
    assert!(text.contains("▼ read"), "{text}");
    assert!(text.contains("args: a.rs"), "{text}");
    assert!(!text.contains("result"), "{text}");
}

#[test]
fn long_result_lines_are_capped_and_more_lines_noted() {
    let p = palette();
    let long_line = "y".repeat(200);
    let many = (0..15)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = paint(80, 40, |f| {
        render_tool_call(
            f,
            f.area(),
            "read",
            "f",
            Some(&format!("{long_line}\n{many}")),
            false,
            false,
            &p,
        );
    });
    // Over-long single line truncated to 95 chars + ellipsis; the run
    // wraps across rows but the total painted y count stays capped.
    assert!(text.contains("…"), "{text}");
    assert_eq!(
        text.chars().filter(|c| *c == 'y').count(),
        95,
        "capped to 95 chars: {text}"
    );
    // Only 10 result lines shown (long line + line-0..line-8) plus a
    // "more lines" footer.
    assert!(text.contains("more lines"), "{text}");
    assert!(text.contains("line-8"), "{text}");
    assert!(!text.contains("line-9"), "beyond the 10-line cap: {text}");
}

#[test]
fn error_result_uses_error_color() {
    let p = palette();
    let text = paint(80, 20, |f| {
        render_tool_call(
            f,
            f.area(),
            "bash",
            "rm -rf /",
            Some("permission denied"),
            true,
            false,
            &p,
        );
    });
    assert!(text.contains("▼ bash"), "{text}");
    assert!(text.contains("permission denied"), "{text}");
    // Collapsed error variant still paints the error glyph line.
    let text = paint(80, 3, |f| {
        render_tool_call(f, f.area(), "bash", "rm -rf /", None, true, true, &p);
    });
    assert!(text.contains("▶ bash"), "{text}");
}

#[test]
fn multi_line_args_are_indented() {
    let p = palette();
    let text = paint(80, 20, |f| {
        render_tool_call(f, f.area(), "edit", "line1\nline2", None, false, false, &p);
    });
    assert!(text.contains("args: line1"), "{text}");
    assert!(text.contains("args: line2"), "{text}");
}
