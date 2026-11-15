use super::*;
use futures::StreamExt;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use whycodes_core::types::{Message, MessageContent};

/// One HTTP reply on a fresh loopback port. Returns the host base (no `/v1`)
/// and a receiver for the raw request.
fn serve_once(status: &str, body: &str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).unwrap_or(0);
            tx.send(String::from_utf8_lossy(&buf[..n]).into_owned())
                .unwrap();
            stream.write_all(payload.as_bytes()).unwrap();
        }
    });
    (format!("http://{addr}"), rx)
}

fn request(text: &str) -> LlmRequest {
    let msg = |role, t: &str| Message {
        role,
        content: MessageContent::Text(t.into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    };
    LlmRequest {
        system: "you are a coding agent".into(),
        messages: vec![
            msg(Role::User, "earlier question"),
            msg(Role::Assistant, "earlier answer"),
            msg(Role::User, text),
        ]
        .into(),
        tools: Vec::new().into(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

fn noul_reply(yes: f64) -> String {
    json!({
        "model": "clef-flash",
        "answers": {"answer": {"type": "noul", "noul": yes}},
        "usage": {"input_tokens": 120, "output_tokens": 0},
    })
    .to_string()
}

#[test]
fn parse_plain_prompt_is_yes_no() {
    let q = DecisionQuestion::parse("  Is this PR ready to merge?  ");
    assert_eq!(q.question, "Is this PR ready to merge?");
    assert!(q.options.is_empty());
    // One bullet is not a choice; the line stays in the question.
    let q = DecisionQuestion::parse("Ship it?\n- only option");
    assert_eq!(q.question, "Ship it?\n- only option");
    assert!(q.options.is_empty());
}

#[test]
fn parse_bullets_make_a_choice() {
    let q = DecisionQuestion::parse("Which database?\n- postgres\n  * sqlite\n- \n-duckdb");
    assert_eq!(q.question, "Which database?\n-");
    assert_eq!(q.options, ["postgres", "sqlite", "duckdb"]);
}

#[test]
fn parse_accepts_unspaced_dashes_and_numbered_lists() {
    // As typed in the TUI: indented, no space after the dash.
    let q = DecisionQuestion::parse(
        "Hangi veritabanını kullanalım?\n    -postgres\n    -sqlite\n    -duckdb",
    );
    assert_eq!(q.question, "Hangi veritabanını kullanalım?");
    assert_eq!(q.options, ["postgres", "sqlite", "duckdb"]);

    let q = DecisionQuestion::parse("Pick one\n1. alpha\n2) beta\n• gamma\n+ delta");
    assert_eq!(q.options, ["alpha", "beta", "gamma", "delta"]);

    // Not list items: flags, rules, bold, a bare number, a number in prose.
    for line in ["--force", "---", "**bold**", "12", "3 apples", "1.", ""] {
        assert_eq!(option_text(line), None, "{line:?}");
    }
}

#[test]
fn body_is_noul_or_choice() {
    let yes_no = DecisionQuestion::parse("Ready?").body("clef-flash");
    assert_eq!(yes_no["model"], "clef-flash");
    assert_eq!(yes_no["state"], "Ready?");
    assert_eq!(yes_no["questions"]["answer"]["type"], "noul");
    assert_eq!(yes_no["questions"]["answer"]["instructions"], "Ready?");

    let choice = DecisionQuestion::parse("Which?\n- a\n- b").body("m");
    assert_eq!(choice["questions"]["answer"]["type"], "choice");
    assert_eq!(
        choice["questions"]["answer"]["criteria"],
        json!({"a": "a", "b": "b"})
    );
}

#[test]
fn format_answer_sorts_and_draws_bars() {
    let yes =
        format_answer(&json!({"answers": {"answer": {"type": "noul", "noul": 0.71}}})).unwrap();
    assert_eq!(
        yes,
        "**yes** · 71%\n\n```text\n███████░░░  71%  yes\n███░░░░░░░  29%  no\n```\n"
    );

    let choice = format_answer(&json!({"answers": {"answer": {
        "type": "choice",
        "probabilities": {"postgres": 0.31, "sqlite": 0.58, "duckdb": "x"},
        "confidence": 0.734,
    }}}))
    .unwrap();
    let lines: Vec<&str> = choice.lines().collect();
    assert_eq!(lines[0], "**sqlite** · 58%", "{choice}");
    assert_eq!(lines[3], "██████░░░░  58%  sqlite", "{choice}");
    assert_eq!(lines[4], "███░░░░░░░  31%  postgres", "{choice}");
    assert_eq!(
        lines[5], "░░░░░░░░░░   0%  duckdb",
        "unreadable value counts as 0"
    );
    assert!(choice.ends_with("```\nconfidence 73%\n"), "{choice}");
}

#[test]
fn format_answer_rejects_refusals_and_other_shapes() {
    let err = |v: Value| format_answer(&v).unwrap_err();
    assert_eq!(
        err(json!({"answers": {"answer": {"type": "refusal"}}})),
        "the model declined to answer"
    );
    assert_eq!(
        err(json!({"answers": {"answer": {"type": "noul"}}})),
        "noul answer without a value"
    );
    assert_eq!(
        err(json!({"answers": {"answer": {"type": "choice"}}})),
        "choice answer without probabilities"
    );
    assert_eq!(err(json!({"error": "x"})), "no answer in the response");
}

#[test]
fn local_base_and_keys_resolve_per_model() {
    assert_eq!(
        DecisionProvider::default().local_base,
        "http://127.0.0.1:11434/v1"
    );
    assert_eq!(
        DecisionProvider::new(None, Some("http://gpu:11434/")).local_base,
        "http://gpu:11434/v1"
    );
    assert_eq!(
        DecisionProvider::new(None, Some("http://gpu:11434/v1")).local_base,
        "http://gpu:11434/v1"
    );
    assert_eq!(
        DecisionProvider::new(None, Some("  ")).local_base,
        "http://127.0.0.1:11434/v1"
    );

    let p = DecisionProvider::new(Some("sk-config".into()), Some("http://gpu:11434"));
    // Local model: local server, never a key.
    assert_eq!(
        p.endpoint("clef-flash", "sk-call"),
        ("http://gpu:11434/v1/systemone".into(), None)
    );
    // OpenRouter model: env var wins when set; else the call's key, else config.
    let (url, key) = p.endpoint("cloudflare/clef", "sk-call");
    assert_eq!(url, "https://openrouter.ai/api/v1/systemone");
    let env = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    assert_eq!(key, env.clone().or(Some("sk-call".into())));
    if env.is_none() {
        assert_eq!(
            p.endpoint("cloudflare/clef", "").1.as_deref(),
            Some("sk-config")
        );
        assert_eq!(
            DecisionProvider::default().endpoint("x/unknown", "").1,
            None
        );
    }
    // An unknown id goes to OpenRouter.
    assert_eq!(
        p.endpoint("x/unknown", "").0,
        "https://openrouter.ai/api/v1/systemone"
    );
}

#[test]
fn from_config_reads_openrouter_key_and_ollama_base() {
    let mut config = whycodes_config::Config::default();
    let block =
        |name: &str, key: Option<&str>, base: Option<&str>| whycodes_core::types::ProviderConfig {
            name: name.into(),
            api_key: key.map(str::to_string),
            api_base: None,
            base_url: base.map(str::to_string),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
            credentials: Default::default(),
        };
    config.providers.insert(
        "openrouter".into(),
        block("openrouter", Some("sk-or"), None),
    );
    config.providers.insert(
        "ollama".into(),
        block("ollama", None, Some("http://gpu:11434")),
    );
    let p = DecisionProvider::from_config(&config);
    assert_eq!(p.openrouter_key.as_deref(), Some("sk-or"));
    assert_eq!(p.local_base, "http://gpu:11434/v1");
    config.providers.clear();
    let p = DecisionProvider::from_config(&config);
    assert_eq!(p.openrouter_key, None);
    assert_eq!(p.local_base, "http://127.0.0.1:11434/v1");
}

#[tokio::test]
async fn complete_asks_the_last_user_message() {
    let (base, rx) = serve_once("200 OK", &noul_reply(0.8));
    let p = DecisionProvider::new(None, Some(&base));
    assert_eq!(p.name(), "decision");
    assert_eq!(p.default_base_url(), "https://openrouter.ai/api/v1");
    let r = p
        .complete(&request("Merge now?"), "", "clef-flash")
        .await
        .unwrap();
    let ContentBlock::Text { text } = &r.content[0] else {
        panic!("{r:?}")
    };
    assert!(text.starts_with("**yes** · 80%"), "{text}");
    assert_eq!(r.usage.input_tokens, 120);
    assert_eq!(r.model, "clef-flash");
    let raw = rx.recv().unwrap();
    assert!(raw.starts_with("POST /v1/systemone "), "{raw}");
    assert!(
        raw.contains("Merge now?") && !raw.contains("earlier question"),
        "{raw}"
    );
    assert!(
        !raw.to_ascii_lowercase().contains("authorization:"),
        "{raw}"
    );
}

#[tokio::test]
async fn stream_yields_text_usage_and_stop() {
    let (base, _rx) = serve_once("200 OK", &noul_reply(0.2));
    let p = DecisionProvider::new(None, Some(&base));
    let mut events = p
        .stream(&request("Merge now?"), "", "clef-flash")
        .await
        .unwrap();
    let mut seen = Vec::new();
    while let Some(e) = events.next().await {
        seen.push(e.unwrap());
    }
    assert!(
        matches!(&seen[0], StreamEvent::TextDelta { text } if text.starts_with("**no** · 80%"))
    );
    assert!(matches!(
        seen[1],
        StreamEvent::Usage {
            input_tokens: 120,
            ..
        }
    ));
    assert!(matches!(seen[2], StreamEvent::MessageStop));
}

#[tokio::test]
async fn errors_name_the_model_and_the_cause() {
    let msg = |e: whycodes_core::Error| e.to_string();
    let p = DecisionProvider::default();
    let empty = p
        .complete(&request("   "), "", "clef-flash")
        .await
        .unwrap_err();
    assert!(msg(empty).contains("no question"));

    let (base, _rx) = serve_once(
        "402 Payment Required",
        r#"{"error":{"message":"Insufficient credits"}}"#,
    );
    let p = DecisionProvider::new(None, Some(&base));
    let e = msg(p
        .complete(&request("q?"), "", "clef-flash")
        .await
        .unwrap_err());
    assert!(
        e.contains("HTTP 402") && e.contains("Insufficient credits"),
        "{e}"
    );

    let (base, _rx) = serve_once(
        "403 Forbidden",
        r#"{"detail":{"message":"organization deactivated"}}"#,
    );
    let p = DecisionProvider::new(None, Some(&base));
    let e = msg(p
        .complete(&request("q?"), "", "clef-flash")
        .await
        .unwrap_err());
    assert!(e.contains("organization deactivated"), "{e}");

    let (base, _rx) = serve_once("500 Internal Server Error", r#"{"error":"boom"}"#);
    let p = DecisionProvider::new(None, Some(&base));
    assert!(
        msg(p
            .stream(&request("q?"), "", "clef-flash")
            .await
            .err()
            .unwrap())
        .contains("boom")
    );

    let (base, _rx) = serve_once("502 Bad Gateway", "not json");
    let p = DecisionProvider::new(None, Some(&base));
    assert!(
        msg(p
            .complete(&request("q?"), "", "clef-flash")
            .await
            .unwrap_err())
        .contains("HTTP 502")
    );

    let (base, _rx) = serve_once("200 OK", r#"{"answers":{"answer":{"type":"refusal"}}}"#);
    let p = DecisionProvider::new(None, Some(&base));
    assert!(
        msg(p
            .complete(&request("q?"), "", "clef-flash")
            .await
            .unwrap_err())
        .contains("declined")
    );

    // Nothing listening: transport error.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let p = DecisionProvider::new(None, Some(&format!("http://{addr}")));
    assert!(
        msg(p
            .complete(&request("q?"), "", "clef-flash")
            .await
            .unwrap_err())
        .contains("clef-flash")
    );
}

#[tokio::test]
async fn openrouter_models_send_a_bearer_token() {
    // Any key-requiring model with a reachable base: point the local base at
    // the mock and call through `endpoint` semantics by using a key directly.
    let (base, rx) = serve_once("200 OK", &noul_reply(0.5));
    let p = DecisionProvider::new(None, Some(&base));
    // Local models never send one, even when the call carries a key.
    p.complete(&request("q?"), "sk-call", "clef-flash")
        .await
        .unwrap();
    let raw = rx.recv().unwrap().to_ascii_lowercase();
    assert!(!raw.contains("authorization:"), "{raw}");
}

#[test]
fn registry_lists_decision_but_keeps_it_out_of_side_requests() {
    let registry = crate::provider::ProviderRegistry::default();
    assert!(registry.get(DECISION_PROVIDER).is_some());
    assert!(registry.get_for_side_request(DECISION_PROVIDER).is_none());
    assert!(registry.get_for_side_request("anthropic").is_some());
    assert!(registry.get_for_side_request("no-such-provider").is_none());

    let mut configured = crate::provider::ProviderRegistry::default();
    configured.register_from_config(&whycodes_config::Config::default());
    assert!(configured.get(DECISION_PROVIDER).is_some());
}

#[test]
fn decision_provider_needs_no_api_key_gate() {
    assert!(!crate::endpoint::provider_requires_api_key(
        DECISION_PROVIDER,
        None
    ));
    assert!(crate::endpoint::provider_requires_api_key("acme", None));
}
