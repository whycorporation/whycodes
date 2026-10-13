use super::*;

#[tokio::test]
async fn responses_notifications_and_errors_are_jsonrpc() {
    let (out, mut rx) = Outbox::new();
    out.respond(json!(1), json!({"ok": true}));
    out.respond_error(json!("a"), INVALID_PARAMS, "bad");
    out.notify("session/update", json!({"x": 1}));
    assert_eq!(
        rx.recv().await.unwrap(),
        json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}})
    );
    assert_eq!(
        rx.recv().await.unwrap(),
        json!({"jsonrpc": "2.0", "id": "a", "error": {"code": -32602, "message": "bad"}})
    );
    assert_eq!(
        rx.recv().await.unwrap(),
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {"x": 1}})
    );
}

#[tokio::test]
async fn requests_resolve_by_id() {
    let (out, mut rx) = Outbox::new();
    let asking = out.clone();
    let first = tokio::spawn(async move { asking.request("m", json!({})).await });
    let sent = rx.recv().await.unwrap();
    assert_eq!(sent["method"], "m");
    // Ignored: no id, unknown id.
    out.resolve(&json!({"result": 1}));
    out.resolve(&json!({"id": 99, "result": 1}));
    out.resolve(&json!({"id": sent["id"], "result": {"v": 7}}));
    assert_eq!(first.await.unwrap(), Some(json!({"v": 7})));

    let asking = out.clone();
    let second = tokio::spawn(async move { asking.request("m", json!({})).await });
    let sent = rx.recv().await.unwrap();
    out.resolve(&json!({"id": sent["id"], "error": {"code": 1, "message": "no"}}));
    assert_eq!(second.await.unwrap(), None, "error response");

    let asking = out.clone();
    let third = tokio::spawn(async move { asking.request("m", json!({})).await });
    rx.recv().await.unwrap();
    out.close_pending();
    assert_eq!(third.await.unwrap(), None, "connection closed");
}

#[tokio::test]
async fn write_loop_writes_lines_until_closed() {
    let (out, rx) = Outbox::new();
    out.notify("a", json!(1));
    out.notify("b", json!(2));
    drop(out);
    let mut buf = Vec::new();
    write_loop(&mut buf, rx).await.unwrap();
    let text = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"method\":\"a\""), "{text}");
    assert!(text.ends_with('\n'));
}

struct Broken;

impl AsyncWrite for Broken {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Err(std::io::Error::other("closed")))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn write_errors_end_the_loop_and_later_sends_are_dropped() {
    let (out, rx) = Outbox::new();
    out.notify("a", json!(1));
    assert!(write_loop(Broken, rx).await.is_err());
    // Receiver gone: sending only logs.
    out.notify("b", json!(2));
    let mut b = Broken;
    assert!(AsyncWriteExt::flush(&mut b).await.is_ok());
    assert!(AsyncWriteExt::shutdown(&mut b).await.is_ok());
}
