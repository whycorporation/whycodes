use super::*;

#[test]
fn compact_prompt_includes_transcript_and_optional_context() {
    let bare = build_compact_summary_prompt("hello transcript", None);
    assert!(bare.contains("hello transcript"));
    assert!(!bare.contains("User-provided context"));
    let with = build_compact_summary_prompt("hello transcript", Some(" keep auth.rs "));
    assert!(with.contains("keep auth.rs"));
    assert!(with.contains("<summary>"));
    let empty_ctx = build_compact_summary_prompt("t", Some("   "));
    assert!(!empty_ctx.contains("User-provided context"));
}

fn info() -> whycodes_core::types::AgentInfo {
    whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    }
}

fn fat_session() -> Session {
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("old");
    session.add_assistant_message(vec![ContentBlock::Text { text: "ack".into() }]);
    session.add_user_message("fix login");
    session.add_assistant_message(vec![ContentBlock::Text {
        text: "working".into(),
    }]);
    session
}

#[tokio::test]
async fn compact_empty_session_is_default() {
    let agent = Agent::new(info());
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    let outcome = agent
        .compact_session(&mut session, "script", "m", "k", None)
        .await;
    assert_eq!(outcome.messages_before, 0);
    assert!(session.messages.is_empty());
}

#[tokio::test]
async fn llm_compact_missing_provider_falls_back_local() {
    let mut agent = Agent::new(info());
    agent.compaction_llm = true;
    let mut session = fat_session();
    let outcome = agent
        .compact_session(
            &mut session,
            "no-such-provider",
            "compact-missing-provider-test",
            "k",
            Some("keep auth.rs"),
        )
        .await;
    assert!(outcome.dropped_messages() || outcome.reduced() || session.messages.len() <= 4);
    let last = session.messages.last().unwrap().content.as_text().unwrap();
    assert!(
        last.contains("fix login") || last.contains("auth.rs") || last.contains("continued"),
        "{last}"
    );
}

#[tokio::test]
async fn llm_compact_error_falls_back_local() {
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::new([
        whycodes_llm::ScriptedStep::Error("compact failed".into()),
    ])));
    let mut agent = Agent::new(info()).with_provider_registry(registry);
    agent.compaction_llm = true;
    let mut session = fat_session();
    let _ = agent
        .compact_session(
            &mut session,
            "script",
            "compact-error-fallback-test",
            "k",
            None,
        )
        .await;
    assert!(!session.messages.is_empty());
}

#[tokio::test]
async fn llm_compact_empty_text_falls_back_local() {
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::new([
        whycodes_llm::ScriptedStep::Thinking("only thinking".into()),
    ])));
    let mut agent = Agent::new(info()).with_provider_registry(registry);
    agent.compaction_llm = true;
    let mut session = fat_session();
    let _ = agent
        .compact_session(
            &mut session,
            "script",
            "compact-empty-text-fallback-test",
            "k",
            None,
        )
        .await;
    assert!(!session.messages.is_empty());
}

#[tokio::test]
async fn llm_compact_text_is_used() {
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::text(
        "<summary>LLM summary of login work</summary>",
    )));
    let mut agent = Agent::new(info()).with_provider_registry(registry);
    agent.compaction_llm = true;
    let mut session = fat_session();
    let _ = agent
        .compact_session(
            &mut session,
            "script",
            "compact-text-used-test",
            "k",
            Some("keep auth.rs"),
        )
        .await;
    let joined: String = session
        .messages
        .iter()
        .filter_map(|m| m.content.as_text().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("LLM summary") || joined.contains("login") || joined.contains("continued"),
        "{joined}"
    );
}

#[tokio::test]
async fn compact_skips_llm_when_key_empty() {
    let mut agent = Agent::new(info());
    agent.compaction_llm = true;
    let mut session = fat_session();
    let _ = agent
        .compact_session(&mut session, "script", "m", "", None)
        .await;
    assert!(!session.messages.is_empty());
}

#[tokio::test]
async fn llm_compact_summary_empty_transcript_is_none() {
    let agent = Agent::new(info());
    for transcript in ["   ", ""] {
        assert!(
            summarize_with_model(
                &agent.provider_registry,
                None,
                transcript,
                "script",
                "m",
                "k",
                None
            )
            .await
            .is_none()
        );
    }
}

struct NonTextCompleteProvider;

impl whycodes_llm::LlmProvider for NonTextCompleteProvider {
    fn name(&self) -> &str {
        "script"
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
                content: vec![ContentBlock::RedactedThinking {
                    data: "opaque".into(),
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
async fn llm_compact_non_text_complete_falls_back_local() {
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(NonTextCompleteProvider));
    let mut agent = Agent::new(info()).with_provider_registry(registry);
    agent.compaction_llm = true;
    let mut session = fat_session();
    let _ = agent
        .compact_session(
            &mut session,
            "script",
            "compact-non-text-complete-test",
            "k",
            None,
        )
        .await;
    assert!(!session.messages.is_empty());
}

fn prepared_registry() -> whycodes_llm::ProviderRegistry {
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "script",
        vec![whycodes_llm::ScriptedStep::Text(
            "<summary>prepared earlier</summary>".into(),
        )],
    )));
    registry
}

#[tokio::test]
async fn speculative_summary_is_prepared_once_and_used_while_the_prefix_holds() {
    let mut agent = Agent::new(info()).with_provider_registry(prepared_registry());
    agent.compaction_llm = true;
    let mut session = fat_session();

    assert!(agent.maybe_spawn_speculative_summary(&session, "script", "spec-happy", "k"));
    assert!(
        !agent.maybe_spawn_speculative_summary(&session, "script", "spec-happy", "k"),
        "the same prefix is not summarized twice"
    );
    let summary = agent.take_speculative_summary(&session).await.unwrap();
    assert!(summary.contains("prepared earlier"), "{summary}");
    assert!(
        agent.take_speculative_summary(&session).await.is_none(),
        "consumed"
    );

    let outcome = agent.compact_with_summary(&mut session, &summary).await;
    assert!(outcome.dropped_messages() || outcome.reduced());
    let last = session.messages.last().unwrap().content.as_text().unwrap();
    assert!(last.contains("prepared earlier"), "{last}");

    // An empty session compacts to nothing without a model call.
    let mut empty = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    let outcome = agent.compact_with_summary(&mut empty, "unused").await;
    assert_eq!(outcome.messages_after, 0);
}

#[tokio::test]
async fn speculative_summary_is_dropped_when_stale_or_unavailable() {
    let mut agent = Agent::new(info()).with_provider_registry(prepared_registry());
    agent.compaction_llm = true;

    // Local-only compaction, no key, or no prefix: nothing is started.
    let session = fat_session();
    assert!(!agent.maybe_spawn_speculative_summary(&session, "script", "spec-nokey", ""));
    let mut local = Agent::new(info()).with_provider_registry(prepared_registry());
    local.compaction_llm = false;
    assert!(!local.maybe_spawn_speculative_summary(&session, "script", "spec-local", "k"));
    let mut bare = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    bare.add_user_message("only one ask");
    assert!(!agent.maybe_spawn_speculative_summary(&bare, "script", "spec-bare", "k"));
    assert!(
        agent.take_speculative_summary(&bare).await.is_none(),
        "nothing prepared"
    );

    // The prefix moved (a new user turn): the job is stale and aborted.
    let mut session = fat_session();
    assert!(agent.maybe_spawn_speculative_summary(&session, "script", "spec-stale", "k"));
    session.add_user_message("another ask");
    assert!(agent.take_speculative_summary(&session).await.is_none());

    // A new prefix replaces an older job instead of keeping both.
    assert!(agent.maybe_spawn_speculative_summary(&session, "script", "spec-replace-1", "k"));
    session.add_user_message("yet another ask");
    assert!(agent.maybe_spawn_speculative_summary(&session, "script", "spec-replace-2", "k"));

    // Another session's job is never used.
    let other = fat_session();
    assert!(agent.take_speculative_summary(&other).await.is_none());

    // A job that produced no text (provider error) yields nothing.
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "script",
        vec![whycodes_llm::ScriptedStep::Error("summary failed".into())],
    )));
    let mut failing = Agent::new(info()).with_provider_registry(registry);
    failing.compaction_llm = true;
    let session = fat_session();
    assert!(failing.maybe_spawn_speculative_summary(&session, "script", "spec-fails-unique", "k"));
    assert!(failing.take_speculative_summary(&session).await.is_none());

    // An unknown provider: the job runs and finds nothing to call.
    assert!(failing.maybe_spawn_speculative_summary(&session, "nope", "spec-no-provider", "k"));
    assert!(failing.take_speculative_summary(&session).await.is_none());

    // A job cancelled from outside (shutdown) is reported as nothing prepared.
    assert!(failing.maybe_spawn_speculative_summary(&session, "nope", "spec-cancelled", "k"));
    if let Some(job) = super::super::recover_lock(&failing.speculative_summary).as_ref() {
        job.handle.abort();
    }
    assert!(failing.take_speculative_summary(&session).await.is_none());
}
