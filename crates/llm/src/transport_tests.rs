use super::*;

#[test]
fn format_turn_error_cleans_server_json() {
    let err = whycodes_core::Error::llm(
        r#"{"error":{"message":"[500]: An internal server error occurred","type":"server_error","code":"internal_server_error"}}"#,
    );
    let s = format_turn_error(&err);
    assert!(!s.contains('{'), "{s}");
    assert!(
        s.to_ascii_lowercase().contains("server") || s.contains("500"),
        "{s}"
    );
}

fn req() -> LlmRequest {
    LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(vec![whycodes_core::types::Message {
            role: whycodes_core::types::Role::User,
            content: whycodes_core::types::MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: vec![].into(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

#[tokio::test]
async fn complete_and_stream_via_scripted_provider() {
    use crate::scripted::{ScriptedProvider, ScriptedStep};
    use tokio_stream::StreamExt;
    use whycodes_core::types::ContentBlock;

    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let transport = LlmTransport::default().with_retry(crate::retry::RetryPolicy::test_fast());
    let provider = ScriptedProvider::new([ScriptedStep::Text("hello-transport".into())]);
    let req = req();
    let resp = transport.complete(&provider, &req, "k", "m").await.unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello-transport"))),
        "{resp:?}"
    );

    let provider = ScriptedProvider::new([
        ScriptedStep::Text("he".into()),
        ScriptedStep::Text("llo".into()),
    ]);
    let mut stream = transport.stream(&provider, &req, "k", "m").await.unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hello");
}

#[tokio::test]
async fn complete_times_out_when_scripted_hang_exceeds_budget() {
    use crate::scripted::{ScriptedProvider, ScriptedStep};
    let transport = LlmTransport {
        retry: crate::retry::RetryPolicy {
            max_retries: 0,
            ..crate::retry::RetryPolicy::test_fast()
        },
        complete_timeout: Some(std::time::Duration::from_millis(20)),
    };
    let provider = ScriptedProvider::new([ScriptedStep::Hang(std::time::Duration::from_secs(2))]);
    let err = transport
        .complete(&provider, &req(), "k", "hang-timeout")
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_ascii_lowercase().contains("timed out")
            || err.to_string().contains("Timeout"),
        "{err}"
    );
}

#[tokio::test]
async fn stream_turn_replays_cache_hit() {
    use crate::response_cache::ResponseCache;
    use crate::scripted::{ScriptedProvider, ScriptedStep};
    use tokio_stream::StreamExt;

    let req = req();
    ResponseCache::global().store(&req, "cache-model", "cached-text");
    let transport = LlmTransport::default();
    let provider = ScriptedProvider::new([ScriptedStep::Text("should-not-run".into())]);
    let mut turn = transport
        .stream_turn(
            crate::race::StreamTarget {
                provider: &provider,
                api_key: "k",
                model: "cache-model",
            },
            &req,
            StreamTurnOpts {
                cache: true,
                race: None,
                race_after: std::time::Duration::from_millis(10),
            },
        )
        .await
        .unwrap();
    assert!(turn.cache_hit);
    let mut text = String::new();
    while let Some(ev) = turn.events.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "cached-text");
}

#[tokio::test]
async fn stream_turn_skips_cache_when_disabled() {
    use crate::scripted::{ScriptedProvider, ScriptedStep};
    use tokio_stream::StreamExt;

    let req = req();
    let transport = LlmTransport::default();
    let provider = ScriptedProvider::new([ScriptedStep::Text("live-turn".into())]);
    let mut turn = transport
        .stream_turn(
            crate::race::StreamTarget {
                provider: &provider,
                api_key: "k",
                model: "live-model",
            },
            &req,
            StreamTurnOpts {
                cache: false,
                race: None,
                race_after: std::time::Duration::from_millis(10),
            },
        )
        .await
        .unwrap();
    assert!(!turn.cache_hit);
    let mut text = String::new();
    while let Some(ev) = turn.events.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "live-turn");
}

#[test]
fn default_transport_and_user_facing_error() {
    let t = default_transport();
    assert!(t.complete_timeout.is_some());
    let err = whycodes_core::Error::llm("rate limited 429");
    let msg = user_facing_error(&err);
    assert!(!msg.is_empty());
    let unknown = whycodes_core::Error::llm("something odd happened");
    assert_eq!(
        user_facing_error(&unknown),
        "LLM error: something odd happened"
    );
    assert_eq!(format_turn_error(&unknown), "something odd happened");
}

#[tokio::test]
async fn complete_cache_hit_and_no_timeout() {
    use crate::response_cache::ResponseCache;
    use crate::scripted::{ScriptedProvider, ScriptedStep};

    let req = req();
    ResponseCache::global().store(&req, "complete-cache", "from-cache");
    let transport = LlmTransport {
        retry: crate::retry::RetryPolicy {
            max_retries: 0,
            ..crate::retry::RetryPolicy::test_fast()
        },
        complete_timeout: None,
    };
    let provider = ScriptedProvider::new([ScriptedStep::Text("should-not-run".into())]);
    let resp = transport
        .complete(&provider, &req, "k", "complete-cache")
        .await
        .unwrap();
    assert!(resp.content.iter().any(
        |b| matches!(b, whycodes_core::types::ContentBlock::Text { text } if text == "from-cache")
    ));

    let provider = ScriptedProvider::new([ScriptedStep::Text("live".into())]);
    let resp = transport
        .complete(&provider, &req, "k", "no-timeout-model")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(
            |b| matches!(b, whycodes_core::types::ContentBlock::Text { text } if text == "live")
        )
    );
}
