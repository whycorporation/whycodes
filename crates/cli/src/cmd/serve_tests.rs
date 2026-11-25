use super::*;

#[test]
fn connect_shares_cmd_run_test_tui_gate() {
    // Windows coverage: a leftover serve on :1 plus tui_available() used to
    // open a live TUI from `cmd_connect` and hold IsolatedHome's ENV_LOCK.
    assert!(!should_use_tui(false, false, true));
    assert!(should_use_tui(false, true, false));
}

#[tokio::test]
async fn web_stub_runs() {
    cmd_web().await.unwrap();
}

#[test]
fn connect_and_serve_printer_helpers() {
    assert!(attach_health_line("http://127.0.0.1:3030", "proj", 12).contains("proj"));
    let msg = connect_unreachable_msg("http://x", "boom", "x");
    assert!(msg.contains("cannot reach"));
    assert!(msg.contains("whycodes serve"));
    assert!(connect_session_line("abc").contains("abc"));
    assert!(serve_start_line(3030).contains("3030"));
    let lines = serve_endpoint_lines(3030, "127.0.0.1:3030");
    assert!(lines.iter().any(|l| l.contains("/v1/health")));
    assert!(lines.iter().any(|l| l.contains("127.0.0.1:3030")));
    let web = web_stub_lines();
    assert!(web.iter().any(|l| l.contains("not yet implemented")));
}

#[test]
fn connect_token_prefers_the_env_then_this_projects_lock_for_that_port() {
    let dir = tempfile::tempdir().unwrap();
    let base = "http://127.0.0.1:4040";
    assert_eq!(connect_token(None, dir.path(), base), None);
    let lock = super::super::lockfile::ServeLock {
        pid: 1,
        port: 4040,
        started_at: 0,
        interactive: false,
        parent_pid: None,
        token: Some("from-lock".into()),
    };
    let path = super::super::lockfile::lock_path(dir.path());
    super::super::lockfile::write_lock(&path, &lock).unwrap();
    assert_eq!(
        connect_token(None, dir.path(), base).as_deref(),
        Some("from-lock")
    );
    assert_eq!(
        connect_token(Some(" from-env ".into()), dir.path(), base).as_deref(),
        Some("from-env")
    );
    assert_eq!(
        connect_token(Some(" ".into()), dir.path(), base).as_deref(),
        Some("from-lock")
    );
    // Another port is another daemon; a base without a port names none.
    assert_eq!(
        connect_token(None, dir.path(), "http://127.0.0.1:5050"),
        None
    );
    assert_eq!(connect_token(None, dir.path(), "http://localhost"), None);
}
