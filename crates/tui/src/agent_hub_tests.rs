use super::*;
use crate::app::{AppMode, SubagentUpdate};
use crate::config::TuiAppConfig;
use crossterm::event::{Event, KeyEvent};
use ratatui::{Terminal, backend::TestBackend};

fn app() -> TuiApp {
    TuiApp::new(TuiAppConfig::default())
}

fn worker(app: &mut TuiApp, id: &str, status: &str, activity: &str, output: &str) {
    app.upsert_subagent(SubagentUpdate {
        id: id.into(),
        kind: "explore".into(),
        description: format!("goal of {id}"),
        status: status.into(),
        activity: activity.into(),
        elapsed_ms: 2500,
        output: output.into(),
    });
}

fn press(app: &mut TuiApp, code: KeyCode) {
    assert!(crate::input::handle_event(
        app,
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    ));
}

fn screen(
    app: &mut TuiApp,
    w: u16,
    h: u16,
    paint: impl Fn(&mut ratatui::Frame, &mut TuiApp),
) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| paint(f, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect()
}

#[test]
fn rows_put_running_first_then_newest_finished() {
    let mut a = app();
    worker(&mut a, "task-old", "completed", "", "TLDR: found it\nmore");
    worker(&mut a, "task-run", "running", "read src/lib.rs", "");
    worker(&mut a, "task-new", "cancelled", "", "");
    let ids: Vec<&str> = rows(&a).iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["task-run", "task-new", "task-old"]);

    a.set_subagent_tokens("task-old", 12_345);
    a.set_subagent_tokens("task-old", 0);
    a.set_subagent_tokens("missing", 5);
    let old = a.subagents.iter().find(|s| s.id == "task-old").unwrap();
    assert_eq!(
        row_text(old),
        (
            "task-old · explore · done".to_string(),
            "2.5s · 12.3k tok · TLDR: found it".to_string()
        )
    );
    let run = a.subagents.iter().find(|s| s.id == "task-run").unwrap();
    let (label, detail) = row_text(run);
    assert_eq!(label, "task-run · explore · running");
    assert!(detail.ends_with("s · read src/lib.rs"), "{detail}");
    let new = a.subagents.iter().find(|s| s.id == "task-new").unwrap();
    assert_eq!(row_text(new).1, "2.5s");

    a.agents_cursor = 1;
    assert_eq!(selected_id(&a).as_deref(), Some("task-new"));
    a.agents_cursor = 9;
    assert_eq!(selected_id(&a), None);
}

#[test]
fn labels_and_titles() {
    assert_eq!(status_label("running"), "running");
    assert_eq!(status_label("completed"), "done");
    assert_eq!(status_label("done"), "done");
    assert_eq!(status_label("cancelled"), "cancelled");
    assert_eq!(status_label("killed"), "cancelled");
    assert_eq!(status_label("failed"), "failed");
    assert_eq!(format_tokens(950), "950 tok");
    assert_eq!(format_tokens(1000), "1.0k tok");

    let mut a = app();
    assert!(title(&a).contains("k cancel") && title(&a).contains("r re-run"));
    a.attached = true;
    assert!(!title(&a).contains("cancel") && !title(&a).contains("re-run"));
    assert!(title(&a).contains("t tell"));
    assert!(rerun_prompt("task-1", "explore", "scan").starts_with("Re-run worker task-1: scan"));
}

#[test]
fn alt_a_opens_the_hub_and_arrows_move() {
    let mut a = app();
    worker(&mut a, "task-a", "running", "", "");
    worker(&mut a, "task-b", "completed", "", "");
    a.agents_cursor = 5;
    assert!(crate::input::handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT))
    ));
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Agents)));
    assert_eq!(a.mode, AppMode::Dialog);
    assert_eq!(a.agents_cursor, 0);
    press(&mut a, KeyCode::Down);
    assert_eq!(a.agents_cursor, 1);
    press(&mut a, KeyCode::Up);
    assert_eq!(a.agents_cursor, 0);
    // A Ctrl chord is not a hub key.
    assert!(!handle_key(
        &mut a,
        &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)
    ));
    press(&mut a, KeyCode::Esc);
    assert!(a.dialogs.active().is_none());
}

#[test]
fn t_prefills_tell_for_a_running_worker_only() {
    let mut a = app();
    worker(&mut a, "task-run", "running", "", "");
    worker(&mut a, "task-done", "completed", "", "");
    open(&mut a);
    press(&mut a, KeyCode::Char('t'));
    assert!(a.dialogs.active().is_none());
    assert_eq!(a.input_buffer, "/tell task-run ");
    assert_eq!(a.input_cursor, a.input_buffer.len());

    a.input_buffer.clear();
    open(&mut a);
    a.agents_cursor = 1;
    press(&mut a, KeyCode::Char('t'));
    assert!(a.input_buffer.is_empty());
    assert!(a.dialogs.active().is_none());
    assert!(!a.toasts.is_empty());
}

#[test]
fn k_requests_a_cancel_and_the_run_loop_sends_it() {
    let mut a = app();
    worker(&mut a, "task-run", "running", "", "");
    worker(&mut a, "task-done", "failed", "", "");
    open(&mut a);
    press(&mut a, KeyCode::Char('k'));
    assert_eq!(a.pending_worker_cancel.as_deref(), Some("task-run"));
    assert!(a.status_message.contains("Cancelling task-run"));
    assert!(
        matches!(a.dialogs.active(), Some(DialogKind::Agents)),
        "hub stays open"
    );

    let cancels = whycodes_agent::worker_cancel::WorkerCancels::default();
    cancels.register("task-run");
    send_pending_cancel(&mut a, &cancels);
    assert!(cancels.is_cancelled("task-run"));
    assert!(a.pending_worker_cancel.is_none());
    send_pending_cancel(&mut a, &cancels);

    a.toasts = Default::default();
    a.pending_worker_cancel = Some("gone".into());
    send_pending_cancel(&mut a, &cancels);
    assert!(!a.toasts.is_empty(), "a finished worker is explained");

    a.agents_cursor = 1;
    press(&mut a, KeyCode::Char('k'));
    assert!(a.pending_worker_cancel.is_none(), "nothing to cancel");
}

#[test]
fn r_queues_a_rerun_as_the_next_user_turn() {
    let mut a = app();
    worker(&mut a, "task-run", "running", "", "");
    worker(&mut a, "task-done", "completed", "", "");
    open(&mut a);
    press(&mut a, KeyCode::Char('r'));
    assert!(
        a.pending_reruns.is_empty(),
        "a running worker is not re-run"
    );

    a.agents_cursor = 1;
    press(&mut a, KeyCode::Char('r'));
    assert!(a.dialogs.active().is_none());
    assert_eq!(a.pending_reruns.len(), 1);

    send_pending_rerun(&mut a, true);
    assert!(!a.has_pending_turn(), "waits while a turn runs");
    a.enqueue_prompt_text("typed first");
    send_pending_rerun(&mut a, false);
    assert_eq!(a.pending_reruns.len(), 1, "waits behind a queued turn");
    a.take_pending_turn();
    send_pending_rerun(&mut a, false);
    assert!(a.pending_reruns.is_empty());
    let turn = a.take_pending_turn().unwrap();
    assert!(
        turn.text
            .starts_with("Re-run worker task-done: goal of task-done"),
        "{}",
        turn.text
    );
    assert!(
        a.messages
            .iter()
            .any(|m| m.role == ChatRole::User && m.content.starts_with("Re-run worker")),
        "the user sees what is sent"
    );
    send_pending_rerun(&mut a, false);
}

#[test]
fn attached_hub_ignores_cancel_and_rerun() {
    let mut a = app();
    a.attached = true;
    worker(&mut a, "task-run", "running", "", "");
    worker(&mut a, "task-done", "completed", "", "");
    open(&mut a);
    press(&mut a, KeyCode::Char('k'));
    a.agents_cursor = 1;
    press(&mut a, KeyCode::Char('r'));
    assert!(a.pending_worker_cancel.is_none());
    assert!(a.pending_reruns.is_empty());
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Agents)));
}

#[test]
fn empty_hub_keys_fall_through_and_enter_closes() {
    let mut a = app();
    open(&mut a);
    assert!(!handle_key(
        &mut a,
        &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)
    ));
    press(&mut a, KeyCode::Enter);
    assert!(a.dialogs.active().is_none());
    assert!(a.open_subagent.is_none());
}

#[test]
fn enter_opens_the_live_log_then_the_saved_file_once_finished() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = app();
    a.project_dir = dir.path().to_path_buf();
    worker(&mut a, "task-1", "running", "read a.rs", "→ read a.rs\n");
    open(&mut a);
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.open_subagent.as_deref(), Some("task-1"));
    assert!(a.subagent_view_file.is_none(), "running: the live log");
    let text = screen(&mut a, 70, 12, |f, a| {
        crate::ui::subagents::render_frame(f, f.area(), a, &a.config.palette())
    });
    assert!(text.contains("→ read a.rs"), "{text}");

    let agents = whycodes_core::project_dir(dir.path()).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("task-1.md"), "saved report").unwrap();
    worker(&mut a, "task-1", "completed", "", "final");
    assert_eq!(a.subagent_view_file.as_deref(), Some("saved report"));
    let text = screen(&mut a, 70, 12, |f, a| {
        crate::ui::subagents::render_frame(f, f.area(), a, &a.config.palette())
    });
    assert!(text.contains("saved report"), "{text}");

    press(&mut a, KeyCode::Char('q'));
    assert!(a.open_subagent.is_none());
    assert!(a.subagent_view_file.is_none());
}

#[test]
fn worker_view_scrolls_and_clamps_to_its_text() {
    let mut a = app();
    let log: String = (0..40).map(|i| format!("line {i}\n")).collect();
    worker(&mut a, "task-1", "running", "", &log);
    a.open_subagent_view("task-1");
    press(&mut a, KeyCode::Down);
    press(&mut a, KeyCode::Char('j'));
    assert_eq!(a.subagent_view_scroll, 2);
    press(&mut a, KeyCode::Up);
    press(&mut a, KeyCode::Char('k'));
    assert_eq!(a.subagent_view_scroll, 0);
    press(&mut a, KeyCode::PageDown);
    assert_eq!(a.subagent_view_scroll, 10);
    press(&mut a, KeyCode::PageUp);
    assert_eq!(a.subagent_view_scroll, 0);
    a.subagent_view_scroll = 500;
    let text = screen(&mut a, 40, 12, |f, a| {
        crate::ui::subagents::render_frame(f, f.area(), a, &a.config.palette())
    });
    assert!(text.contains("line 39"), "clamped to the end: {text}");
    assert!(a.subagent_view_scroll < 500);
    press(&mut a, KeyCode::Char('g'));
    assert_eq!(a.subagent_view_scroll, 0);
    a.subagent_view_scroll = 3;
    press(&mut a, KeyCode::Home);
    assert_eq!(a.subagent_view_scroll, 0);
    // Other keys are not the view's.
    press(&mut a, KeyCode::Char('x'));
    press(&mut a, KeyCode::Esc);
    assert!(a.open_subagent.is_none());
}

#[test]
fn hub_dialog_paints_rows_or_an_empty_note() {
    let mut a = app();
    open(&mut a);
    let text = screen(&mut a, 90, 20, |f, a| {
        crate::ui::dialogs::render(f, a, &a.config.palette())
    });
    assert!(text.contains("No workers yet"), "{text}");

    worker(&mut a, "task-1", "running", "bash cargo test", "");
    let text = screen(&mut a, 90, 20, |f, a| {
        crate::ui::dialogs::render(f, a, &a.config.palette())
    });
    assert!(text.contains("task-1 · explore · running"), "{text}");
    assert!(text.contains("bash cargo test"), "{text}");
}

#[test]
fn read_artifact_uses_the_saved_name() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_artifact(dir.path(), "task-1"), None);
    assert_eq!(read_artifact(dir.path(), "../.."), None);
    let agents = whycodes_core::project_dir(dir.path()).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("worker-0.md"), "w0").unwrap();
    assert_eq!(read_artifact(dir.path(), "worker-0").as_deref(), Some("w0"));
    assert_eq!(
        read_artifact(dir.path(), "../worker-0").as_deref(),
        Some("w0")
    );
}
