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
fn render_openrouter_key_dialog_masks_and_tiny() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.openrouter_key_input = "sk-or-secret-key".into();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_openrouter_key_dialog(f, &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("OpenRouter") || text.contains("API"),
        "{text}"
    );
    assert!(!text.contains("sk-or-secret-key"), "{text}");
    app.openrouter_key_input.clear();
    let tiny = TestBackend::new(8, 3);
    let mut tiny_term = Terminal::new(tiny).unwrap();
    tiny_term
        .draw(|f| render_openrouter_key_dialog(f, &mut app, &palette))
        .unwrap();
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

#[test]
fn render_provider_select_shows_discovered_local_servers() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.provider_dialog.providers = vec!["acme".into()];
    app.provider_dialog.discovering = true;
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let grab = |terminal: &Terminal<TestBackend>| -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect()
    };
    terminal
        .draw(|f| render_provider_dialog(f, &mut app, &palette))
        .unwrap();
    assert!(grab(&terminal).contains("Probing 127.0.0.1"));

    app.provider_dialog.discovering = false;
    app.provider_dialog.local_servers = vec![
        whycodes_llm::LocalServer {
            provider: "lmstudio".into(),
            label: "LM Studio".into(),
            api_base: "http://127.0.0.1:1234/v1".into(),
            models: vec!["qwen".into()],
        },
        whycodes_llm::LocalServer {
            provider: "vllm".into(),
            label: "vLLM".into(),
            api_base: "http://127.0.0.1:8000/v1".into(),
            models: vec!["a".into(), "b".into()],
        },
    ];
    app.provider_dialog.selected = 1;
    terminal
        .draw(|f| render_provider_dialog(f, &mut app, &palette))
        .unwrap();
    let text = grab(&terminal);
    assert!(text.contains("2 local server(s) found"), "{text}");
    assert!(text.contains("lmstudio · LM Studio · 1 model"), "{text}");
    assert!(text.contains("http://127.0.0.1:1234/v1"), "{text}");
    assert!(text.contains("vllm · vLLM · 2 models"), "{text}");

    assert_eq!(
        local_servers_status(false, 0),
        "No local model servers running (LM Studio, vLLM, llama.cpp)."
    );
    let none = whycodes_llm::LocalServer {
        provider: "llamacpp".into(),
        label: "llama.cpp server".into(),
        api_base: "http://127.0.0.1:8080/v1".into(),
        models: Vec::new(),
    };
    assert_eq!(
        local_server_row_label(&none),
        "llamacpp · llama.cpp server · no models loaded"
    );
}
