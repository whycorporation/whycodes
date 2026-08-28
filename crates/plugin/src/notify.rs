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
    send_to_channels(cfg, payload, TELEGRAM_API_BASE).await;
}

/// Channel fan-out with the Telegram API base as a parameter so tests can
/// aim it at a loopback mock; production always passes [`TELEGRAM_API_BASE`].
async fn send_to_channels(cfg: &NotifyConfig, payload: &NotifyPayload, telegram_base: &str) {
    if !cfg.enabled_for(payload.event) {
        return;
    }
    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(1, 60));
    let text = format_message(payload);

    if let Some(url) = cfg.discord_webhook_url() {
        // The allowlist gate lives at the fan-out, before any request is
        // built, so a config value can never aim the notifier at an
        // arbitrary POST target.
        if !discord_url_allowed(url) {
            tracing::warn!(
                "notify: discord webhook URL is not an allowed Discord Incoming Webhook"
            );
        } else if let Err(e) = post_discord(url, &text, timeout).await {
            tracing::warn!(error = %e, "notify: discord webhook failed");
        }
    }

    if let (Some(token), Some(chat)) = (cfg.telegram_token(), cfg.telegram_chat())
        && let Err(e) = post_telegram(telegram_base, token, chat, &text, timeout).await
    {
        tracing::warn!(error = %e, "notify: telegram send failed");
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

/// Discord host allowlist, relaxed to loopback origins under `cfg(test)` so
/// the full send path can run against a local mock server.
fn discord_url_allowed(url: &str) -> bool {
    if cfg!(test) && url.starts_with("http://127.0.0.1:") {
        return true;
    }
    is_discord_webhook_url(url)
}

/// POST to a Discord webhook. The caller (`send_to_channels`) has already
/// allowlisted the host; this only shapes the body (mentions disabled).
async fn post_discord(url: &str, content: &str, timeout: Duration) -> Result<(), String> {
    let body = json!({ "content": content, "allowed_mentions": { "parse": [] } });
    post_json(url, &body, timeout, None).await
}

/// Fixed Telegram Bot API origin; only tests substitute a different base.
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

fn telegram_send_url(base: &str, token: &str) -> Result<String, String> {
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
    Ok(format!("{base}/bot{token}/sendMessage"))
}

async fn post_telegram(
    base: &str,
    token: &str,
    chat_id: &str,
    text: &str,
    timeout: Duration,
) -> Result<(), String> {
    let url = telegram_send_url(base, token)?;
    let body = json!({
        "chat_id": chat_id,
        "text": text,
        "disable_web_page_preview": true,
    });
    post_json(&url, &body, timeout, None).await
}

/// Shared reqwest-error → String mapper; a named fn (not a closure) so an
/// unreachable branch (client build failure) cannot hold a coverage region.
fn err_string(e: reqwest::Error) -> String {
    e.to_string()
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
        .map_err(err_string)?;
    let mut req = client
        .post(url)
        .header("content-type", "application/json")
        .json(body);
    if let Some(headers) = extra_headers {
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
    }
    let resp = req.send().await.map_err(err_string)?;
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
    fn format_message_keeps_short_session_id_whole() {
        let p = NotifyPayload::turn_done("t", "b", Some("ab12".into()));
        let msg = format_message(&p);
        assert!(msg.contains("`ab12`"), "{msg}");
    }

    #[tokio::test]
    async fn post_json_reports_connect_error() {
        // Port 1 on loopback: nothing listens; the send itself must fail
        // and surface as an Err, not a panic.
        let err = post_json(
            "http://127.0.0.1:1/hook",
            &json!({}),
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err();
        assert!(!err.is_empty());
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
        assert_eq!(
            telegram_send_url(TELEGRAM_API_BASE, "123:abc").as_deref(),
            Ok("https://api.telegram.org/bot123:abc/sendMessage")
        );
        assert!(telegram_send_url(TELEGRAM_API_BASE, "123/evil").is_err());
        assert!(telegram_send_url(TELEGRAM_API_BASE, "123?x=1").is_err());
        assert!(telegram_send_url(TELEGRAM_API_BASE, "123#f").is_err());
        assert!(telegram_send_url(TELEGRAM_API_BASE, "123 x").is_err());
        assert!(telegram_send_url(TELEGRAM_API_BASE, "123\nx").is_err());
        assert!(telegram_send_url(TELEGRAM_API_BASE, "123\rx").is_err());
        assert!(telegram_send_url(TELEGRAM_API_BASE, "").is_err());
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
    async fn post_discord_sends_content_with_mentions_disabled() {
        let (url, captured) = capture_post("HTTP/1.1 204 No Content");
        post_discord(&url, "hi", Duration::from_secs(2))
            .await
            .expect("mock 204");
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("\"content\":\"hi\""), "{raw}");
        assert!(raw.contains("allowed_mentions"), "{raw}");
    }

    #[tokio::test]
    async fn post_json_sets_extra_headers() {
        let (url, captured) = capture_post("HTTP/1.1 200 OK");
        post_json(
            &url,
            &json!({}),
            Duration::from_secs(2),
            Some(&[("x-test-header", "v1")]),
        )
        .await
        .expect("mock 200");
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.to_lowercase().contains("x-test-header: v1"), "{raw}");
    }

    #[tokio::test]
    async fn post_telegram_hits_send_message_route() {
        let (url, captured) = capture_post("HTTP/1.1 200 OK");
        let base = url.strip_suffix("/hook").expect("mock URL shape");
        post_telegram(base, "123:abc", "42", "hi", Duration::from_secs(2))
            .await
            .expect("mock 200");
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("POST /bot123:abc/sendMessage"), "{raw}");
        assert!(raw.contains("\"chat_id\":\"42\""), "{raw}");
    }

    #[tokio::test]
    async fn post_telegram_rejects_bad_token_without_request() {
        let err = post_telegram(
            TELEGRAM_API_BASE,
            "bad/token",
            "1",
            "hi",
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(err.contains("invalid characters"), "{err}");
    }

    #[tokio::test]
    async fn send_to_channels_posts_both_channels() {
        let (discord_url, discord_captured) = capture_post("HTTP/1.1 204 No Content");
        let (tg_url, tg_captured) = capture_post("HTTP/1.1 200 OK");
        let tg_base = tg_url.strip_suffix("/hook").expect("mock URL shape");
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            discord_webhook: Some(discord_url),
            telegram_bot_token: Some("123:abc".into()),
            telegram_chat_id: Some("42".into()),
            ..NotifyConfig::default()
        };
        let payload = NotifyPayload::turn_done("t", "b", Some("sessionid".into()));
        send_to_channels(&cfg, &payload, tg_base).await;
        let discord_raw = discord_captured.lock().expect("lock").clone();
        assert!(discord_raw.contains("\"content\""), "{discord_raw}");
        let tg_raw = tg_captured.lock().expect("lock").clone();
        assert!(tg_raw.contains("sendMessage"), "{tg_raw}");
    }

    #[tokio::test]
    async fn send_to_channels_logs_failures_without_stopping() {
        // Discord: disallowed host → allowlist warn. Telegram: mock 500 →
        // send warn. Both branches run; neither aborts the fan-out.
        let (tg_url, tg_captured) = capture_post("HTTP/1.1 500 Internal Server Error");
        let tg_base = tg_url.strip_suffix("/hook").expect("mock URL shape");
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            discord_webhook: Some("https://example.com/api/webhooks/1/x".into()),
            telegram_bot_token: Some("123:abc".into()),
            telegram_chat_id: Some("42".into()),
            ..NotifyConfig::default()
        };
        let payload = NotifyPayload::turn_done("t", "b", None);
        send_to_channels(&cfg, &payload, tg_base).await;
        let tg_raw = tg_captured.lock().expect("lock").clone();
        assert!(
            tg_raw.contains("sendMessage"),
            "telegram must still run: {tg_raw}"
        );
    }

    #[tokio::test]
    async fn send_to_channels_telegram_only_skips_discord() {
        let (tg_url, captured) = capture_post("HTTP/1.1 200 OK");
        let tg_base = tg_url.strip_suffix("/hook").expect("mock URL shape");
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            telegram_bot_token: Some("123:abc".into()),
            telegram_chat_id: Some("42".into()),
            ..NotifyConfig::default()
        };
        send_to_channels(&cfg, &NotifyPayload::turn_done("t", "b", None), tg_base).await;
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("sendMessage"), "{raw}");
    }

    #[tokio::test]
    async fn send_to_channels_discord_only_success() {
        let (discord_url, captured) = capture_post("HTTP/1.1 204 No Content");
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            discord_webhook: Some(discord_url),
            ..NotifyConfig::default()
        };
        send_to_channels(
            &cfg,
            &NotifyPayload::turn_done("t", "b", None),
            "http://127.0.0.1:1",
        )
        .await;
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("\"content\""), "{raw}");
    }

    #[tokio::test]
    async fn send_to_channels_warns_on_discord_http_error() {
        let (discord_url, _captured) = capture_post("HTTP/1.1 500 Internal Server Error");
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            discord_webhook: Some(discord_url),
            ..NotifyConfig::default()
        };
        send_to_channels(
            &cfg,
            &NotifyPayload::turn_done("t", "b", None),
            "http://127.0.0.1:1",
        )
        .await;
    }

    #[tokio::test]
    async fn send_to_channels_skips_disabled_event() {
        // need_input not in `on` → no channel is contacted (no mock server
        // exists; a request would error loudly if one were attempted).
        let cfg = NotifyConfig {
            on: vec!["turn_done".into()],
            telegram_bot_token: Some("123:abc".into()),
            telegram_chat_id: Some("42".into()),
            ..NotifyConfig::default()
        };
        let payload = NotifyPayload::need_input("t", "b", None);
        send_to_channels(&cfg, &payload, "http://127.0.0.1:1").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_notify_sends_when_enabled() {
        // Exercises the spawned production path end-to-end: spawn_notify →
        // send_notify → Discord POST against the loopback mock (allowed
        // under cfg(test)).
        let (url, captured) = capture_post("HTTP/1.1 204 No Content");
        let cfg = NotifyConfig {
            on: vec!["need_input".into()],
            discord_webhook: Some(url),
            ..NotifyConfig::default()
        };
        spawn_notify(cfg, NotifyPayload::need_input("t", "b", None));
        for _ in 0..50 {
            if !captured.lock().expect("lock").is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let raw = captured.lock().expect("lock").clone();
        assert!(raw.contains("\"content\""), "{raw}");
    }

    #[test]
    fn discord_allowlist_still_blocks_non_discord_hosts() {
        // The cfg(test) loopback exception must not widen the production
        // allowlist: non-loopback, non-Discord origins stay rejected.
        assert!(!discord_url_allowed("https://example.com/api/webhooks/1/x"));
        assert!(!discord_url_allowed("http://evil.test/api/webhooks/1/x"));
        assert!(discord_url_allowed("https://discord.com/api/webhooks/1/x"));
    }
}
