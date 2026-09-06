use super::*;
use crate::config::TuiAppConfig;

#[test]
fn slash_command_from_prompt_and_consume() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert!(slash_command_from_prompt(&app).is_none());
    app.input_buffer = "/help".into();
    assert_eq!(slash_command_from_prompt(&app).as_deref(), Some("/help"));
    consume_slash_draft(&mut app);
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.input_cursor, 0);
    assert!(slash_command_from_prompt(&app).is_none());
}

#[test]
fn expand_at_files_inlines_and_keeps_bare_at() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello-at").unwrap();
    let out = expand_at_files("see @note.txt please", dir.path());
    assert!(out.contains("hello-at"), "{out}");
    assert!(out.contains("note.txt"), "{out}");
    assert_eq!(expand_at_files("keep @ alone", dir.path()), "keep @ alone");
    assert_eq!(expand_at_files("no mentions", dir.path()), "no mentions");
}
