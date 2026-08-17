//! In-process [`LlmProvider`] that plays a scripted sequence of stream events.
//!
//! Used by agent / server / CLI tests so turns can be driven without HTTP.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use crate::provider::LlmProvider;

type EventStream = Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>;

/// One scripted action. Consumed in order by [`ScriptedProvider::stream`].
#[derive(Debug, Clone)]
pub enum ScriptedStep {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    Thinking(String),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Error(String),
    FailOpen(String),
    Hang(Duration),
}

/// Provider registered as `"script"` unless renamed.
pub struct ScriptedProvider {
    name: String,
    steps: Mutex<VecDeque<ScriptedStep>>,
}

impl ScriptedProvider {
    pub fn new(steps: impl IntoIterator<Item = ScriptedStep>) -> Self {
        Self {
            name: "script".into(),
            steps: Mutex::new(steps.into_iter().collect()),
        }
    }

    pub fn named(name: impl Into<String>, steps: impl IntoIterator<Item = ScriptedStep>) -> Self {
        Self {
            name: name.into(),
            steps: Mutex::new(steps.into_iter().collect()),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new([ScriptedStep::Text(text.into())])
    }

    fn take_steps(&self) -> Vec<ScriptedStep> {
        self.steps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }

    async fn complete(
        &self,
        _request: &LlmRequest,
        _api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
        let mut text = String::new();
        let mut usage = Usage::default();
        for step in self.take_steps() {
            match step {
                ScriptedStep::Text(t) => text.push_str(&t),
                ScriptedStep::Thinking(_) | ScriptedStep::ToolCall { .. } => {}
                ScriptedStep::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    usage.input_tokens = input_tokens;
                    usage.output_tokens = output_tokens;
                }
                ScriptedStep::Error(msg) | ScriptedStep::FailOpen(msg) => {
                    return Err(whycode_core::Error::Provider(msg));
                }
                ScriptedStep::Hang(d) => tokio::time::sleep(d).await,
            }
        }
        Ok(LlmResponse {
            content: if text.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text { text }]
            },
            stop_reason: Some("end_turn".into()),
            usage,
            model: model.into(),
        })
    }

    async fn stream(
        &self,
        _request: &LlmRequest,
        _api_key: &str,
        _model: &str,
    ) -> whycode_core::Result<EventStream> {
        let steps = self.take_steps();
        if let Some(ScriptedStep::FailOpen(msg)) = steps.first() {
            return Err(whycode_core::Error::Provider(msg.clone()));
        }
        Ok(Box::pin(async_stream::stream! {
            for step in steps {
                match step {
                    ScriptedStep::Text(text) => {
                        yield Ok(StreamEvent::TextDelta { text });
                    }
                    ScriptedStep::ToolCall { id, name, input } => {
                        yield Ok(StreamEvent::ToolUse { id, name, input });
                    }
                    ScriptedStep::Thinking(text) => {
                        yield Ok(StreamEvent::Thinking { text });
                    }
                    ScriptedStep::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        yield Ok(StreamEvent::Usage {
                            input_tokens,
                            output_tokens,
                        });
                    }
                    ScriptedStep::Error(message) => {
                        yield Ok(StreamEvent::Error { message });
                    }
                    ScriptedStep::FailOpen(_) => {}
                    ScriptedStep::Hang(d) => {
                        if !d.is_zero() {
                            tokio::time::sleep(d).await;
                        }
                    }
                }
            }
            yield Ok(StreamEvent::MessageStop);
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;
    use whycode_core::types::{LlmRequest, Message, MessageContent, Role};

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
            tools: vec![],
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
    async fn fail_open_errors_before_stream() {
        let p = ScriptedProvider::new([ScriptedStep::FailOpen("nope".into())]);
        assert!(p.stream(&req(), "", "m").await.is_err());
    }

    #[tokio::test]
    async fn complete_joins_text_and_reports_usage() {
        let p = ScriptedProvider::text("hello");
        let out = p.complete(&req(), "", "model-x").await.unwrap();
        assert_eq!(out.model, "model-x");
        match &out.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            other => panic!("{other:?}"),
        }
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
}
