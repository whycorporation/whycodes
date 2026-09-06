use super::*;

#[test]
fn resolves_override_and_small_siblings() {
    let (p, m) = resolve_title_model("anthropic", "claude-sonnet-4-5", None);
    assert_eq!(p, "anthropic");
    assert!(m.contains("haiku"));

    let (p, m) = resolve_title_model("openai", "gpt-4o", Some("gpt-4.1-nano"));
    assert_eq!(p, "openai");
    assert_eq!(m, "gpt-4.1-nano");

    let (p, m) = resolve_title_model("xai", "grok-4", Some("openrouter/foo"));
    assert_eq!(p, "openrouter");
    assert_eq!(m, "foo");
}

#[test]
fn keeps_already_small_model() {
    let (p, m) = resolve_title_model("openai", "gpt-4o-mini", None);
    assert_eq!(p, "openai");
    assert_eq!(m, "gpt-4o-mini");
}

#[test]
fn small_siblings_for_every_provider() {
    let cases = [
        ("xai", "grok-4", "grok-3-mini"),
        ("groq", "llama-3.3-70b", "llama-3.1-8b-instant"),
        ("mistral", "mistral-large", "mistral-small-latest"),
        ("deepseek", "deepseek-reasoner", "deepseek-chat"),
        (
            "together",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo",
        ),
    ];
    for (provider, model, expect) in cases {
        let (p, m) = resolve_title_model(provider, model, None);
        assert_eq!(p, provider);
        assert_eq!(m, expect, "{provider} should map to a small sibling");
    }
}

#[test]
fn gemini_models_kept_as_is_because_mini_substring() {
    // "gemini" contains "mini", so the already-small marker fires and the
    // model is kept — Google models never get swapped (pre-existing quirk).
    let (p, m) = resolve_title_model("google", "gemini-2.5-pro", None);
    assert_eq!(p, "google");
    assert_eq!(m, "gemini-2.5-pro");
}

#[test]
fn openrouter_small_sibling_tracks_family() {
    let (_, m) = resolve_title_model("openrouter", "anthropic/claude-sonnet-4-5", None);
    assert_eq!(m, "anthropic/claude-haiku-4.5");
    let (_, m) = resolve_title_model("openrouter", "openai/gpt-5", None);
    assert_eq!(m, "openai/gpt-4o-mini");
    let (_, m) = resolve_title_model("openrouter", "mistralai/mistral-large", None);
    assert_eq!(m, "openai/gpt-4o-mini");
    // Gemini family is "already small" (mini substring) → kept.
    let (_, m) = resolve_title_model("openrouter", "google/gemini-2.5-pro", None);
    assert_eq!(m, "google/gemini-2.5-pro");
}

#[test]
fn ollama_keeps_local_model() {
    let (p, m) = resolve_title_model("ollama", "qwen2.5-coder:7b", None);
    assert_eq!(p, "ollama");
    assert_eq!(m, "qwen2.5-coder:7b");
}

#[test]
fn unknown_provider_keeps_model() {
    let (p, m) = resolve_title_model("myproxy", "custom-model", None);
    assert_eq!(p, "myproxy");
    assert_eq!(m, "custom-model");
}

#[test]
fn small_markers_detected() {
    assert!(is_already_small("claude-haiku-4-5-20251001"));
    assert!(is_already_small("gpt-4o-mini"));
    assert!(is_already_small("llama-3.1-8b-instant"));
    assert!(is_already_small("gemini-2.5-pro"), "gemini contains mini");
    assert!(!is_already_small("claude-sonnet-4-5"));
    // Marker match is case-sensitive on the raw string; callers lowercase.
    assert!(!is_already_small("GPT-4O-MINI"));
}

#[test]
fn truncate_adds_ellipsis_and_respects_chars() {
    assert_eq!(truncate("short", 100), "short");
    let out = truncate("abcdefghij", 4);
    assert_eq!(out, "abcd…");
    assert_eq!(out.chars().count(), 5);
}

#[test]
fn apply_refine_result_handles_empty_and_valid() {
    let mut session = Session::new(std::path::PathBuf::from("/tmp/proj"), String::new());
    session.add_user_message("fix auth");
    let before = session.title.clone();
    apply_refine_result(&mut session, "", "gpt-4o-mini");
    assert_eq!(session.title, before, "empty title must not clobber");
    apply_refine_result(&mut session, "Fix auth retries", "gpt-4o-mini");
    assert_eq!(session.title, "Fix auth retries");
}

#[test]
fn refine_gate_allows_legacy_default_multi_turn() {
    let mut session = Session::new(std::path::PathBuf::from("/tmp/proj"), String::new());
    // Placeholder still Default after two user turns (pre-auto-title rows).
    session.title = "New session - 2026-01-01".into();
    session.title_source = whycodes_session::TitleSource::Default;
    session.add_user_message("fix auth");
    session.add_user_message("also retries");
    assert!(should_refine_title(&session));

    session.title_source = whycodes_session::TitleSource::Heuristic;
    assert!(!should_refine_title(&session)); // multi-turn + already heuristicked
}

#[test]
fn skips_trivial_greetings() {
    assert!(is_trivial_title_seed("selam"));
    assert!(is_trivial_title_seed("Hi!"));
    assert!(is_trivial_title_seed("merhaba nasılsın"));
    assert!(is_trivial_title_seed("ping"));
    assert!(!is_trivial_title_seed(
        "fix the auth retry bug in session.rs"
    ));
    assert!(!is_trivial_title_seed("read crates/tui/src/run.rs"));

    let mut session = Session::new(std::path::PathBuf::from("/tmp/proj"), String::new());
    session.title_source = whycodes_session::TitleSource::Heuristic;
    session.add_user_message("selam");
    assert!(!should_refine_title(&session));
}

#[test]
fn google_antigravity_and_openrouter_o_family() {
    let (_, m) = resolve_title_model("google-antigravity", "gemini-3-pro", None);
    // "gemini" contains "mini", so already-small keeps the original.
    assert_eq!(m, "gemini-3-pro");
    let (_, m) = resolve_title_model("openrouter", "openai/o3-pro", None);
    assert_eq!(m, "openai/gpt-4o-mini");
    assert!(is_trivial_title_seed(""));
    assert!(is_trivial_title_seed("   "));
    assert!(!is_trivial_title_seed(&"x".repeat(49)));
    let mut session = Session::new(std::path::PathBuf::from("/tmp/proj"), String::new());
    session.title_source = whycodes_session::TitleSource::Manual;
    session.add_user_message("fix auth");
    assert!(!should_refine_title(&session));
}

#[tokio::test]
async fn generate_title_uses_scripted_text_and_strips_prefix() {
    let provider = whycodes_llm::ScriptedProvider::named(
        "title-script",
        [whycodes_llm::ScriptedStep::Text(
            "Title: Retry Loop\n".into(),
        )],
    );
    let title = generate_title(
        &provider,
        "k",
        "title-gen-unique-model",
        "please explain the retry loop",
        Some("I walked through crates/llm"),
    )
    .await
    .expect("title");
    assert!(!title.is_empty(), "{title}");
    assert!(!title.to_lowercase().starts_with("title:"), "{title}");
}

#[tokio::test]
async fn generate_title_empty_on_non_text() {
    let provider = whycodes_llm::ScriptedProvider::named(
        "title-empty",
        [whycodes_llm::ScriptedStep::Thinking("only think".into())],
    );
    let title = generate_title(&provider, "k", "title-empty-unique-model", "fix auth", None)
        .await
        .expect("ok empty");
    assert!(title.is_empty(), "{title}");
}

struct ImageOnlyTitleProvider;

impl whycodes_llm::LlmProvider for ImageOnlyTitleProvider {
    fn name(&self) -> &str {
        "title-image"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async move {
            Ok(whycodes_core::types::LlmResponse {
                content: vec![whycodes_core::types::ContentBlock::Image {
                    source: whycodes_core::types::ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "abc".into(),
                    },
                }],
                stop_reason: Some("end_turn".into()),
                usage: Default::default(),
                model: model.into(),
            })
        })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::llm("complete-only")) })
    }
}

#[tokio::test]
async fn generate_title_filters_non_text_image_block() {
    let title = generate_title(
        &ImageOnlyTitleProvider,
        "k",
        "title-image-unique-model",
        "fix auth",
        None,
    )
    .await
    .expect("ok empty");
    assert!(title.is_empty(), "{title}");
}

#[test]
fn apply_refine_result_skips_manual_title_source() {
    let mut session = Session::new(std::path::PathBuf::from("/tmp/proj"), String::new());
    session.title = "Keep me".into();
    session.title_source = whycodes_session::TitleSource::Manual;
    apply_refine_result(&mut session, "Retry Loop", "gpt-4o-mini");
    assert_eq!(session.title, "Keep me");
}

#[test]
fn openrouter_non_gemini_non_gpt_falls_to_mini() {
    let (_, m) = resolve_title_model("openrouter", "qwen/qwen-plus", None);
    assert_eq!(m, "openai/gpt-4o-mini");
}
