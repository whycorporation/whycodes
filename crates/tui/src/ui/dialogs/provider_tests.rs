use super::*;
use crate::app::AuthMethod;
use crate::config::TuiAppConfig;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn render_provider_select_and_add_custom() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.provider_dialog.providers = vec!["acme".into(), "local".into()];
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_provider_dialog(f, &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("Select Provider") || text.contains("acme"),
        "{text}"
    );

    app.provider_dialog.mode = ProviderDialogMode::AddCustom;
    terminal
        .draw(|f| render_provider_dialog(f, &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("Custom") || text.contains("Name") || text.contains("API"),
        "{text}"
    );
}

#[test]
fn render_provider_select_scrollbar_and_tiny() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.provider_dialog.providers = (0..20).map(|i| format!("p{i}")).collect();
    app.provider_dialog.selected = 18;
    let backend = TestBackend::new(80, 16);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_provider_dialog(f, &mut app, &palette))
        .unwrap();
    let tiny = TestBackend::new(8, 3);
    let mut tiny_term = Terminal::new(tiny).unwrap();
    tiny_term
        .draw(|f| render_provider_dialog(f, &mut app, &palette))
        .unwrap();
}

#[test]
fn render_provider_add_custom_auth_methods_and_model_empty_query() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.provider_dialog.mode = ProviderDialogMode::AddCustom;
    for method in [
        AuthMethod::ApiKey,
        AuthMethod::Bearer,
        AuthMethod::Basic,
        AuthMethod::None,
    ] {
        app.provider_dialog.form_auth_method = method;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_provider_dialog(f, &mut app, &palette))
            .unwrap();
    }

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.model_selection.models = vec![("acme".into(), "m1".into()), ("xai".into(), "grok".into())];
    app.model_selection.query = "zzzz-no-match".into();
    app.model_selection.searching = true;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_model_dialog(f, &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("No matching") || text.contains("matching models"),
        "{text}"
    );

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.model_selection.models = (0..30).map(|i| ("acme".into(), format!("m{i}"))).collect();
    app.model_selection.selected = 28;
    let backend = TestBackend::new(80, 16);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_model_dialog(f, &mut app, &palette))
        .unwrap();
    let tiny = TestBackend::new(8, 3);
    let mut tiny_term = Terminal::new(tiny).unwrap();
    tiny_term
        .draw(|f| render_model_dialog(f, &mut app, &palette))
        .unwrap();
}
