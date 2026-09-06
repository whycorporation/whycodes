use super::*;
use crate::provider::{LlmProvider, ProviderRegistry};
use crate::scripted::{ScriptedProvider, ScriptedStep};
use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};

fn init_tracing() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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

fn keys(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[tokio::test]
async fn empty_chain_exhausts() {
    let chain = FallbackChain::new(vec![], HashMap::new());
    let err = chain
        .complete(&req(), &ProviderRegistry::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("exhausted"), "{err}");
}

#[tokio::test]
async fn missing_provider_is_skipped() {
    init_tracing();
    let chain = FallbackChain::new(vec![("nope".into(), "m".into())], keys(&[("nope", "k")]));
    let err = chain
        .complete(&req(), &ProviderRegistry::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("exhausted"), "{err}");
}

#[tokio::test]
async fn first_success_short_circuits() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::named(
        "primary",
        [ScriptedStep::Text("ok".into())],
    )));
    registry.register(Box::new(ScriptedProvider::named(
        "backup",
        [ScriptedStep::Text("unused".into())],
    )));
    let chain = FallbackChain::new(
        vec![
            ("primary".into(), "m1".into()),
            ("backup".into(), "m2".into()),
        ],
        keys(&[("primary", "k"), ("backup", "k")]),
    );
    let resp = chain.complete(&req(), &registry).await.unwrap();
    assert_eq!(resp.model, "m1");
    assert!(matches!(
        &resp.content[0],
        whycodes_core::types::ContentBlock::Text { text } if text == "ok"
    ));
}

#[tokio::test]
async fn second_provider_used_after_first_fails() {
    init_tracing();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::named(
        "primary",
        [ScriptedStep::Error("down".into())],
    )));
    registry.register(Box::new(ScriptedProvider::named(
        "backup",
        [ScriptedStep::Text("recovered".into())],
    )));
    let chain = registry.get_with_fallback(
        ("primary".into(), "m1".into()),
        vec![("backup".into(), "m2".into())],
        keys(&[("primary", "k"), ("backup", "k")]),
    );
    let resp = chain.complete(&req(), &registry).await.unwrap();
    assert_eq!(resp.model, "m2");
    assert!(matches!(
        &resp.content[0],
        whycodes_core::types::ContentBlock::Text { text } if text == "recovered"
    ));
}

#[tokio::test]
async fn all_failing_providers_surface_last_error() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::named(
        "a",
        [ScriptedStep::Error("first".into())],
    )));
    registry.register(Box::new(ScriptedProvider::named(
        "b",
        [ScriptedStep::Error("second".into())],
    )));
    let chain = FallbackChain::new(
        vec![("a".into(), "m".into()), ("b".into(), "m".into())],
        keys(&[("a", "k"), ("b", "k")]),
    );
    let err = chain.complete(&req(), &registry).await.unwrap_err();
    assert!(err.to_string().contains("second"), "{err}");
}

#[test]
fn provider_name_and_default_url_on_scripted() {
    let p = ScriptedProvider::text("x");
    assert_eq!(p.name(), "script");
    assert_eq!(p.default_base_url(), "http://script.invalid");
}

#[tokio::test]
async fn missing_api_key_still_invokes_provider() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::named(
        "solo",
        [ScriptedStep::Text("ok".into())],
    )));
    let chain = FallbackChain::new(vec![("solo".into(), "m".into())], HashMap::new());
    let resp = chain.complete(&req(), &registry).await.unwrap();
    assert_eq!(resp.model, "m");
}
