//! First-token race failover.
//!
//! Open the primary stream immediately. If no meaningful token arrives within
//! `race_after`, start the backup model and take whichever emits first.
//! The loser is dropped (HTTP body cancelled). Opt-in: racing can bill both
//! prefills until cancel.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use futures::StreamExt;
use tracing::{debug, warn};
use whycode_core::types::{LlmRequest, StreamEvent};

use crate::provider::LlmProvider;
use crate::transport::LlmTransport;

/// One side of a race (provider + key + model).
pub struct StreamTarget<'a> {
    pub provider: &'a dyn LlmProvider,
    pub api_key: &'a str,
    pub model: &'a str,
}

/// Who produced the first token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaceOutcome {
    /// No partner, or partner was the same model.
    PrimaryOnly,
    /// Primary emitted first (race never started, or lost).
    Primary,
    /// Backup won. `reason` is a short tag for JSONL.
    Race { reason: &'static str },
}

impl RaceOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PrimaryOnly => "primary_only",
            Self::Primary => "primary",
            Self::Race { reason } => reason,
        }
    }

    pub fn raced(&self) -> bool {
        matches!(self, Self::Race { .. })
    }
}

pub type EventStream = Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>;

/// First event that counts as TTFT (text, thinking, or a tool call).
pub fn is_first_token(ev: &StreamEvent) -> bool {
    match ev {
        StreamEvent::TextDelta { text } => !text.is_empty(),
        StreamEvent::Thinking { text } | StreamEvent::ThinkingDelta { text } => !text.is_empty(),
        StreamEvent::ToolUse { .. } | StreamEvent::ToolUseDelta { .. } => true,
        _ => false,
    }
}

/// Open primary (and optionally race a backup after `race_after`).
pub async fn stream_raced(
    transport: &LlmTransport,
    primary: StreamTarget<'_>,
    race: Option<StreamTarget<'_>>,
    request: &LlmRequest,
    race_after: Duration,
) -> whycode_core::Result<(EventStream, RaceOutcome)> {
    let Some(race) =
        race.filter(|r| r.model != primary.model || r.provider.name() != primary.provider.name())
    else {
        let s = transport
            .stream(primary.provider, request, primary.api_key, primary.model)
            .await?;
        return Ok((s, RaceOutcome::PrimaryOnly));
    };

    if race_after.is_zero() {
        return race_immediate(transport, primary, race, request).await;
    }

    let mut primary_stream = match transport
        .stream(primary.provider, request, primary.api_key, primary.model)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "race: primary stream open failed");
            let s = transport
                .stream(race.provider, request, race.api_key, race.model)
                .await?;
            return Ok((
                s,
                RaceOutcome::Race {
                    reason: "primary_open_failed",
                },
            ));
        }
    };

    match tokio::time::timeout(race_after, next_first_token(&mut primary_stream)).await {
        Ok(Some(Ok(ev))) => {
            debug!(model = primary.model, "race: primary first token");
            return Ok((prefix_stream(ev, primary_stream), RaceOutcome::Primary));
        }
        Ok(Some(Err(e))) => return Err(e),
        Ok(None) => {
            warn!("race: primary closed with no token; opening partner");
            let s = transport
                .stream(race.provider, request, race.api_key, race.model)
                .await?;
            return Ok((
                s,
                RaceOutcome::Race {
                    reason: "primary_empty",
                },
            ));
        }
        Err(_) => {
            debug!(
                after_ms = race_after.as_millis() as u64,
                "race: no primary token yet; starting partner"
            );
        }
    }

    race_after_timeout(transport, primary_stream, race, request).await
}

async fn race_immediate(
    transport: &LlmTransport,
    primary: StreamTarget<'_>,
    race: StreamTarget<'_>,
    request: &LlmRequest,
) -> whycode_core::Result<(EventStream, RaceOutcome)> {
    let (p, r) = tokio::join!(
        transport.stream(primary.provider, request, primary.api_key, primary.model),
        transport.stream(race.provider, request, race.api_key, race.model),
    );
    match (p, r) {
        (Ok(ps), Ok(rs)) => first_of_two(ps, rs).await,
        (Ok(ps), Err(e)) => {
            warn!(error = %e, "race: partner open failed");
            Ok((ps, RaceOutcome::Primary))
        }
        (Err(e), Ok(rs)) => {
            warn!(error = %e, "race: primary open failed");
            Ok((
                rs,
                RaceOutcome::Race {
                    reason: "primary_open_failed",
                },
            ))
        }
        (Err(e), Err(_)) => Err(e),
    }
}

async fn race_after_timeout(
    transport: &LlmTransport,
    mut primary_stream: EventStream,
    race: StreamTarget<'_>,
    request: &LlmRequest,
) -> whycode_core::Result<(EventStream, RaceOutcome)> {
    let mut race_open = Some(Box::pin(transport.stream(
        race.provider,
        request,
        race.api_key,
        race.model,
    )));
    let mut race_stream: Option<EventStream> = None;
    let mut primary_dead: Option<whycode_core::Error> = None;

    loop {
        tokio::select! {
            p = next_first_token(&mut primary_stream), if primary_dead.is_none() => {
                match p {
                    Some(Ok(ev)) => {
                        drop(race_open);
                        drop(race_stream);
                        return Ok((prefix_stream(ev, primary_stream), RaceOutcome::Primary));
                    }
                    Some(Err(e)) => {
                        if race_open.is_none() && race_stream.is_none() {
                            return Err(e);
                        }
                        primary_dead = Some(e);
                    }
                    None => {
                        let e = whycode_core::Error::Provider(
                            "primary stream ended before first token".into(),
                        );
                        if race_open.is_none() && race_stream.is_none() {
                            return Err(e);
                        }
                        primary_dead = Some(e);
                    }
                }
            }
            opened = await_opt_open(&mut race_open) => {
                match opened {
                    Some(Ok(s)) => race_stream = Some(s),
                    Some(Err(e)) => {
                        warn!(error = %e, "race: partner open failed after timeout");
                        if let Some(pe) = primary_dead.take() {
                            return Err(pe);
                        }
                    }
                    None => {}
                }
            }
            r = await_opt_first_token(&mut race_stream) => {
                match r {
                    Some(Ok(ev)) => {
                        let Some(rs) = race_stream.take() else {
                            return Err(whycode_core::Error::Provider(
                                "race stream missing after first token".into(),
                            ));
                        };
                        drop(primary_stream);
                        return Ok((
                            prefix_stream(ev, rs),
                            RaceOutcome::Race {
                                reason: "first_token",
                            },
                        ));
                    }
                    Some(Err(e)) => {
                        race_stream = None;
                        if let Some(pe) = primary_dead.take() {
                            return Err(pe);
                        }
                        warn!(error = %e, "race: partner stream failed");
                    }
                    None => {
                        if race_stream.is_some() {
                            race_stream = None;
                            if let Some(pe) = primary_dead.take() {
                                return Err(pe);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Pending when `open` is `None`, so `select!` never needs `unwrap`.
async fn await_opt_open<F>(
    open: &mut Option<Pin<Box<F>>>,
) -> Option<whycode_core::Result<EventStream>>
where
    F: Future<Output = whycode_core::Result<EventStream>> + ?Sized,
{
    match open.as_mut() {
        Some(fut) => {
            let r = fut.await;
            *open = None;
            Some(r)
        }
        None => std::future::pending().await,
    }
}

async fn await_opt_first_token(
    stream: &mut Option<EventStream>,
) -> Option<whycode_core::Result<StreamEvent>> {
    match stream.as_mut() {
        Some(s) => next_first_token(s).await,
        None => std::future::pending().await,
    }
}

async fn first_of_two(
    mut primary: EventStream,
    mut race: EventStream,
) -> whycode_core::Result<(EventStream, RaceOutcome)> {
    tokio::select! {
        p = next_first_token(&mut primary) => {
            match p {
                Some(Ok(ev)) => {
                    drop(race);
                    Ok((prefix_stream(ev, primary), RaceOutcome::Primary))
                }
                Some(Err(e)) => {
                    match next_first_token(&mut race).await {
                        Some(Ok(ev)) => Ok((
                            prefix_stream(ev, race),
                            RaceOutcome::Race {
                                reason: "primary_error",
                            },
                        )),
                        Some(Err(_)) => Err(e),
                        None => Err(e),
                    }
                }
                None => match next_first_token(&mut race).await {
                    Some(Ok(ev)) => Ok((
                        prefix_stream(ev, race),
                        RaceOutcome::Race {
                            reason: "primary_empty",
                        },
                    )),
                    Some(Err(e)) => Err(e),
                    None => Err(whycode_core::Error::Provider(
                        "both race streams empty".into(),
                    )),
                },
            }
        }
        r = next_first_token(&mut race) => {
            match r {
                Some(Ok(ev)) => {
                    drop(primary);
                    Ok((
                        prefix_stream(ev, race),
                        RaceOutcome::Race {
                            reason: "first_token",
                        },
                    ))
                }
                Some(Err(_)) | None => match next_first_token(&mut primary).await {
                    Some(Ok(ev)) => Ok((prefix_stream(ev, primary), RaceOutcome::Primary)),
                    Some(Err(e)) => Err(e),
                    None => Err(whycode_core::Error::Provider(
                        "both race streams empty".into(),
                    )),
                },
            }
        }
    }
}

async fn next_first_token(s: &mut EventStream) -> Option<whycode_core::Result<StreamEvent>> {
    while let Some(item) = s.next().await {
        match item {
            Ok(ev) if is_first_token(&ev) => return Some(Ok(ev)),
            Ok(_) => continue,
            Err(e) => return Some(Err(e)),
        }
    }
    None
}

fn prefix_stream(first: StreamEvent, rest: EventStream) -> EventStream {
    Box::pin(async_stream::stream! {
        yield Ok(first);
        let mut rest = rest;
        while let Some(ev) = rest.next().await {
            yield ev;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use whycode_core::types::{LlmResponse, Message, MessageContent, Role, Usage};

    use crate::retry::RetryPolicy;

    struct DelayProvider {
        name: String,
        delay: Duration,
        text: String,
        opens: Arc<AtomicUsize>,
        fail_open: bool,
    }

    #[async_trait]
    impl LlmProvider for DelayProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn default_base_url(&self) -> &str {
            "http://example.invalid"
        }
        async fn complete(
            &self,
            _request: &LlmRequest,
            _api_key: &str,
            model: &str,
        ) -> whycode_core::Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![],
                stop_reason: None,
                usage: Usage::default(),
                model: model.into(),
            })
        }
        async fn stream(
            &self,
            _request: &LlmRequest,
            _api_key: &str,
            _model: &str,
        ) -> whycode_core::Result<EventStream> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if self.fail_open {
                return Err(whycode_core::Error::Provider("boom".into()));
            }
            let delay = self.delay;
            let text = self.text.clone();
            Ok(Box::pin(async_stream::stream! {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                yield Ok(StreamEvent::TextDelta { text });
                yield Ok(StreamEvent::MessageStop);
            }))
        }
    }

    fn req() -> LlmRequest {
        LlmRequest {
            system: "s".into(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
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

    fn transport() -> LlmTransport {
        LlmTransport {
            retry: RetryPolicy {
                max_retries: 0,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(1),
                max_elapsed: Duration::from_secs(2),
                full_jitter: false,
            },
            complete_timeout: None,
        }
    }

    async fn collect_text(mut s: EventStream) -> String {
        let mut out = String::new();
        while let Some(ev) = s.next().await {
            if let Ok(StreamEvent::TextDelta { text }) = ev {
                out.push_str(&text);
            }
        }
        out
    }

    #[tokio::test]
    async fn fast_primary_never_opens_partner() {
        let p_opens = Arc::new(AtomicUsize::new(0));
        let r_opens = Arc::new(AtomicUsize::new(0));
        let primary = DelayProvider {
            name: "p".into(),
            delay: Duration::from_millis(5),
            text: "primary".into(),
            opens: Arc::clone(&p_opens),
            fail_open: false,
        };
        let race = DelayProvider {
            name: "r".into(),
            delay: Duration::from_millis(5),
            text: "race".into(),
            opens: Arc::clone(&r_opens),
            fail_open: false,
        };
        let t = transport();
        let req = req();
        let (s, outcome) = stream_raced(
            &t,
            StreamTarget {
                provider: &primary,
                api_key: "",
                model: "sonnet",
            },
            Some(StreamTarget {
                provider: &race,
                api_key: "",
                model: "haiku",
            }),
            &req,
            Duration::from_millis(200),
        )
        .await
        .unwrap();
        assert_eq!(collect_text(s).await, "primary");
        assert_eq!(outcome, RaceOutcome::Primary);
        assert_eq!(p_opens.load(Ordering::SeqCst), 1);
        assert_eq!(r_opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn slow_primary_loses_to_partner() {
        let primary = DelayProvider {
            name: "p".into(),
            delay: Duration::from_millis(400),
            text: "primary".into(),
            opens: Arc::new(AtomicUsize::new(0)),
            fail_open: false,
        };
        let race = DelayProvider {
            name: "r".into(),
            delay: Duration::from_millis(5),
            text: "haiku".into(),
            opens: Arc::new(AtomicUsize::new(0)),
            fail_open: false,
        };
        let t = transport();
        let req = req();
        let (s, outcome) = stream_raced(
            &t,
            StreamTarget {
                provider: &primary,
                api_key: "",
                model: "sonnet",
            },
            Some(StreamTarget {
                provider: &race,
                api_key: "",
                model: "haiku",
            }),
            &req,
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert_eq!(collect_text(s).await, "haiku");
        assert_eq!(
            outcome,
            RaceOutcome::Race {
                reason: "first_token"
            }
        );
    }

    #[tokio::test]
    async fn primary_open_fail_uses_partner() {
        let primary = DelayProvider {
            name: "p".into(),
            delay: Duration::ZERO,
            text: "primary".into(),
            opens: Arc::new(AtomicUsize::new(0)),
            fail_open: true,
        };
        let race = DelayProvider {
            name: "r".into(),
            delay: Duration::ZERO,
            text: "backup".into(),
            opens: Arc::new(AtomicUsize::new(0)),
            fail_open: false,
        };
        let t = transport();
        let req = req();
        let (s, outcome) = stream_raced(
            &t,
            StreamTarget {
                provider: &primary,
                api_key: "",
                model: "sonnet",
            },
            Some(StreamTarget {
                provider: &race,
                api_key: "",
                model: "haiku",
            }),
            &req,
            Duration::from_millis(50),
        )
        .await
        .unwrap();
        assert_eq!(collect_text(s).await, "backup");
        assert_eq!(
            outcome,
            RaceOutcome::Race {
                reason: "primary_open_failed"
            }
        );
    }

    #[tokio::test]
    async fn same_model_skips_race() {
        let opens = Arc::new(AtomicUsize::new(0));
        let p = DelayProvider {
            name: "same".into(),
            delay: Duration::ZERO,
            text: "only".into(),
            opens: Arc::clone(&opens),
            fail_open: false,
        };
        let t = transport();
        let req = req();
        let (s, outcome) = stream_raced(
            &t,
            StreamTarget {
                provider: &p,
                api_key: "",
                model: "haiku",
            },
            Some(StreamTarget {
                provider: &p,
                api_key: "",
                model: "haiku",
            }),
            &req,
            Duration::from_millis(0),
        )
        .await
        .unwrap();
        assert_eq!(collect_text(s).await, "only");
        assert_eq!(outcome, RaceOutcome::PrimaryOnly);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
    }
}
