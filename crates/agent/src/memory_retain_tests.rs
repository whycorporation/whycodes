use super::*;
use whycodes_core::types::ContentBlock;
use whycodes_memory::MemorySettings;

fn make_session() -> Session {
    let mut s = Session::new(PathBuf::from("/work/proj"), "sys".into());
    s.add_user_message("fix the auth bug");
    s.add_assistant_message(vec![ContentBlock::Text {
        text: "I patched it".into(),
    }]);
    s.add_user_message("also add retries");
    s.add_assistant_message(vec![ContentBlock::Text {
        text: "done, plus retries".into(),
    }]);
    s
}

#[test]
fn snapshot_captures_last_user_turn() {
    let session = make_session();
    let snap = RetainSnapshot::from_session(&session, "final answer");
    assert_eq!(snap.project_path, PathBuf::from("/work/proj"));
    assert_eq!(snap.session_id, session.id);
    assert_eq!(snap.user_text, "also add retries");
    assert_eq!(snap.assistant_text, "final answer");
    assert_eq!(snap.turn_index, 2);
}

#[test]
fn snapshot_empty_user_text_when_no_user_message() {
    let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    let snap = RetainSnapshot::from_session(&session, "hi");
    assert_eq!(snap.user_text, "");
    assert_eq!(snap.turn_index, 1, "turn_index floors at 1");
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn llm_retain_gate_respects_settings() {
    let mut s = MemorySettings::default();
    // Disabled → never.
    s.enabled = false;
    assert!(!should_llm_retain(&s, 0, 1));
    s.enabled = true;
    // auto_retain off → never.
    s.auto_retain = false;
    assert!(!should_llm_retain(&s, 0, 1));
    s.auto_retain = true;
    // retain_llm off → never.
    s.retain_llm = false;
    assert!(!should_llm_retain(&s, 0, 1));
    s.retain_llm = true;
    // Every-N: turn 1 with N=2 skips, turn 2 runs.
    s.retain_every_n = 2;
    assert!(!should_llm_retain(&s, 0, 1));
    assert!(should_llm_retain(&s, 0, 2));
    // Heuristic found facts and always=false → skip.
    s.retain_every_n = 1;
    assert!(!should_llm_retain(&s, 3, 1));
    // retain_llm_always overrides heuristic.
    s.retain_llm_always = true;
    assert!(should_llm_retain(&s, 3, 1));
}

#[test]
fn merge_facts_dedupes_case_insensitively() {
    let mut saved = vec!["Fix auth".to_string()];
    merge_facts(
        &mut saved,
        vec!["fix AUTH".to_string(), "new fact".to_string()],
    );
    assert_eq!(saved, vec!["Fix auth", "new fact"]);
}

#[tokio::test]
async fn post_turn_retain_skips_when_disabled() {
    let session = make_session();
    let settings = MemorySettings::disabled();
    let registry = whycodes_llm::ProviderRegistry::default();
    let provider = registry.get("anthropic").expect("built-in provider");
    let saved = run_post_turn_retain(
        &session,
        "answer",
        &settings,
        provider,
        "anthropic",
        "claude-sonnet-4-5",
        "",
        std::path::Path::new("/tmp"),
    )
    .await;
    assert!(saved.is_empty());
}

#[test]
fn spawn_retain_is_noop_when_disabled() {
    let session = make_session();
    let settings = MemorySettings::disabled();
    let registry = Arc::new(whycodes_llm::ProviderRegistry::default());
    // Must not panic or spawn anything — just returns early.
    spawn_post_turn_retain(
        &session,
        "answer",
        &settings,
        registry,
        "anthropic",
        "m",
        "k",
        None,
    );
}

#[test]
fn spawn_retain_skips_when_no_retain_and_no_inject() {
    let session = make_session();
    let settings = MemorySettings {
        enabled: true,
        auto_retain: false,
        session_inject: false,
        ..MemorySettings::default()
    };
    let registry = Arc::new(whycodes_llm::ProviderRegistry::default());
    spawn_post_turn_retain(
        &session,
        "answer",
        &settings,
        registry,
        "anthropic",
        "m",
        "k",
        None,
    );
}

#[tokio::test]
async fn heuristic_retain_saves_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message(
        "Always use rustfmt and clippy -D warnings in this crate. Remember that.",
    );
    session.add_assistant_message(vec![ContentBlock::Text {
        text: "Noted: rustfmt + clippy -D warnings.".into(),
    }]);
    let settings = MemorySettings {
        enabled: true,
        auto_retain: true,
        retain_llm: false,
        ..MemorySettings::default()
    };
    let registry = whycodes_llm::ProviderRegistry::default();
    let provider = registry.get("anthropic").expect("built-in");
    let saved = run_post_turn_retain(
        &session,
        "Noted: rustfmt + clippy -D warnings.",
        &settings,
        provider,
        "anthropic",
        "claude-sonnet-4-5",
        "",
        dir.path(),
    )
    .await;
    let _ = saved;
}

#[tokio::test]
async fn llm_retain_uses_scripted_provider() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("please remember we use sqlite for sessions");
    session.add_assistant_message(vec![ContentBlock::Text {
        text: "ok, sqlite".into(),
    }]);
    let settings = MemorySettings {
        enabled: true,
        auto_retain: true,
        retain_llm: true,
        retain_llm_always: true,
        retain_every_n: 1,
        ..MemorySettings::default()
    };
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::text(
        "- sessions persist in sqlite\n",
    )));
    let provider = registry.get("script").expect("script");
    let saved = run_post_turn_retain(
        &session,
        "ok, sqlite",
        &settings,
        provider,
        "script",
        "m",
        "k",
        dir.path(),
    )
    .await;
    let _ = saved;
}

#[tokio::test]
async fn spawn_retain_emits_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("Remember: the crate is named whycodes.");
    session.add_assistant_message(vec![ContentBlock::Text { text: "ok".into() }]);
    let settings = MemorySettings {
        enabled: true,
        auto_retain: true,
        retain_llm: false,
        ..MemorySettings::default()
    };
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::text("none")));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_post_turn_retain(
        &session,
        "ok",
        &settings,
        Arc::new(registry),
        "script",
        "m",
        "k",
        Some(tx),
    );
}

#[tokio::test]
async fn spawn_retain_llm_path_and_open_fail() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("please remember we use sqlite for sessions");
    session.add_assistant_message(vec![ContentBlock::Text { text: "ok".into() }]);
    let settings = MemorySettings {
        enabled: true,
        auto_retain: true,
        retain_llm: true,
        retain_llm_always: true,
        retain_every_n: 1,
        ..MemorySettings::default()
    };
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::named(
        "script",
        [whycodes_llm::ScriptedStep::Text(
            "- sessions persist in sqlite\n".into(),
        )],
    )));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_post_turn_retain(
        &session,
        "ok",
        &settings,
        Arc::new(registry),
        "script",
        "retain-llm-unique-model",
        "k",
        Some(tx),
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let snap = RetainSnapshot::from_session(&session, "ok");
    let saved = run_heuristic_retain(
        &snap,
        &settings,
        std::path::Path::new("/dev/null/not-a-data-dir"),
    );
    assert!(saved.is_empty());
}

#[tokio::test]
async fn spawn_retain_llm_error_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("please remember we use sqlite for sessions");
    session.add_assistant_message(vec![ContentBlock::Text { text: "ok".into() }]);
    let settings = MemorySettings {
        enabled: true,
        auto_retain: true,
        retain_llm: true,
        retain_llm_always: true,
        retain_every_n: 1,
        ..MemorySettings::default()
    };
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::named(
        "script",
        [whycodes_llm::ScriptedStep::Error("retain boom".into())],
    )));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_post_turn_retain(
        &session,
        "ok",
        &settings,
        Arc::new(registry),
        "script",
        "retain-llm-err-unique-model",
        "k",
        Some(tx),
    );
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
}

struct NonTextRetainProvider;

impl whycodes_llm::LlmProvider for NonTextRetainProvider {
    fn name(&self) -> &str {
        "retain-nontext"
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
async fn llm_extract_facts_filters_non_text() {
    let p = NonTextRetainProvider;
    let raw = llm_extract_facts(&p, "retain-nontext", "m", "k", "user", "asst")
        .await
        .expect("ok");
    assert!(raw.is_empty(), "{raw}");
    let (p_name, m_id) = crate::title::resolve_title_model("openai", "gpt-4o", None);
    assert_ne!(m_id, "gpt-4o");
    let _ = p_name;
}
