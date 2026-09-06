//! First-token race failover.
//!
//! Open the primary stream immediately. If no meaningful token arrives within
//! `race_after`, start the backup model and take whichever emits first.
//! The loser is dropped (HTTP body cancelled). Opt-in: racing can bill both
//! prefills until cancel.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use futures::Stream;
use futures::StreamExt;
use tracing::{debug, warn};
use whycodes_core::types::{LlmRequest, StreamEvent};

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

pub type EventStream = Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>;

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
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    let Some(race) =
        race.filter(|r| r.model != primary.model || r.provider.name() != primary.provider.name())
    else {
        return open_primary_only(transport, primary, request).await;
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
            warn!("race: primary stream open failed: {e}");
            return open_partner_after_primary_fail(transport, race, request).await;
        }
    };

    match tokio::time::timeout(race_after, next_first_token(&mut primary_stream)).await {
        Ok(Some(Ok(ev))) => {
            debug!("race: primary first token model={}", primary.model);
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
        Err(elapsed) => {
            let after_ms = race_after.as_millis() as u64;
            debug!(
                "race: no primary token yet; starting partner after_ms={after_ms} error={elapsed}"
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
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    let (p, r) = tokio::join!(
        transport.stream(primary.provider, request, primary.api_key, primary.model),
        transport.stream(race.provider, request, race.api_key, race.model),
    );
    match (p, r) {
        (Ok(ps), Ok(rs)) => first_of_two(ps, rs).await,
        (Ok(ps), Err(e)) => {
            warn!("race: partner open failed: {e}");
            Ok((ps, RaceOutcome::Primary))
        }
        (Err(e), Ok(rs)) => {
            warn!("race: primary open failed: {e}");
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
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    let mut race_open = Some(Box::pin(transport.stream(
        race.provider,
        request,
        race.api_key,
        race.model,
    )));
    let mut race_stream: Option<EventStream> = None;
    let mut primary_dead: Option<whycodes_core::Error> = None;

    loop {
        enum AfterTimeout {
            Primary(Option<whycodes_core::Result<StreamEvent>>),
            Opened(whycodes_core::Result<EventStream>),
            Partner(Option<whycodes_core::Result<StreamEvent>>),
        }
        let event = poll_fn(|cx| {
            if primary_dead.is_none() {
                loop {
                    match Pin::new(&mut primary_stream).poll_next(cx) {
                        Poll::Ready(Some(Ok(ev))) if is_first_token(&ev) => {
                            return Poll::Ready(AfterTimeout::Primary(Some(Ok(ev))));
                        }
                        Poll::Ready(Some(Ok(_))) => {}
                        Poll::Ready(Some(Err(e))) => {
                            return Poll::Ready(AfterTimeout::Primary(Some(Err(e))));
                        }
                        Poll::Ready(None) => return Poll::Ready(AfterTimeout::Primary(None)),
                        Poll::Pending => break,
                    }
                }
            }
            if let Some(mut fut) = race_open.take() {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(r) => return Poll::Ready(AfterTimeout::Opened(r)),
                    Poll::Pending => race_open = Some(fut),
                }
            }
            if let Some(s) = race_stream.as_mut() {
                loop {
                    match Pin::new(&mut *s).poll_next(cx) {
                        Poll::Ready(Some(Ok(ev))) if is_first_token(&ev) => {
                            return Poll::Ready(AfterTimeout::Partner(Some(Ok(ev))));
                        }
                        Poll::Ready(Some(Ok(_))) => {}
                        Poll::Ready(Some(Err(e))) => {
                            return Poll::Ready(AfterTimeout::Partner(Some(Err(e))));
                        }
                        Poll::Ready(None) => return Poll::Ready(AfterTimeout::Partner(None)),
                        Poll::Pending => break,
                    }
                }
            }
            Poll::Pending
        })
        .await;
        match event {
            AfterTimeout::Primary(p) => match p {
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
                    let e = whycodes_core::Error::Provider(
                        "primary stream ended before first token".into(),
                    );
                    if race_open.is_none() && race_stream.is_none() {
                        return Err(e);
                    }
                    primary_dead = Some(e);
                }
            },
            AfterTimeout::Opened(opened) => match opened {
                Ok(s) => race_stream = Some(s),
                Err(e) => {
                    warn!("race: partner open failed after timeout: {e}");
                    if let Some(pe) = take_primary_dead(&mut primary_dead) {
                        return Err(pe);
                    }
                }
            },
            AfterTimeout::Partner(r) => match r {
                Some(Ok(ev)) => {
                    drop(primary_stream);
                    return prefix_partner(ev, race_stream.take());
                }
                Some(Err(e)) => {
                    race_stream = None;
                    if let Some(pe) = take_primary_dead(&mut primary_dead) {
                        return Err(pe);
                    }
                    warn!("race: partner stream failed: {e}");
                }
                None => {
                    race_stream = None;
                    if let Some(pe) = take_primary_dead(&mut primary_dead) {
                        return Err(pe);
                    }
                }
            },
        }
    }
}

async fn first_of_two(
    mut primary: EventStream,
    mut race: EventStream,
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    match first_ready2(next_first_token(&mut primary), next_first_token(&mut race)).await {
        Ready2::A(p) => match p {
            Some(Ok(ev)) => {
                drop(race);
                Ok((prefix_stream(ev, primary), RaceOutcome::Primary))
            }
            Some(Err(e)) => match next_first_token(&mut race).await {
                Some(Ok(ev)) => Ok((
                    prefix_stream(ev, race),
                    RaceOutcome::Race {
                        reason: "primary_error",
                    },
                )),
                Some(Err(_)) | None => Err(e),
            },
            None => match next_first_token(&mut race).await {
                Some(Ok(ev)) => Ok((
                    prefix_stream(ev, race),
                    RaceOutcome::Race {
                        reason: "primary_empty",
                    },
                )),
                Some(Err(e)) => Err(e),
                None => Err(whycodes_core::Error::Provider(
                    "both race streams empty".into(),
                )),
            },
        },
        Ready2::B(r) => match r {
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
                None => Err(whycodes_core::Error::Provider(
                    "both race streams empty".into(),
                )),
            },
        },
    }
}

enum Ready2<A, B> {
    A(A),
    B(B),
}

async fn first_ready2<FA, FB, A, B>(a: FA, b: FB) -> Ready2<A, B>
where
    FA: Future<Output = A>,
    FB: Future<Output = B>,
{
    tokio::pin!(a);
    tokio::pin!(b);
    poll_fn(move |cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(Ready2::A(v));
        }
        if let Poll::Ready(v) = b.as_mut().poll(cx) {
            return Poll::Ready(Ready2::B(v));
        }
        Poll::Pending
    })
    .await
}

async fn next_first_token(s: &mut EventStream) -> Option<whycodes_core::Result<StreamEvent>> {
    while let Some(item) = s.next().await {
        match item {
            Ok(ev) if is_first_token(&ev) => return Some(Ok(ev)),
            Ok(_) => continue,
            Err(e) => return Some(Err(e)),
        }
    }
    None
}

pub(crate) fn prefix_partner(
    first: StreamEvent,
    race_stream: Option<EventStream>,
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    let Some(rs) = race_stream else {
        return Err(whycodes_core::Error::Provider(
            "partner stream missing after first token".into(),
        ));
    };
    Ok((
        prefix_stream(first, rs),
        RaceOutcome::Race {
            reason: "first_token",
        },
    ))
}

async fn open_primary_only(
    transport: &LlmTransport,
    primary: StreamTarget<'_>,
    request: &LlmRequest,
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    let s = transport
        .stream(primary.provider, request, primary.api_key, primary.model)
        .await?;
    Ok((s, RaceOutcome::PrimaryOnly))
}

async fn open_partner_after_primary_fail(
    transport: &LlmTransport,
    race: StreamTarget<'_>,
    request: &LlmRequest,
) -> whycodes_core::Result<(EventStream, RaceOutcome)> {
    let s = transport
        .stream(race.provider, request, race.api_key, race.model)
        .await?;
    Ok((
        s,
        RaceOutcome::Race {
            reason: "primary_open_failed",
        },
    ))
}

fn take_primary_dead(
    primary_dead: &mut Option<whycodes_core::Error>,
) -> Option<whycodes_core::Error> {
    primary_dead.take()
}

fn prefix_stream(first: StreamEvent, rest: EventStream) -> EventStream {
    Box::pin(PrefixStream {
        first: Some(first),
        rest,
    })
}

struct PrefixStream {
    first: Option<StreamEvent>,
    rest: EventStream,
}

impl Stream for PrefixStream {
    type Item = whycodes_core::Result<StreamEvent>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(ev) = this.first.take() {
            return Poll::Ready(Some(Ok(ev)));
        }
        this.rest.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
#[path = "race_tests.rs"]
mod tests;
