use super::*;
use futures::StreamExt;
use serde_json::json;
use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};

fn req() -> LlmRequest {
    LlmRequest {
        system: "s".into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(8),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

#[tokio::test]
async fn stream_plays_script_then_stops() {
    let p = ScriptedProvider::named(
        "s",
        [
            ScriptedStep::Text("hi".into()),
            ScriptedStep::Thinking("t".into()),
            ScriptedStep::ToolCall {
                id: "1".into(),
                name: "read".into(),
                input: json!({"path": "a"}),
            },
            ScriptedStep::Usage {
                input_tokens: 1,
                output_tokens: 2,
            },
            ScriptedStep::Hang(Duration::ZERO),
            ScriptedStep::Error("e".into()),
        ],
    );
    assert_eq!(p.name(), "s");
    assert_eq!(p.default_base_url(), "http://script.invalid");
    let mut stream = p.stream(&req(), "", "m").await.unwrap();
    let mut evs = Vec::new();
    while let Some(ev) = stream.next().await {
        evs.push(ev.unwrap());
    }
    assert!(matches!(evs[0], StreamEvent::TextDelta { .. }));
    assert!(matches!(evs[1], StreamEvent::Thinking { .. }));
    assert!(matches!(evs[2], StreamEvent::ToolUse { .. }));
    assert!(matches!(evs[3], StreamEvent::Usage { .. }));
    assert!(matches!(evs[4], StreamEvent::Error { .. }));
    assert!(matches!(evs.last(), Some(StreamEvent::MessageStop)));
}

#[tokio::test]
async fn stream_emits_extended_event_variants() {
    let p = ScriptedProvider::new([
        ScriptedStep::MessageStart,
        ScriptedStep::ThinkingDelta("td".into()),
        ScriptedStep::ThinkingSignature("sig".into()),
        ScriptedStep::RedactedThinking("red".into()),
        ScriptedStep::ToolCall {
            id: "1".into(),
            name: "read".into(),
            input: json!({}),
        },
        ScriptedStep::ToolUseDelta {
            id: "1".into(),
            input_json_delta: r#"{"path":"a"}"#.into(),
        },
        ScriptedStep::MessageDelta(json!({"stop_reason": "end_turn"})),
        ScriptedStep::CacheUsage {
            creation_input_tokens: 2,
            read_input_tokens: 3,
        },
    ]);
    let mut stream = p.stream(&req(), "", "m").await.unwrap();
    let mut kinds = Vec::new();
    while let Some(ev) = stream.next().await {
        kinds.push(std::mem::discriminant(&ev.unwrap()));
    }
    assert!(kinds.len() >= 9);
    let empty = ScriptedProvider::new([
        ScriptedStep::ThinkingDelta("t".into()),
        ScriptedStep::MessageStart,
        ScriptedStep::CacheUsage {
            creation_input_tokens: 1,
            read_input_tokens: 0,
        },
    ]);
    let out = empty.complete(&req(), "", "m").await.unwrap();
    assert!(out.content.is_empty());
}

#[tokio::test]
async fn fail_open_errors_before_stream() {
    let p = ScriptedProvider::new([ScriptedStep::FailOpen("nope".into())]);
    assert!(p.stream(&req(), "", "m").await.is_err());
}

#[tokio::test]
async fn stream_skips_later_fail_open_and_waits_out_hang() {
    let p = ScriptedProvider::new([
        ScriptedStep::Text("a".into()),
        ScriptedStep::FailOpen("ignored".into()),
        ScriptedStep::Hang(Duration::from_millis(1)),
        ScriptedStep::Text("b".into()),
    ]);
    let mut stream = p.stream(&req(), "", "m").await.unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let StreamEvent::TextDelta { text: d } = ev.unwrap() {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "ab");
}

#[tokio::test]
async fn complete_joins_text_and_reports_usage() {
    let p = ScriptedProvider::text("hello");
    let out = p.complete(&req(), "", "model-x").await.unwrap();
    assert_eq!(out.model, "model-x");
    assert!(matches!(
        &out.content[0],
        ContentBlock::Text { text } if text == "hello"
    ));
}

#[tokio::test]
async fn complete_error_and_empty_and_hang() {
    let err = ScriptedProvider::new([ScriptedStep::Error("x".into())]);
    assert!(err.complete(&req(), "", "m").await.is_err());

    let empty = ScriptedProvider::new([
        ScriptedStep::Thinking("t".into()),
        ScriptedStep::Hang(Duration::from_millis(1)),
        ScriptedStep::Usage {
            input_tokens: 3,
            output_tokens: 4,
        },
    ]);
    let out = empty.complete(&req(), "", "m").await.unwrap();
    assert!(out.content.is_empty());
    assert_eq!(out.usage.input_tokens, 3);
}

#[tokio::test]
async fn repeating_replays_after_drain() {
    let p = ScriptedProvider::repeating("ollama", [ScriptedStep::Text("ok".into())]);
    let a = p.complete(&req(), "", "m").await.unwrap();
    assert!(matches!(
        &a.content[0],
        ContentBlock::Text { text } if text == "ok"
    ));
    let b = p.complete(&req(), "", "m").await.unwrap();
    assert!(matches!(
        &b.content[0],
        ContentBlock::Text { text } if text == "ok"
    ));
}

#[tokio::test]
async fn batched_plays_one_vec_per_stream() {
    let p = ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::FailOpen("context_length_exceeded".into())],
            vec![ScriptedStep::Text("ok".into())],
        ],
    );
    assert!(p.stream(&req(), "", "m").await.is_err());
    let mut stream = p.stream(&req(), "", "m").await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert!(matches!(first, StreamEvent::TextDelta { text } if text == "ok"));
}

#[test]
fn lock_recovers_from_poison() {
    let p = ScriptedProvider::text("y");
    let handle = std::thread::scope(|s| {
        s.spawn(|| {
            let _g = p.steps.lock().unwrap();
            panic!("poison");
        })
        .join()
    });
    assert!(handle.is_err());
    assert_eq!(p.take_steps().len(), 1);
}

#[test]
fn batches_lock_recovers_from_poison() {
    let p = ScriptedProvider::batched("script", [vec![ScriptedStep::Text("y".into())]]);
    let handle = std::thread::scope(|s| {
        s.spawn(|| {
            let _g = p.batches.lock().unwrap();
            panic!("poison");
        })
        .join()
    });
    assert!(handle.is_err());
    assert_eq!(p.take_steps().len(), 1);
}
