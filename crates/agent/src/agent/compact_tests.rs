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

/// Stands in for the Anthropic provider: answers `compact` with a fixed
/// result and records what it was asked.
struct NativeCompactProvider {
    reply: std::sync::Mutex<Option<whycodes_core::Result<whycodes_core::types::LlmResponse>>>,
    seen: Seen,
}

type Seen = Arc<std::sync::Mutex<Vec<(whycodes_core::types::LlmRequest, String)>>>;

impl whycodes_llm::LlmProvider for NativeCompactProvider {
    fn name(&self) -> &str {
        "anthropic"
    }
    fn default_base_url(&self) -> &str {
        "http://native.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::llm("no local summary here")) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::llm("compact-only")) })
    }
    fn compact<'a>(
        &'a self,
        request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
        instructions: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        self.seen
            .lock()
            .unwrap()
            .push((request.clone(), instructions.to_string()));
        let reply = self.reply.lock().unwrap().take().unwrap();
        Box::pin(async move { reply })
    }
}

fn compaction_raw() -> serde_json::Value {
    serde_json::json!({"type": "compaction", "content": "native summary", "signature": "sig"})
}

fn native_reply(
    stop_reason: &str,
    content: Vec<ContentBlock>,
) -> whycodes_core::Result<whycodes_core::types::LlmResponse> {
    Ok(whycodes_core::types::LlmResponse {
        content,
        stop_reason: Some(stop_reason.into()),
        usage: Default::default(),
        model: "claude-opus-5-5".into(),
    })
}

fn native_agent(reply: whycodes_core::Result<whycodes_core::types::LlmResponse>) -> (Agent, Seen) {
    let seen: Seen = Arc::default();
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(NativeCompactProvider {
        reply: std::sync::Mutex::new(Some(reply)),
        seen: Arc::clone(&seen),
    }));
    let mut agent = Agent::new(info()).with_provider_registry(registry);
    agent.compaction_llm = true;
    agent.compaction_native = true;
    (agent, seen)
}

fn block() -> ContentBlock {
    ContentBlock::compaction_from_raw(compaction_raw()).unwrap()
}

fn opens_with_block(session: &Session) -> bool {
    session.messages.iter().any(|m| {
        matches!(
            &m.content,
            whycodes_core::types::MessageContent::Blocks(b)
                if matches!(b.first(), Some(ContentBlock::Compaction { .. }))
        )
    })
}

#[tokio::test]
async fn native_compaction_replaces_the_prefix_with_the_provider_block() {
    let (agent, seen) = native_agent(native_reply("compaction", vec![block()]));
    let mut session = fat_session();
    let outcome = agent
        .compact_session(
            &mut session,
            "anthropic",
            "claude-opus-5-5",
            "k",
            Some("keep auth.rs"),
        )
        .await;
    assert!(outcome.dropped_messages());
    assert_eq!(session.messages.len(), 3);
    assert!(matches!(
        &session.messages[0].content,
        whycodes_core::types::MessageContent::Blocks(b)
            if matches!(&b[..], [ContentBlock::Compaction { raw, .. }] if *raw == compaction_raw())
    ));
    assert_eq!(session.messages[1].content.as_text(), Some("fix login"));

    let seen = seen.lock().unwrap();
    let (request, instructions) = &seen[0];
    assert_eq!(request.messages.len(), 2, "only the summarized prefix");
    assert_eq!(request.system, session.system_prompt);
    assert_eq!(request.max_tokens, Some(NATIVE_COMPACT_MAX_TOKENS));
    assert!(instructions.contains("keep auth.rs"));
    assert!(instructions.contains("Do NOT call or use any tools"));
    assert!(!instructions.contains("Conversation:"));
}

#[tokio::test]
async fn native_compaction_falls_back_to_full_replace() {
    // HTTP / transport failure.
    let (agent, _) = native_agent(Err(whycodes_core::Error::llm("400 compaction_misplaced")));
    let mut session = fat_session();
    agent
        .compact_session(&mut session, "anthropic", "claude-opus-5-5", "k", None)
        .await;
    let last = session.messages.last().unwrap().content.as_text().unwrap();
    assert!(last.contains("continued"), "{last}");
    assert!(!opens_with_block(&session));

    // A 200 without a summary (the call ran out of tokens).
    let (agent, seen) = native_agent(native_reply("max_tokens", Vec::new()));
    let mut session = fat_session();
    agent
        .compact_session(&mut session, "anthropic", "claude-opus-5-5", "k", None)
        .await;
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(!opens_with_block(&session));

    // Not asked: a model without the capability, or no clean cut (the turn
    // before the last ask is not an assistant reply).
    let (agent, seen) = native_agent(native_reply("compaction", vec![block()]));
    agent
        .compact_session(
            &mut fat_session(),
            "anthropic",
            "claude-3-5-sonnet",
            "k",
            None,
        )
        .await;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("first ask");
    session.add_user_message("second ask");
    agent
        .compact_session(&mut session, "anthropic", "claude-opus-5-5", "k", None)
        .await;
    assert!(seen.lock().unwrap().is_empty());

    // Not asked: `compaction_method = "local"`, or `compaction_llm = "off"`.
    let (mut local, local_seen) = native_agent(native_reply("compaction", vec![block()]));
    local.compaction_native = false;
    local
        .compact_session(
            &mut fat_session(),
            "anthropic",
            "claude-opus-5-5",
            "k",
            None,
        )
        .await;
    local.compaction_native = true;
    local.compaction_llm = false;
    local
        .compact_session(
            &mut fat_session(),
            "anthropic",
            "claude-opus-5-5",
            "k",
            None,
        )
        .await;
    assert!(local_seen.lock().unwrap().is_empty());

    // No provider registered under that name.
    let mut bare = Agent::new(info()).with_provider_registry(whycodes_llm::ProviderRegistry::new());
    bare.compaction_native = true;
    let mut session = fat_session();
    bare.compact_session(&mut session, "anthropic", "claude-opus-5-5", "k", None)
        .await;
    assert!(!opens_with_block(&session));
}

#[test]
fn native_compaction_block_needs_one_block_and_the_compaction_stop() {
    let one = native_reply("compaction", vec![block()]).unwrap();
    assert!(native_compaction_block(one).is_some());
    let two = native_reply("compaction", vec![block(), block()]).unwrap();
    assert!(native_compaction_block(two).is_none());
    let text = native_reply("compaction", vec![ContentBlock::Text { text: "t".into() }]).unwrap();
    assert!(native_compaction_block(text).is_none());
    let tool = native_reply("tool_use", vec![block()]).unwrap();
    assert!(native_compaction_block(tool).is_none());
}

#[test]
fn native_instructions_fit_the_provider_cap() {
    let huge = "n".repeat(50_000);
    let instructions = native_compact_instructions(Some(&huge));
    assert!(
        instructions.chars().count() <= 16_384,
        "{}",
        instructions.len()
    );
    assert!(instructions.ends_with("block as your text output."));
    assert_eq!(
        native_compact_instructions(None),
        compact_instructions(None)
    );
}

#[test]
fn speculative_summary_is_not_prepared_when_native_compaction_runs() {
    let (agent, _) = native_agent(native_reply("compaction", vec![block()]));
    let session = fat_session();
    assert!(!agent.maybe_spawn_speculative_summary(&session, "anthropic", "claude-opus-5-5", "k"));
}

#[test]
fn compaction_method_auto_turns_native_compaction_on() {
    let mut cfg = whycodes_config::Config::default();
    let mut agent = Agent::new(info());
    agent.apply_config(&cfg);
    assert!(!agent.compaction_native, "local is the default method");
    cfg.session.compaction_method = " Auto ".into();
    agent.apply_config(&cfg);
    assert!(agent.compaction_native);
}
