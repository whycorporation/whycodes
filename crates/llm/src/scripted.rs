//! In-process [`LlmProvider`] that plays a scripted sequence of stream events.
//!
//! Used by agent / server / CLI tests so turns can be driven without HTTP.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use serde_json::Value;
use whycodes_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

/// One scripted action. Consumed in order by [`ScriptedProvider::stream`].
#[derive(Debug, Clone)]
pub enum ScriptedStep {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    /// Incremental tool-argument fragment (`StreamEvent::ToolUseDelta`).
    ToolUseDelta {
        id: String,
        input_json_delta: String,
    },
    Thinking(String),
    ThinkingDelta(String),
    ThinkingSignature(String),
    RedactedThinking(String),
    MessageStart,
    MessageDelta(Value),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    CacheUsage {
        creation_input_tokens: u64,
        read_input_tokens: u64,
    },
    Error(String),
    FailOpen(String),
    Hang(Duration),
}

/// Provider registered as `"script"` unless renamed.
pub struct ScriptedProvider {
    name: String,
    steps: Mutex<VecDeque<ScriptedStep>>,
    /// When non-empty, [`Self::take_steps`] refills from this after a drain
    /// so multi-turn tests do not get an empty stream on the second call.
    repeat: Vec<ScriptedStep>,
    /// One vector per `stream`/`complete` call. Consumed before `steps`/`repeat`.
    batches: Mutex<VecDeque<Vec<ScriptedStep>>>,
}

impl ScriptedProvider {
    pub fn new(steps: impl IntoIterator<Item = ScriptedStep>) -> Self {
        Self {
            name: "script".into(),
            steps: Mutex::new(steps.into_iter().collect()),
            repeat: Vec::new(),
            batches: Mutex::new(VecDeque::new()),
        }
    }

    pub fn named(name: impl Into<String>, steps: impl IntoIterator<Item = ScriptedStep>) -> Self {
        Self {
            name: name.into(),
            steps: Mutex::new(steps.into_iter().collect()),
            repeat: Vec::new(),
            batches: Mutex::new(VecDeque::new()),
        }
    }

    /// Like [`Self::named`], but each `stream`/`complete` replay the same steps.
    pub fn repeating(
        name: impl Into<String>,
        steps: impl IntoIterator<Item = ScriptedStep>,
    ) -> Self {
        let steps: Vec<ScriptedStep> = steps.into_iter().collect();
        Self {
            name: name.into(),
            steps: Mutex::new(steps.clone().into()),
            repeat: steps,
            batches: Mutex::new(VecDeque::new()),
        }
    }

    /// One inner vector is consumed per `stream`/`complete` (multi-step turn tests).
    pub fn batched(
        name: impl Into<String>,
        batches: impl IntoIterator<Item = Vec<ScriptedStep>>,
    ) -> Self {
        Self {
            name: name.into(),
            steps: Mutex::new(VecDeque::new()),
            repeat: Vec::new(),
            batches: Mutex::new(batches.into_iter().collect()),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new([ScriptedStep::Text(text.into())])
    }

    fn take_steps(&self) -> Vec<ScriptedStep> {
        let mut batches = self.batches.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(next) = batches.pop_front() {
            return next;
        }
        drop(batches);
        let mut guard = self.steps.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_empty() && !self.repeat.is_empty() {
            *guard = self.repeat.iter().cloned().collect();
        }
        guard.drain(..).collect()
    }
}

impl LlmProvider for ScriptedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }

    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            let mut text = String::new();
            let mut usage = Usage::default();
            for step in self.take_steps() {
                match step {
                    ScriptedStep::Text(t) => text.push_str(&t),
                    ScriptedStep::Thinking(_)
                    | ScriptedStep::ThinkingDelta(_)
                    | ScriptedStep::ThinkingSignature(_)
                    | ScriptedStep::RedactedThinking(_)
                    | ScriptedStep::ToolCall { .. }
                    | ScriptedStep::ToolUseDelta { .. }
                    | ScriptedStep::MessageStart
                    | ScriptedStep::MessageDelta(_)
                    | ScriptedStep::CacheUsage { .. } => {}
                    ScriptedStep::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        usage.input_tokens = input_tokens;
                        usage.output_tokens = output_tokens;
                    }
                    ScriptedStep::Error(msg) | ScriptedStep::FailOpen(msg) => {
                        return Err(whycodes_core::Error::Provider(msg));
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
        })
    }

    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move {
            let steps = self.take_steps();
            if let Some(ScriptedStep::FailOpen(msg)) = steps.first() {
                return Err(whycodes_core::Error::Provider(msg.clone()));
            }
            Ok(Box::pin(ScriptedStream {
                steps: steps.into(),
                hang: None,
                stopped: false,
            }) as ProviderEventStream)
        })
    }
}

struct ScriptedStream {
    steps: VecDeque<ScriptedStep>,
    hang: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    stopped: bool,
}

fn event_for_step(step: ScriptedStep) -> Option<StreamEvent> {
    match step {
        ScriptedStep::Text(text) => Some(StreamEvent::TextDelta { text }),
        ScriptedStep::ToolCall { id, name, input } => {
            Some(StreamEvent::ToolUse { id, name, input })
        }
        ScriptedStep::ToolUseDelta {
            id,
            input_json_delta,
        } => Some(StreamEvent::ToolUseDelta {
            id,
            input_json_delta,
        }),
        ScriptedStep::Thinking(text) => Some(StreamEvent::Thinking { text }),
        ScriptedStep::ThinkingDelta(text) => Some(StreamEvent::ThinkingDelta { text }),
        ScriptedStep::ThinkingSignature(signature) => {
            Some(StreamEvent::ThinkingSignature { signature })
        }
        ScriptedStep::RedactedThinking(data) => Some(StreamEvent::RedactedThinking { data }),
        ScriptedStep::MessageStart => Some(StreamEvent::MessageStart {
            message: Box::new(whycodes_core::types::Message {
                role: whycodes_core::types::Role::Assistant,
                content: whycodes_core::types::MessageContent::Text(String::new()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }),
        }),
        ScriptedStep::MessageDelta(delta) => Some(StreamEvent::MessageDelta { delta }),
        ScriptedStep::Usage {
            input_tokens,
            output_tokens,
        } => Some(StreamEvent::Usage {
            input_tokens,
            output_tokens,
        }),
        ScriptedStep::CacheUsage {
            creation_input_tokens,
            read_input_tokens,
        } => Some(StreamEvent::CacheUsage {
            creation_input_tokens,
            read_input_tokens,
        }),
        ScriptedStep::Error(message) => Some(StreamEvent::Error { message }),
        ScriptedStep::FailOpen(_) | ScriptedStep::Hang(_) => None,
    }
}

impl Stream for ScriptedStream {
    type Item = whycodes_core::Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(hang) = this.hang.as_mut() {
                match hang.as_mut().poll(cx) {
                    Poll::Ready(()) => this.hang = None,
                    Poll::Pending => return Poll::Pending,
                }
            }
            match this.steps.pop_front() {
                Some(ScriptedStep::Hang(d)) => {
                    if !d.is_zero() {
                        this.hang = Some(Box::pin(tokio::time::sleep(d)));
                    }
                }
                Some(step) => {
                    if let Some(ev) = event_for_step(step) {
                        return Poll::Ready(Some(Ok(ev)));
                    }
                }
                None if !this.stopped => {
                    this.stopped = true;
                    return Poll::Ready(Some(Ok(StreamEvent::MessageStop)));
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

#[cfg(test)]
#[path = "scripted_tests.rs"]
mod tests;
