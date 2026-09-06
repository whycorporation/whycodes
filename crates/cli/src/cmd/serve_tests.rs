use super::*;

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
