use super::*;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use whycodes_tools::question::{QuestionOption, QuestionSpec};

#[test]
fn render_question_paints_prompt_and_other() {
    let palette = ThemeName::DefaultDark.palette();
    let state = QuestionDialogState::new(vec![QuestionSpec {
        prompt: "Pick a store?".into(),
        options: vec![QuestionOption {
            label: "SQLite".into(),
            description: "local".into(),
            preview: None,
        }],
        multi_select: false,
        important: false,
    }]);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let paint = render_question_dialog(f, &state, &palette, None);
            assert!(paint.list_total >= 1);
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(text.contains("Pick a store"), "{text}");
    assert!(text.contains("SQLite") || text.contains("Other"), "{text}");
}
