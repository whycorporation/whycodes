use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use whycodes_core::types::{LlmResponse, Message, MessageContent, Role, Usage};

use crate::provider::{ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture};
use crate::retry::RetryPolicy;

struct DelayProvider {
    name: String,
    delay: Duration,
    text: String,
    opens: Arc<AtomicUsize>,
    fail_open: bool,
    fail_mid: bool,
    hang: bool,
    thinking: bool,
    tool_use: bool,
}

impl LlmProvider for DelayProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn default_base_url(&self) -> &str {
        "http://example.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            Ok(LlmResponse {
                content: vec![],
                stop_reason: None,
                usage: Usage::default(),
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
            self.opens.fetch_add(1, Ordering::SeqCst);
            if self.fail_open {
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                return Err(whycodes_core::Error::Provider("boom".into()));
            }
            if self.hang {
                return Ok(Box::pin(futures::stream::pending()) as ProviderEventStream);
            }
            let delay = self.delay;
            let text = self.text.clone();
            let fail_mid = self.fail_mid;
            let thinking = self.thinking;
            let tool_use = self.tool_use;
            Ok(Box::pin(async_stream::stream! {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if fail_mid {
                    yield Err(whycodes_core::Error::Provider("mid".into()));
                    return;
                }
                if thinking {
                    yield Ok(StreamEvent::Thinking { text });
                } else if tool_use {
                    yield Ok(StreamEvent::ToolUse {
                        id: "1".into(),
                        name: "read".into(),
                        input: serde_json::json!({}),
                    });
                } else {
                    yield Ok(StreamEvent::TextDelta { text });
                }
                yield Ok(StreamEvent::MessageStop);
            }) as ProviderEventStream)
        })
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
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let race = DelayProvider {
        name: "r".into(),
        delay: Duration::from_millis(5),
        text: "race".into(),
        opens: Arc::clone(&r_opens),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
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
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let race = DelayProvider {
        name: "r".into(),
        delay: Duration::from_millis(5),
        text: "haiku".into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
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
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let race = DelayProvider {
        name: "r".into(),
        delay: Duration::ZERO,
        text: "backup".into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
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
async fn empty_primary_uses_partner() {
    let primary = DelayProvider {
        name: "p".into(),
        delay: Duration::ZERO,
        text: String::new(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let race = DelayProvider {
        name: "r".into(),
        delay: Duration::ZERO,
        text: "backup".into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "primary_empty"
        }
    );
}

#[tokio::test]
async fn immediate_race_keeps_primary_when_partner_open_fails() {
    let primary = DelayProvider {
        name: "p".into(),
        delay: Duration::ZERO,
        text: "primary".into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let race = DelayProvider {
        name: "r".into(),
        delay: Duration::ZERO,
        text: "unused".into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: true,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
    };
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .unwrap();

    assert_eq!(collect_text(s).await, "primary");
    assert_eq!(outcome, RaceOutcome::Primary);
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
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
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

#[allow(clippy::too_many_arguments)]
fn delay(
    name: &str,
    delay: Duration,
    text: &str,
    fail_open: bool,
    fail_mid: bool,
    hang: bool,
    thinking: bool,
    tool_use: bool,
) -> DelayProvider {
    DelayProvider {
        name: name.into(),
        delay,
        text: text.into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open,
        fail_mid,
        hang,
        thinking,
        tool_use,
    }
}

#[test]
fn race_outcome_as_str_and_is_first_token() {
    assert_eq!(RaceOutcome::PrimaryOnly.as_str(), "primary_only");
    assert_eq!(RaceOutcome::Primary.as_str(), "primary");
    assert_eq!(
        RaceOutcome::Race {
            reason: "first_token"
        }
        .as_str(),
        "first_token"
    );
    assert!(!RaceOutcome::Primary.raced());
    assert!(
        RaceOutcome::Race {
            reason: "first_token"
        }
        .raced()
    );
    assert!(is_first_token(&StreamEvent::Thinking {
        text: "plan".into()
    }));
    assert!(is_first_token(&StreamEvent::ThinkingDelta {
        text: "p".into()
    }));
    assert!(is_first_token(&StreamEvent::ToolUse {
        id: "1".into(),
        name: "read".into(),
        input: serde_json::json!({}),
    }));
    assert!(is_first_token(&StreamEvent::ToolUseDelta {
        id: "1".into(),
        input_json_delta: "{}".into(),
    }));
    assert!(!is_first_token(&StreamEvent::TextDelta {
        text: String::new()
    }));
    assert!(!is_first_token(&StreamEvent::MessageStop));
    assert!(!is_first_token(&StreamEvent::Thinking {
        text: String::new()
    }));
}

#[tokio::test]
async fn immediate_race_primary_wins_when_both_ok() {
    let primary = delay(
        "p",
        Duration::ZERO,
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::from_millis(50),
        "race",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "primary");
    assert_eq!(outcome, RaceOutcome::Primary);
}

#[tokio::test]
async fn immediate_race_partner_wins_on_first_token() {
    let primary = delay(
        "p",
        Duration::from_millis(80),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::ZERO,
        "haiku",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
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
async fn immediate_race_uses_partner_when_primary_open_fails() {
    let primary = delay(
        "p",
        Duration::ZERO,
        "primary",
        true,
        false,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::ZERO,
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
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
async fn immediate_race_errors_when_both_open_fail() {
    let primary = delay("p", Duration::ZERO, "p", true, false, false, false, false);
    let race = delay("r", Duration::ZERO, "r", true, false, false, false, false);
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
}

#[tokio::test]
async fn immediate_race_uses_partner_when_primary_errors_mid_stream() {
    let primary = delay("p", Duration::ZERO, "p", false, true, false, false, false);
    let race = delay(
        "r",
        Duration::from_millis(5),
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "primary_error"
        }
    );
}

#[tokio::test]
async fn immediate_race_uses_partner_when_primary_is_empty() {
    let primary = delay("p", Duration::ZERO, "", false, false, false, false, false);
    // Partner must be slower so primary drains to empty before the race
    // token arrives; otherwise `select!` can report `first_token`.
    let race = delay(
        "r",
        Duration::from_millis(20),
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "primary_empty"
        }
    );
}

#[tokio::test]
async fn immediate_race_errors_when_both_empty() {
    let primary = delay("p", Duration::ZERO, "", false, false, false, false, false);
    let race = delay("r", Duration::ZERO, "", false, false, false, false, false);
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(err.to_string().contains("both race streams empty"), "{err}");
}

#[tokio::test]
async fn immediate_race_keeps_primary_when_partner_fails_mid_stream() {
    let primary = delay(
        "p",
        Duration::from_millis(20),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay("r", Duration::ZERO, "r", false, true, false, false, false);
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "primary");
    assert_eq!(outcome, RaceOutcome::Primary);
}

#[tokio::test]
async fn timeout_primary_still_wins_after_partner_starts() {
    let primary = delay(
        "p",
        Duration::from_millis(30),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::from_millis(200),
        "haiku",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "primary");
    assert_eq!(outcome, RaceOutcome::Primary);
}

#[tokio::test]
async fn timeout_partner_open_fail_keeps_hanging_primary() {
    let primary = delay(
        "p",
        Duration::from_millis(40),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::ZERO,
        "unused",
        true,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "primary");
    assert_eq!(outcome, RaceOutcome::Primary);
}

#[tokio::test]
async fn timeout_primary_error_uses_partner() {
    let primary = delay(
        "p",
        Duration::from_millis(30),
        "p",
        false,
        true,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::ZERO,
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    );
}

#[tokio::test]
async fn timeout_partner_stream_fail_keeps_primary() {
    let primary = delay(
        "p",
        Duration::from_millis(40),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay("r", Duration::ZERO, "r", false, true, false, false, false);
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "primary");
    assert_eq!(outcome, RaceOutcome::Primary);
}

#[tokio::test]
async fn thinking_and_tool_use_count_as_first_token() {
    let primary = delay(
        "p",
        Duration::ZERO,
        "plan",
        false,
        false,
        false,
        true,
        false,
    );
    let race = delay(
        "r",
        Duration::from_millis(50),
        "unused",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(200),
    )
    .await
    .unwrap();
    let mut saw_thinking = false;
    let mut rest = s;
    while let Some(ev) = rest.next().await {
        if let Ok(StreamEvent::Thinking { text }) = ev {
            saw_thinking = text == "plan";
        }
    }
    assert!(saw_thinking);
    assert_eq!(outcome, RaceOutcome::Primary);

    let primary = delay("p", Duration::ZERO, "x", false, false, false, false, true);
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(200),
    )
    .await
    .unwrap();
    let mut saw_tool = false;
    let mut rest = s;
    while let Some(ev) = rest.next().await {
        if let Ok(StreamEvent::ToolUse { name, .. }) = ev {
            saw_tool = name == "read";
        }
    }
    assert!(saw_tool);
    assert_eq!(outcome, RaceOutcome::Primary);
}

#[tokio::test]
async fn no_race_when_partner_is_none() {
    let primary = delay(
        "p",
        Duration::ZERO,
        "only",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
        StreamTarget {
            provider: &primary,
            api_key: "",
            model: "sonnet",
        },
        None,
        &req(),
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "only");
    assert_eq!(outcome, RaceOutcome::PrimaryOnly);
}

#[tokio::test]
async fn delay_provider_complete_and_default_url() {
    let p = delay("p", Duration::ZERO, "x", false, false, false, false, false);
    assert_eq!(p.default_base_url(), "http://example.invalid");
    let resp = p.complete(&req(), "", "m").await.unwrap();
    assert_eq!(resp.model, "m");
}

#[test]
fn prefix_partner_errors_when_stream_is_missing() {
    let err = prefix_partner(StreamEvent::MessageStop, None)
        .map(|_| ())
        .unwrap_err();
    assert!(err.to_string().contains("partner stream missing"), "{err}");
}

#[tokio::test]
async fn prefix_partner_keeps_first_token_then_rest() {
    let rest: EventStream = Box::pin(futures::stream::iter([Ok(StreamEvent::MessageStop)]));
    let (mut s, outcome) =
        prefix_partner(StreamEvent::TextDelta { text: "hi".into() }, Some(rest)).unwrap();
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    );
    assert!(matches!(
        s.next().await,
        Some(Ok(StreamEvent::TextDelta { text })) if text == "hi"
    ));
    assert!(matches!(s.next().await, Some(Ok(StreamEvent::MessageStop))));
}

#[tokio::test]
async fn timeout_primary_mid_error_inside_window_returns_err() {
    let primary = delay(
        "p",
        Duration::from_millis(5),
        "p",
        false,
        true,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::from_millis(200),
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(200),
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(err.to_string().contains("mid"), "{err}");
}

#[tokio::test]
async fn immediate_race_errors_when_both_fail_mid() {
    let primary = delay("p", Duration::ZERO, "p", false, true, false, false, false);
    let race = delay(
        "r",
        Duration::from_millis(5),
        "r",
        false,
        true,
        false,
        false,
        false,
    );
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("mid") || !err.to_string().is_empty(),
        "{err}"
    );
}

#[tokio::test]
async fn immediate_race_empty_primary_then_partner_fail_mid() {
    let primary = delay("p", Duration::ZERO, "", false, false, false, false, false);
    let race = delay(
        "r",
        Duration::from_millis(5),
        "r",
        false,
        true,
        false,
        false,
        false,
    );
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("mid") || !err.to_string().is_empty(),
        "{err}"
    );
}

#[tokio::test]
async fn immediate_race_partner_fail_then_primary_empty() {
    let primary = delay(
        "p",
        Duration::from_millis(20),
        "",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay("r", Duration::ZERO, "r", false, true, false, false, false);
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::ZERO,
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("empty") || err.to_string().contains("mid"),
        "{err}"
    );
}

#[tokio::test]
async fn timeout_primary_empty_while_partner_opens() {
    let primary = delay(
        "p",
        Duration::from_millis(30),
        "",
        false,
        false,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::from_millis(5),
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    );
}

#[tokio::test]
async fn timeout_primary_dead_and_partner_open_fail() {
    let primary = delay(
        "p",
        Duration::from_millis(30),
        "p",
        false,
        true,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::ZERO,
        "unused",
        true,
        false,
        false,
        false,
        false,
    );
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("mid") || !err.to_string().is_empty(),
        "{err}"
    );
}

#[tokio::test]
async fn timeout_primary_dead_and_partner_stream_fail() {
    let primary = delay(
        "p",
        Duration::from_millis(30),
        "p",
        false,
        true,
        false,
        false,
        false,
    );
    let race = delay("r", Duration::ZERO, "r", false, true, false, false, false);
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("mid") || !err.to_string().is_empty(),
        "{err}"
    );
}

#[tokio::test]
async fn timeout_primary_dead_and_partner_empty() {
    let primary = delay(
        "p",
        Duration::from_millis(30),
        "p",
        false,
        true,
        false,
        false,
        false,
    );
    let race = delay("r", Duration::ZERO, "", false, false, false, false, false);
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("mid") || !err.to_string().is_empty(),
        "{err}"
    );
}

#[tokio::test]
async fn timeout_skips_primary_usage_then_takes_partner() {
    let primary = crate::scripted::ScriptedProvider::named(
        "p",
        [
            crate::scripted::ScriptedStep::Hang(Duration::from_millis(20)),
            crate::scripted::ScriptedStep::Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
        ],
    );
    let race = crate::scripted::ScriptedProvider::named(
        "r",
        [
            crate::scripted::ScriptedStep::Hang(Duration::from_millis(40)),
            crate::scripted::ScriptedStep::Text("backup".into()),
        ],
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    );
}

#[tokio::test]
async fn timeout_primary_usage_then_none_while_partner_opens() {
    let primary = crate::scripted::ScriptedProvider::named(
        "p",
        [
            crate::scripted::ScriptedStep::Hang(Duration::from_millis(20)),
            crate::scripted::ScriptedStep::Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
        ],
    );
    let race = crate::scripted::ScriptedProvider::named(
        "r",
        [
            crate::scripted::ScriptedStep::Hang(Duration::from_millis(5)),
            crate::scripted::ScriptedStep::Text("backup".into()),
        ],
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert!(matches!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    ));
}

#[tokio::test]
async fn hang_stream_never_emits_before_timeout() {
    use tokio::time::timeout;
    let primary = delay("p", Duration::ZERO, "x", false, false, true, false, false);
    let race = delay(
        "r",
        Duration::ZERO,
        "backup",
        false,
        false,
        false,
        false,
        false,
    );
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    );
    let hanging = delay("h", Duration::ZERO, "x", false, false, true, false, false);
    let opened = hanging.stream(&req(), "", "m").await.unwrap();
    let raced = timeout(Duration::from_millis(20), collect_text(opened)).await;
    assert!(raced.is_err(), "hanging stream should not complete");
}

#[tokio::test]
async fn timeout_primary_dead_then_slow_partner_open_fail() {
    let primary = delay(
        "p",
        Duration::from_millis(20),
        "p",
        false,
        true,
        false,
        false,
        false,
    );
    let race = delay(
        "r",
        Duration::from_millis(40),
        "unused",
        true,
        false,
        false,
        false,
        false,
    );
    let err = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(5),
    )
    .await
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.to_string().contains("mid") || !err.to_string().is_empty(),
        "{err}"
    );
}

#[tokio::test]
async fn partner_skips_non_first_token_then_wins() {
    let primary = delay(
        "p",
        Duration::from_millis(400),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = DelayProvider {
        name: "r".into(),
        delay: Duration::from_millis(5),
        text: "haiku".into(),
        opens: Arc::new(AtomicUsize::new(0)),
        fail_open: false,
        fail_mid: false,
        hang: false,
        thinking: false,
        tool_use: false,
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

struct SkipThenTextProvider {
    delay: Duration,
}

impl LlmProvider for SkipThenTextProvider {
    fn name(&self) -> &str {
        "skip"
    }
    fn default_base_url(&self) -> &str {
        "http://example.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            Ok(LlmResponse {
                content: vec![],
                stop_reason: None,
                usage: Usage::default(),
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
        let delay = self.delay;
        Box::pin(async move {
            Ok(Box::pin(async_stream::stream! {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                yield Ok(StreamEvent::Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                });
                yield Ok(StreamEvent::MessageStop);
                yield Ok(StreamEvent::TextDelta {
                    text: "backup".into(),
                });
            }) as ProviderEventStream)
        })
    }
}

#[tokio::test]
async fn partner_usage_then_text_is_first_token() {
    let primary = delay(
        "p",
        Duration::from_millis(400),
        "primary",
        false,
        false,
        false,
        false,
        false,
    );
    let race = SkipThenTextProvider {
        delay: Duration::from_millis(5),
    };
    let (s, outcome) = stream_raced(
        &transport(),
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
        &req(),
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    assert_eq!(collect_text(s).await, "backup");
    assert_eq!(
        outcome,
        RaceOutcome::Race {
            reason: "first_token"
        }
    );
}
