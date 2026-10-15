use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// HTTP/1.1 server that answers every request on a connection with `body`
/// and counts accepted connections.
async fn counting_server(body: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = sock.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let head = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    if sock.write_all(head.as_bytes()).await.is_err()
                        || sock.write_all(&body).await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    (origin, accepted)
}

async fn wait_for(accepted: &AtomicUsize, n: usize) {
    for _ in 0..200 {
        if accepted.load(Ordering::SeqCst) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn origin_keeps_scheme_host_and_port_only() {
    assert_eq!(
        endpoint_origin("https://api.example.com/v1/chat/completions").as_deref(),
        Some("https://api.example.com")
    );
    assert_eq!(
        endpoint_origin("http://127.0.0.1:4554/api/chat").as_deref(),
        Some("http://127.0.0.1:4554")
    );
    assert_eq!(endpoint_origin("ftp://example.com/x"), None);
    assert_eq!(endpoint_origin("not a url"), None);
}

#[test]
fn nothing_starts_outside_a_runtime() {
    assert!(!spawn("https://outside-runtime.invalid/v1"));
}

#[test]
fn an_origin_is_claimed_once_per_window() {
    let origin = "https://claim-window.invalid";
    let now = Instant::now();
    assert!(claim(origin, now));
    assert!(!claim(origin, now + Duration::from_secs(1)));
    assert!(claim(origin, now + PRECONNECT_WINDOW));
    release(origin);
    assert!(claim(origin, now));
}

#[tokio::test]
async fn the_real_request_reuses_the_preconnected_socket() {
    let (origin, accepted) = counting_server(b"no".to_vec()).await;
    assert!(spawn(&format!("{origin}/v1/chat/completions")));
    assert!(!spawn(&format!("{origin}/v1/models")), "same origin again");
    wait_for(&accepted, 1).await;
    // Let the dial finish draining so the socket is back in the pool.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let reply = crate::client_identity::http_client()
        .get(format!("{origin}/v1/chat/completions"))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 404);
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "a second socket was opened"
    );
}

#[tokio::test]
async fn a_large_reply_is_drained_only_up_to_the_cap() {
    let (origin, accepted) = counting_server(vec![b'x'; DRAIN_CAP_BYTES * 2]).await;
    assert!(dial(&origin).await);
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_failed_dial_frees_the_origin() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(!dial(&origin).await);

    assert!(spawn(&origin));
    for _ in 0..200 {
        if !dialled().contains_key(&origin) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(spawn(&origin), "a failed dial must not block the next one");
}
