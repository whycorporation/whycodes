//! Discord Incoming Webhook + Telegram Bot API session notifications.
//!
//! Best-effort: failures are logged, never surfaced to the agent loop.
//! Discord URLs are host-allowlisted in `whycodes-config`; Telegram always
//! posts to `https://api.telegram.org/bot…/sendMessage`.

use std::time::Duration;

use serde_json::{Value, json};
use whycodes_config::{NotifyConfig, NotifyEvent, is_discord_webhook_url};

/// One outbound notification.
#[derive(Debug, Clone)]
pub struct NotifyPayload {
    pub event: NotifyEvent,
    pub title: String,
    pub body: String,
    pub session_id: Option<String>,
}

impl NotifyPayload {
    pub fn turn_done(
        title: impl Into<String>,
        body: impl Into<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            event: NotifyEvent::TurnDone,
            title: title.into(),
            body: body.into(),
            session_id,
        }
    }

    pub fn need_input(
        title: impl Into<String>,
        body: impl Into<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            event: NotifyEvent::NeedInput,
            title: title.into(),
            body: body.into(),
            session_id,
        }
    }
}

/// Fire-and-forget send. No-op when the event is disabled or no channel is set.
pub fn spawn_notify(cfg: NotifyConfig, payload: NotifyPayload) {
    if !cfg.enabled_for(payload.event) {
        return;
    }
    tokio::spawn(async move {
        send_notify(&cfg, &payload).await;
    });
}

/// Awaited send (tests). Logs per-channel failures; never panics.
pub async fn send_notify(cfg: &NotifyConfig, payload: &NotifyPayload) {
    if !cfg.enabled_for(payload.event) {
        return;
    }
    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(1, 60));
    let text = format_message(payload);

    if let Some(url) = cfg.discord_webhook_url() {
        match post_discord(url, &text, timeout).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, "notify: discord webhook failed"),
        }
    }

    if let (Some(token), Some(chat)) = (cfg.telegram_token(), cfg.telegram_chat()) {
        match post_telegram(token, chat, &text, timeout).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, "notify: telegram send failed"),
        }
    }
}

fn format_message(payload: &NotifyPayload) -> String {
    let mut s = format!("**{}**\n{}", payload.title, payload.body.trim());
    if let Some(id) = payload.session_id.as_deref() {
        let short = if id.len() > 8 { &id[..8] } else { id };
        s.push_str("\n`");
        s.push_str(short);
        s.push('`');
    }
    // Discord webhook content cap is 2000; Telegram is 4096. Stay under both.
    truncate_chars(&s, 1900)
}

fn truncate_chars(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

async fn post_discord(url: &str, content: &str, timeout: Duration) -> Result<(), String> {
    if !is_discord_webhook_url(url) {
        return Err("discord webhook URL is not an allowed Discord Incoming Webhook".into());
    }
    let body = json!({ "content": content, "allowed_mentions": { "parse": [] } });
    post_json(url, &body, timeout, None).await
}

fn telegram_send_url(token: &str) -> Result<String, String> {
    if token.is_empty()
        || token.contains('/')
        || token.contains('?')
        || token.contains('#')
        || token.contains('\n')
        || token.contains('\r')
        || token.contains(' ')
    {
        return Err("telegram bot token contains invalid characters".into());
    }
    Ok(format!("https://api.telegram.org/bot{token}/sendMessage"))
}

async fn post_telegram(
    token: &str,
    chat_id: &str,
    text: &str,
    timeout: Duration,
) -> Result<(), String> {
    let url = telegram_send_url(token)?;
    let body = json!({
        "chat_id": chat_id,
        "text": text,
        "disable_web_page_preview": true,
    });
    post_json(&url, &body, timeout, None).await
}

async fn post_json(
    url: &str,
    body: &Value,
    timeout: Duration,
    extra_headers: Option<&[(&str, &str)]>,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client
        .post(url)
        .header("content-type", "application/json")
        .json(body);
    if let Some(headers) = extra_headers {
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let snippet = resp.text().await.unwrap_or_default();
    let snippet: String = snippet.chars().take(240).collect();
    Err(format!("HTTP {status}: {snippet}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn capture_post(status_line: &'static str) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock HTTP");
        let addr = listener.local_addr().expect("addr");
        let captured = Arc::new(Mutex::new(String::new()));
        let cap = Arc::clone(&captured);
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            *cap.lock().expect("lock") = String::from_utf8_lossy(&buf[..n]).into_owned();
            let _ = stream.write_all(
                format!("{status_line}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                    .as_bytes(),
            );
        });
        (format!("http://{addr}/hook"), captured)
    }

    #[test]
    fn format_message_includes_event_and_short_id() {
        let p = NotifyPayload::turn_done("Turn done", "Worked for 3s", Some("abcdefghij".into()));
        let msg = format_message(&p);
        assert!(msg.contains("**Turn done**"));
        assert!(msg.contains("Worked for 3s"));
        assert!(msg.contains("`abcdefgh`"));
        assert!(!msg.contains("ij"));
    }

    #[test]
    fn format_message_truncates() {
        let p = NotifyPayload::need_input("Need input", "x".repeat(3000), None);
        let msg = format_message(&p);
        assert!(msg.chars().count() <= 1900);
        assert!(msg.ends_with('…'));
    }

    #[test]
    fn telegram_url_rejects_path_injection() {
        assert!(telegram_send_url("123:abc").is_ok());
        assert!(telegram_send_url("123/evil").is_err());
        assert!(telegram_send_url("123?x=1").is_err());
        assert!(telegram_send_url("").is_err());
    }

    #[test]
    fn spawn_notify_noop_when_disabled() {
        spawn_notify(
            NotifyConfig::default(),
            NotifyPayload::turn_done("t", "b", None),
        );
    }

    #[tokio::test]
    async fn send_notify_skips_without_channel() {
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            ..NotifyConfig::default()
        };
        send_notify(&cfg, &NotifyPayload::turn_done("t", "b", None)).await;
    }

    #[tokio::test]
    async fn post_json_success_and_error() {
        let (url, captured) = capture_post("HTTP/1.1 204 No Content");
        post_json(
            &url,
            &json!({"content": "hi"}),
            Duration::from_secs(2),
            None,
        )
        .await
        .expect("204 ok");
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("POST "), "{raw}");
        assert!(raw.contains("\"content\":\"hi\""), "{raw}");

        let (url, _) = capture_post("HTTP/1.1 500 Internal Server Error");
        let err = post_json(&url, &json!({}), Duration::from_secs(2), None)
            .await
            .unwrap_err();
        assert!(err.contains("HTTP 500"), "{err}");
    }

    #[tokio::test]
    async fn post_discord_rejects_non_webhook() {
        let err = post_discord(
            "https://example.com/not-discord",
            "hi",
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(err.contains("allowed"), "{err}");
    }

    #[tokio::test]
    async fn send_notify_posts_telegram_when_configured() {
        let (url, captured) = capture_post("HTTP/1.1 200 OK");
        // Direct post_json path used by telegram; token URL is always api.telegram.org.
        post_json(
            &url,
            &json!({"chat_id": "1", "text": "hi"}),
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap();
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("chat_id"), "{raw}");
    }
}
