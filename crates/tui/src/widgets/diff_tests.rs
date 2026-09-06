use super::*;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

#[test]
fn parse_unified_diff_classifies_lines() {
    let parsed =
        parse_unified_diff("--- a/x\n+++ b/x\n@@ -1 +1 @@\n context\n-old\n+new\n\n leftover");
    assert_eq!(parsed[0].kind, DiffLineKind::Header);
    assert_eq!(parsed[1].kind, DiffLineKind::Header);
    assert_eq!(parsed[2].kind, DiffLineKind::Header);
    assert_eq!(parsed[3].kind, DiffLineKind::Context);
    assert_eq!(parsed[4].kind, DiffLineKind::Remove);
    assert_eq!(parsed[4].content, "old");
    assert_eq!(parsed[5].kind, DiffLineKind::Add);
    assert_eq!(parsed[5].content, "new");
    assert_eq!(parsed[6].kind, DiffLineKind::Context);
    assert!(parse_unified_diff("").is_empty());
}

#[test]
fn render_unified_and_split_paint_titles() {
    let palette = ThemeName::DefaultDark.palette();
    let lines = parse_unified_diff("@@ -1 +1 @@\n-old\n+new\n ctx");
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_unified_diff(f, Rect::new(0, 0, 60, 12), &lines, &palette);
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(text.contains("Diff"), "{text}");
    assert!(text.contains("old") && text.contains("new"), "{text}");

    terminal
        .draw(|f| {
            render_split_diff(f, Rect::new(0, 0, 60, 12), &lines, &lines, &palette);
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(text.contains("Old") && text.contains("New"), "{text}");
}
