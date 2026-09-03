//! Render tests for the dialogs module.
//!
//! Every dialog is a paint function taking `&mut Frame`, so the natural test
//! is to draw it into a `TestBackend` terminal and assert on the returned
//! chrome / paint metadata plus the painted buffer text. Loaded as
//! `dialogs::render_tests`.

use super::*;
use crate::app::{
    AppMode, AuthMethod, ConfirmAction, DialogKind, ImportPickerItem, ImportPickerState,
    ProviderDialogMode, QuestionDialogState, SessionEntry, TuiApp,
};
use crate::config::TuiAppConfig;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use whycodes_tools::question::{QuestionOption, QuestionSpec};

fn cfg() -> TuiAppConfig {
    TuiAppConfig::default()
}

/// Render `f` into a fresh terminal and return the painted buffer text.
fn paint<F>(width: u16, height: u16, f: F) -> (ratatui::buffer::Buffer, String)
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| f(frame)).expect("draw");
    let buf = terminal.backend().buffer().clone();
    let text = buffer_text(&buf);
    (buf, text)
}

fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
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

// ── base: dialog_frame geometry ────────────────────────────────────────

#[test]
fn dialog_frame_fills_blank_cells_with_palette_bg() {
    use ratatui::style::Color;
    let palette = ThemeName::DefaultDark.palette();
    let (buf, _) = paint(80, 24, |f| {
        let _ = dialog_frame(f, "Help", &["Esc / [✗]"], &palette, None);
    });
    // Interior of the modal must be themed spaces, not Reset (Clear leftover).
    let mut saw_fill = false;
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = buf.cell((x, y)).expect("cell");
            if cell.bg == palette.bg {
                saw_fill = true;
                assert_ne!(cell.bg, Color::Reset, "modal cell must not be Reset");
            }
        }
    }
    assert!(saw_fill, "expected palette.bg fill inside the modal");
}

#[test]
fn dialog_frame_centers_modal_and_reports_content() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, _text) = paint(80, 40, |f| {
        let chrome = dialog_frame(
            f,
            "Select Provider",
            &["↑/↓", "Enter select", "Esc / [✗]"],
            &palette,
            None,
        );
        // Modal centered horizontally: x >= 8 on an 80-wide screen at 60%.
        assert!(chrome.modal.x >= 8, "modal.x={}", chrome.modal.x);
        assert!(
            chrome.modal.width >= 40,
            "modal.width={}",
            chrome.modal.width
        );
        // Content is inside the border (padded both sides).
        assert!(
            chrome.content.x > chrome.modal.x,
            "content.x={} modal.x={}",
            chrome.content.x,
            chrome.modal.x
        );
        assert!(
            chrome.content.height < chrome.modal.height,
            "content must leave room for border + footer"
        );
        // Close button hit sits on the top border row.
        let hit = chrome.close_hit.expect("wide modal has a close button");
        assert_eq!(hit.y, chrome.modal.y);
        assert_eq!(hit.height, 1);
    });
}

#[test]
fn dialog_frame_placed_bottom_docks_modal() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, _text) = paint(100, 40, |f| {
        let chrome = dialog_frame_placed(
            f,
            "Question",
            &["Enter select"],
            &palette,
            80,
            30,
            None,
            DialogPlacement::Bottom,
        );
        // Docked: modal bottom edge == screen bottom edge.
        assert_eq!(
            chrome.modal.y + chrome.modal.height,
            40,
            "bottom-docked modal must end at the screen bottom"
        );
        // Centered variant must NOT be at the bottom.
        let centered = dialog_frame(f, "Q", &[], &palette, None);
        assert!(
            centered.modal.y + centered.modal.height <= 40,
            "centered modal stays inside the screen"
        );
    });
}

#[test]
fn dialog_frame_too_small_area_returns_no_close_hit() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, _text) = paint(8, 4, |f| {
        let chrome = dialog_frame(f, "T", &["Esc"], &palette, None);
        assert!(chrome.close_hit.is_none());
    });
}

#[test]
fn centered_rect_expands_on_phone_portrait() {
    // 50% of 40 cols is 20 — too narrow for a readable modal. Floor to 36.
    // Callers now pass Grok's 90%, but the floor still applies to a 50% request.
    let r = Rect::new(0, 0, 40, 24);
    let modal = centered_rect(50, 30, r);
    assert!(
        modal.width >= 36,
        "portrait modal.width={} must expand past 50%",
        modal.width
    );
    assert!(
        modal.height >= 10,
        "portrait modal.height={} must expand past 30%",
        modal.height
    );
    assert!(modal.x + modal.width <= 40);
    assert!(modal.y + modal.height <= 24);
}

#[test]
fn centered_rect_fills_tiny_viewport() {
    let r = Rect::new(0, 0, 20, 8);
    let modal = centered_rect(50, 30, r);
    assert_eq!(modal.width, 20);
    assert_eq!(modal.height, 8);
    assert_eq!(modal.x, 0);
    assert_eq!(modal.y, 0);
}

#[test]
fn bottom_rect_stays_docked_after_min_expand() {
    let r = Rect::new(0, 0, 40, 16);
    let modal = bottom_rect(50, 22, r);
    assert_eq!(
        modal.y + modal.height,
        16,
        "bottom-docked modal must stay on the bottom after expand"
    );
    assert!(modal.width >= 36, "modal.width={}", modal.width);
}

#[test]
fn close_button_rect_needs_room() {
    // Narrow modal → no close button.
    let narrow = Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 10,
    };
    assert!(close_button_rect(narrow).is_none());
    // Zero width → none.
    assert!(close_button_rect(Rect::default()).is_none());
}

// ── alert ──────────────────────────────────────────────────────────────

#[test]
fn alert_dialog_paints_title_and_message() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(80, 24, |f| {
        let chrome = render_alert_dialog(f, "Alert", "Something happened", &palette, None);
        assert!(chrome.content.width > 0);
        assert!(chrome.content.height > 0);
    });
    assert!(text.contains("Alert"), "{text}");
    assert!(text.contains("Something happened"), "{text}");
    // Footer shortcut chip.
    assert!(text.contains("[✗]"), "{text}");
}

// ── confirm ────────────────────────────────────────────────────────────

#[test]
fn confirm_dialog_paints_multiline_message() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(80, 24, |f| {
        render_confirm_dialog(f, "Sure?", "Line one\nLine two", &palette, None);
    });
    assert!(text.contains("Sure?"), "{text}");
    assert!(text.contains("Line one"), "{text}");
    assert!(text.contains("Line two"), "{text}");
}

#[test]
fn permission_dialog_shows_tool_command_and_risk() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(90, 30, |f| {
        render_permission_dialog(
            f,
            "bash",
            "Command:\nrm -rf /tmp/x\n\nRisk: destructive delete",
            &palette,
            None,
        );
    });
    assert!(text.contains("Permission required"), "{text}");
    assert!(text.contains("bash"), "{text}");
    assert!(text.contains("rm -rf /tmp/x"), "{text}");
    assert!(text.contains("destructive delete"), "{text}");
    assert!(text.contains("Allow this tool to run?"), "{text}");
}

#[test]
fn permission_dialog_without_risk_and_key_value_detail() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(90, 30, |f| {
        render_permission_dialog(f, "read", "path: src/main.rs\noffset: 10", &palette, None);
    });
    assert!(text.contains("read"), "{text}");
    assert!(text.contains("path: src/main.rs"), "{text}");
    // Not a command → "Details" label, no "$ " prefix.
    assert!(text.contains("Details"), "{text}");
}

#[test]
fn permission_dialog_truncates_overflowing_body() {
    let palette = ThemeName::DefaultDark.palette();
    // Many logical lines (not one wrapped blob) so the body exceeds the
    // content row budget even after the portrait min-height expand.
    let body: String = (0..40).map(|i| format!("echo line {i}\n")).collect();
    let detail = format!("Command:\n{body}\nRisk: x");
    let (_buf, text) = paint(80, 12, |f| {
        render_permission_dialog(f, "bash", &detail, &palette, None);
    });
    assert!(text.contains("truncated"), "{text}");
}

// ── select ─────────────────────────────────────────────────────────────

#[test]
fn select_dialog_paints_items_and_marks_selection() {
    let palette = ThemeName::DefaultDark.palette();
    let items = vec![
        SelectItem::new("alpha"),
        SelectItem::with_detail("beta", "the second"),
        SelectItem::new("gamma"),
    ];
    let (_buf, text) = paint(80, 30, |f| {
        let info = render_select(f, "Pick", &items, 1, "Nothing", &palette, None);
        assert_eq!(info.total, 3);
        assert!(info.list_area.is_some());
        assert!(
            info.visible >= 3,
            "viewport fits all items, visible={}",
            info.visible
        );
        assert!(info.modal.is_some());
        assert!(info.close_hit.is_some());
    });
    assert!(text.contains("alpha"), "{text}");
    assert!(text.contains("beta"), "{text}");
    assert!(text.contains("gamma"), "{text}");
    // Detail rendered dimmed after label.
    assert!(text.contains("the second"), "{text}");
    // Grok picker leaf mark on every row (selection is the wash, not ▸).
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("beta")).collect();
    assert!(
        lines.iter().any(|l| l.contains('◆')),
        "selected row must carry the diamond: {text}"
    );
}

#[test]
fn select_dialog_empty_message() {
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(80, 24, |f| {
        let info = render_select(f, "Pick", &[], 0, "No sessions yet", &palette, None);
        assert_eq!(info.total, 0);
        assert!(info.list_area.is_some());
    });
    assert!(text.contains("No sessions yet"), "{text}");
}

#[test]
fn select_dialog_overflow_paints_scrollbar() {
    let palette = ThemeName::DefaultDark.palette();
    let items: Vec<SelectItem> = (0..30)
        .map(|i| SelectItem::new(format!("item-{i}")))
        .collect();
    let (_buf, text) = paint(60, 12, |f| {
        let info = render_select(f, "Pick", &items, 29, "", &palette, None);
        assert!(
            info.scrollbar_hit.is_some(),
            "30 items in a 12-row box overflow"
        );
        assert!(info.scroll_start > 0, "selection near the end must scroll");
    });
    // Latest items visible after scroll.
    assert!(text.contains("item-29"), "{text}");
    // Scrollbar column painted on the right edge.
    assert!(text.contains('█') || text.contains('│'), "{text}");
}

// ── help ───────────────────────────────────────────────────────────────

#[test]
fn help_overlay_paints_sections_and_clamps_scroll() {
    let mut app = TuiApp::new(cfg());
    app.help_scroll = 999; // way beyond content — must clamp
    let palette = app.config.palette();
    let (buf, text) = paint(90, 24, |f| {
        render_help_overlay(f, &mut app, &palette);
    });
    assert!(text.contains("Keyboard Shortcuts"), "{text}");
    assert!(
        text.contains("/ to search") || text.contains("search:"),
        "{text}"
    );
    // Scrolled to the max offset, so the tail bindings are visible.
    assert!(text.contains("/exit") || text.contains("Quit"), "{text}");
    assert!(text.contains("◆"), "{text}");
    // Clamped into range so the scroll window never exceeds content.
    assert!(app.help_scroll <= 100, "help_scroll={}", app.help_scroll);
    assert!(app.help_scroll > 0, "large offset must clamp, not stay raw");
    // Hit metadata recorded for mouse handling.
    assert!(app.dialog_list_hit.is_some(), "help sets a list hit area");
    assert!(app.dialog_list_total > 0);
    // Scrollbar visible because content overflows a 24-row window.
    assert!(app.dialog_list_visible < app.dialog_list_total);
    let _ = buf;
}

#[test]
fn help_overlay_search_filters_bindings() {
    let mut app = TuiApp::new(cfg());
    app.help_searching = true;
    app.help_query = "provider".into();
    let palette = app.config.palette();
    let (_buf, text) = paint(90, 30, |f| {
        render_help_overlay(f, &mut app, &palette);
    });
    assert!(text.contains("search: provider"), "{text}");
    assert!(text.contains("Ctrl+P"), "{text}");
    assert!(
        !text.contains("Ctrl+O"),
        "unrelated session binding should drop out: {text}"
    );
}

#[test]
fn help_overlay_fits_without_scrollbar_on_tall_window() {
    let mut app = TuiApp::new(cfg());
    let palette = app.config.palette();
    let (_buf, _text) = paint(120, 200, |f| {
        render_help_overlay(f, &mut app, &palette);
    });
    // `visible` is the viewport height (not clamped to total), and the
    // content fits so no scrollbar is painted and the offset stays 0.
    assert!(app.dialog_list_visible >= app.dialog_list_total);
    assert_eq!(app.help_scroll, 0);
}

// ── provider ───────────────────────────────────────────────────────────

#[test]
fn provider_dialog_select_lists_providers_and_add_custom() {
    let mut app = TuiApp::new(cfg());
    app.provider_dialog.providers = vec!["anthropic".into(), "openai".into()];
    let palette = app.config.palette();
    let (_buf, text) = paint(80, 30, |f| {
        render_provider_dialog(f, &mut app, &palette);
    });
    assert!(text.contains("Select Provider"), "{text}");
    assert!(text.contains("anthropic"), "{text}");
    assert!(text.contains("openai"), "{text}");
    assert!(text.contains("Add Custom Provider"), "{text}");
    // Hit metadata recorded via apply_select_paint.
    assert!(app.dialog_list_hit.is_some());
    assert_eq!(app.dialog_list_total, 3); // 2 providers + Add Custom
}

#[test]
fn provider_dialog_add_custom_masks_api_key() {
    let mut app = TuiApp::new(cfg());
    app.provider_dialog.mode = ProviderDialogMode::AddCustom;
    app.provider_dialog.form_name = "groq".into();
    app.provider_dialog.form_api_key = "sk-super-secret".into();
    app.provider_dialog.form_base_url = "https://api.groq.com/openai/v1".into();
    app.provider_dialog.form_auth_method = AuthMethod::ApiKey;
    let palette = app.config.palette();
    let (_buf, text) = paint(90, 30, |f| {
        render_provider_dialog(f, &mut app, &palette);
    });
    assert!(text.contains("Add Custom Provider"), "{text}");
    assert!(text.contains("groq"), "{text}");
    assert!(text.contains("api.groq.com"), "{text}");
    // Secret must not leak as plaintext.
    assert!(!text.contains("sk-super-secret"), "{text}");
    assert!(text.contains('*'), "key masked with asterisks: {text}");
    // Auth method row.
    assert!(text.contains("Auth Method"), "{text}");
}

#[test]
fn provider_dialog_add_custom_shows_error_and_saved() {
    let mut app = TuiApp::new(cfg());
    app.provider_dialog.mode = ProviderDialogMode::AddCustom;
    app.provider_dialog.error = Some("invalid base URL".into());
    app.provider_dialog.saved = true;
    let palette = app.config.palette();
    let (_buf, text) = paint(90, 30, |f| {
        render_provider_dialog(f, &mut app, &palette);
    });
    assert!(text.contains("invalid base URL"), "{text}");
    assert!(text.contains("Provider saved!"), "{text}");
}

// ── model ──────────────────────────────────────────────────────────────

#[test]
fn model_dialog_empty_state_hints_at_provider() {
    let mut app = TuiApp::new(cfg());
    let palette = app.config.palette();
    let (_buf, text) = paint(80, 24, |f| {
        render_model_dialog(f, &mut app, &palette);
    });
    assert!(text.contains("Select Model"), "{text}");
    assert!(text.contains("No models configured"), "{text}");
    assert!(
        text.contains("Add a") || text.contains("provider first"),
        "wrapped empty-state hint: {text}"
    );
}

#[test]
fn model_dialog_lists_provider_model_pairs() {
    let mut app = TuiApp::new(cfg());
    app.model_selection.models = vec![
        ("anthropic".into(), "claude-sonnet".into()),
        ("openai".into(), "gpt-4o".into()),
    ];
    // Header, model, header, model — select gpt-4o.
    app.model_selection.selected = 3;
    let palette = app.config.palette();
    let (_buf, text) = paint(80, 24, |f| {
        render_model_dialog(f, &mut app, &palette);
    });
    assert!(text.contains("anthropic"), "{text}");
    assert!(text.contains("claude-sonnet"), "{text}");
    assert!(text.contains("openai"), "{text}");
    assert!(text.contains("gpt-4o"), "{text}");
    assert!(
        text.contains("/ to search") || text.contains("search:"),
        "{text}"
    );
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("gpt-4o")).collect();
    assert!(
        lines.iter().any(|l| l.contains('◆')),
        "selected model row must carry the diamond: {text}"
    );
    assert!(app.dialog_list_hit.is_some());
}

#[test]
fn model_dialog_search_and_collapsed_headers() {
    let mut app = TuiApp::new(cfg());
    app.model_selection.models = vec![
        ("anthropic".into(), "claude-sonnet".into()),
        ("anthropic".into(), "claude-opus".into()),
        ("openai".into(), "gpt-4o".into()),
    ];
    app.model_selection.collapsed.insert("anthropic".into());
    app.model_selection.query = "gpt".into();
    app.model_selection.searching = true;
    let palette = app.config.palette();
    let (_buf, text) = paint(80, 24, |f| {
        render_model_dialog(f, &mut app, &palette);
    });
    assert!(text.contains("search: gpt"), "{text}");
    assert!(text.contains("gpt-4o"), "{text}");
    assert!(
        !text.contains("claude-sonnet"),
        "search should hide non-matching models: {text}"
    );
    // Query auto-expands matching groups even if they were collapsed.
    assert!(text.contains("openai"), "{text}");
}

// ── question ───────────────────────────────────────────────────────────

fn single_spec(multi: bool) -> Vec<QuestionSpec> {
    vec![QuestionSpec {
        prompt: "Pick one?".into(),
        options: vec![
            QuestionOption {
                label: "SQLite".into(),
                description: "simple".into(),
                preview: None,
            },
            QuestionOption {
                label: "Postgres".into(),
                description: "scalable".into(),
                preview: None,
            },
        ],
        multi_select: multi,
    }]
}

#[test]
fn question_dialog_paints_prompt_and_option_rows() {
    let state = QuestionDialogState::new(single_spec(false));
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(100, 30, |f| {
        let paint = render_question_dialog(f, &state, &palette, None);
        assert_eq!(paint.list_total, 3, "2 options + Other");
        assert!(paint.list_area.is_some(), "options map to a clickable list");
        assert!(paint.chrome.close_hit.is_some());
    });
    assert!(text.contains("Pick one?"), "{text}");
    assert!(text.contains("SQLite"), "{text}");
    assert!(text.contains("Postgres"), "{text}");
    assert!(text.contains("Other…"), "{text}");
    // Cursor marker on first option.
    assert!(text.contains('▸'), "{text}");
}

#[test]
fn question_dialog_free_text_focus_shows_input() {
    let mut state = QuestionDialogState::new(single_spec(false));
    state.free_text_focus = true;
    state.free_text = "my answer".into();
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(100, 30, |f| {
        render_question_dialog(f, &state, &palette, None);
    });
    assert!(text.contains("my answer"), "{text}");
    assert!(text.contains('>'), "prompt caret: {text}");
}

#[test]
fn question_dialog_multi_select_shows_checkboxes() {
    let mut state = QuestionDialogState::new(single_spec(true));
    state.multi_selected.insert(0);
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(100, 30, |f| {
        render_question_dialog(f, &state, &palette, None);
    });
    assert!(text.contains("[×]"), "checked box: {text}");
    assert!(text.contains("[ ]"), "unchecked box: {text}");
    assert!(text.contains("Multi-select"), "{text}");
}

#[test]
fn question_dialog_empty_options_is_free_form() {
    let state = QuestionDialogState::new(vec![QuestionSpec {
        prompt: "Anything else?".into(),
        options: vec![],
        multi_select: false,
    }]);
    // No options → free-text focus is auto-enabled by the constructor.
    assert!(state.free_text_focus);
    let palette = ThemeName::DefaultDark.palette();
    let (_buf, text) = paint(100, 30, |f| {
        render_question_dialog(f, &state, &palette, None);
    });
    assert!(text.contains("Anything else?"), "{text}");
    // Free-form question: Other row carries the type-your-own hint.
    assert!(text.contains("Other…"), "{text}");
}

// ── mod.rs dispatch ────────────────────────────────────────────────────

#[test]
fn render_dispatches_all_dialog_kinds() {
    // Opening each kind through the dispatcher must not panic and must set
    // the shared modal hit boxes.
    let mut app = TuiApp::new(cfg());

    // Alert
    app.dialogs.push(DialogKind::Alert {
        title: "Alert".into(),
        message: "msg".into(),
    });
    let palette = app.config.palette();
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(
        app.dialog_modal_hit.is_some(),
        "alert must register modal hit"
    );

    // Confirm
    app.dialogs.clear();
    app.dialogs.push(DialogKind::Confirm {
        title: "Quit?".into(),
        message: "Really?".into(),
        on_confirm: ConfirmAction::Quit,
    });
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_modal_hit.is_some());

    // Permission
    app.dialogs.clear();
    app.dialogs.push(DialogKind::Permission {
        tool_name: "bash".into(),
        detail: "Command:\necho hi".into(),
    });
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_modal_hit.is_some());

    // Provider (select mode)
    app.dialogs.clear();
    app.provider_dialog.providers = vec!["x".into()];
    app.dialogs.push(DialogKind::Provider);
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Model
    app.dialogs.clear();
    app.model_selection.models = vec![("p".into(), "m".into())];
    app.dialogs.push(DialogKind::Model);
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Agent
    app.dialogs.clear();
    app.primary_agents = vec!["build".into(), "plan".into()];
    app.agent_name = "build".into();
    app.dialogs.push(DialogKind::Agent);
    let (_buf, text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());
    assert!(text.contains("build"), "{text}");
    assert!(text.contains("plan"), "{text}");

    // Help
    app.dialogs.clear();
    app.dialogs.push(DialogKind::Help);
    let (_buf, _text) = paint(90, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Question
    app.dialogs.clear();
    app.dialogs
        .push(DialogKind::Question(QuestionDialogState::new(single_spec(
            false,
        ))));
    let (_buf, _text) = paint(100, 30, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Theme picker (select path)
    app.dialogs.clear();
    app.dialogs.push(DialogKind::Theme);
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Login
    app.dialogs.clear();
    app.dialogs.push(DialogKind::Login);
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Reasoning effort
    app.dialogs.clear();
    app.provider_name = "xai".into();
    app.model_name = "grok-4".into();
    app.dialogs.push(DialogKind::Effort);
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());

    // Import checkbox picker
    app.dialogs.clear();
    app.import_picker = ImportPickerState {
        items: vec![
            ImportPickerItem {
                label: "MCP `fs`".into(),
                detail: "npx".into(),
            },
            ImportPickerItem {
                label: "permission `bash` = ask".into(),
                detail: String::new(),
            },
        ],
        checked: vec![true, false],
        cursor: 0,
    };
    app.dialogs.push(DialogKind::Import);
    let (_buf, text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
    assert!(app.dialog_list_hit.is_some());
    assert!(text.contains("MCP `fs`"), "{text}");
    assert!(text.contains("[x]"), "{text}");
    assert!(text.contains("[ ]"), "{text}");

    // Close all — renders as no-op without panic.
    app.dialogs.clear();
    let (_buf, _text) = paint(80, 24, |f| super::render(f, &mut app, &palette));
}

#[test]
fn session_list_dispatch_builds_select_rows() {
    let mut app = TuiApp::new(cfg());
    app.session_list.sessions = vec![
        SessionEntry {
            id: "aaa".into(),
            title: "First".into(),
            messages: 3,
            updated_at: None,
            live: None,
        },
        SessionEntry {
            id: "bbb".into(),
            title: "Live one".into(),
            messages: 7,
            updated_at: None,
            live: Some(0),
        },
    ];
    app.dialogs.push(DialogKind::SessionList);
    let palette = app.config.palette();
    let (_buf, text) = paint(90, 24, |f| super::render(f, &mut app, &palette));
    assert!(text.contains("First"), "{text}");
    assert!(text.contains("Live one"), "{text}");
    assert!(text.contains("3 messages"), "{text}");
    assert!(app.dialog_list_hit.is_some());
}

#[test]
fn session_list_detail_missing_timestamp_omits_clock() {
    let entry = SessionEntry {
        id: "x".into(),
        title: "T".into(),
        messages: 2,
        updated_at: None,
        live: None,
    };
    assert_eq!(session_list_detail(&entry), "2 messages");
}

#[test]
fn help_mode_renders_via_standalone_entry() {
    let mut app = TuiApp::new(cfg());
    app.mode = AppMode::Help;
    app.dialogs.clear();
    let palette = app.config.palette();
    let (_buf, text) = paint(90, 24, |f| {
        render_help(f, &mut app, &palette);
    });
    assert!(text.contains("Keyboard Shortcuts"), "{text}");
    assert!(app.dialog_list_hit.is_some());
}
