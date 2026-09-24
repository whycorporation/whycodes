use super::*;
use serde_json::json;
use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};
use whycodes_llm::{ProviderRegistry, ScriptedProvider, ScriptedStep};

fn agent() -> Agent {
    Agent::new(AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
}

#[test]
fn truncate_tokens_keeps_short_text() {
    assert_eq!(truncate_tokens("hello", 2_000), "hello");
    let long = "word ".repeat(3_000);
    let out = truncate_tokens(&long, 10);
    assert!(out.contains("truncated"), "{out}");
}

#[tokio::test]
async fn empty_question_is_usage_error() {
    let a = agent();
    let s = Session::new(std::path::PathBuf::from("."), "sys".into());
    let err = run(&a, &s, "openai", "m", "k", "   ").await.unwrap_err();
    assert!(err.to_string().contains("/btw"), "{err}");
}

#[test]
fn read_only_set_excludes_bash_and_write() {
    assert!(READ_ONLY.contains(&"read"));
    assert!(READ_ONLY.contains(&"grep"));
    assert!(READ_ONLY.contains(&"glob"));
    assert!(!READ_ONLY.contains(&"bash"));
    assert!(!READ_ONLY.contains(&"write"));
}

#[tokio::test]
async fn side_turn_refuses_bash_and_does_not_write_parent_session() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                input: json!({"command": "echo pwned-btw"}),
            }],
            vec![ScriptedStep::Text("side-ok".into())],
        ],
    )));
    let a = Agent::new(AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    let mut parent = Session::new(std::path::PathBuf::from("."), "sys".into());
    parent.add_user_message("keep-me");
    let before = parent.messages.len();
    let tokens_before = parent.token_count();
    let out = run(&a, &parent, "script", "m", "k", "hello").await.unwrap();
    assert!(out.contains("side-ok"), "{out}");
    assert!(!out.contains("pwned-btw"), "{out}");
    assert_eq!(parent.messages.len(), before);
    assert_eq!(parent.token_count(), tokens_before);
    assert!(
        !parent
            .messages
            .iter()
            .any(|m| m.content.as_text().is_some_and(|t| t.contains("side-ok"))),
        "parent must not store the side answer"
    );
}

#[tokio::test]
async fn side_turn_refuses_write() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "write".into(),
                input: json!({"path": "x.txt", "content": "nope"}),
            }],
            vec![ScriptedStep::Text("write-blocked".into())],
        ],
    )));
    let a = Agent::new(AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    let parent = Session::new(std::path::PathBuf::from("."), "sys".into());
    let out = run(&a, &parent, "script", "m", "k", "hello").await.unwrap();
    assert!(out.contains("write-blocked"), "{out}");
}

#[tokio::test]
async fn unknown_provider_is_llm_error() {
    let a = agent();
    let s = Session::new(std::path::PathBuf::from("."), "sys".into());
    let err = run(&a, &s, "no-such-provider", "m", "k", "hello")
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("unknown"), "{err}");
}

#[tokio::test]
async fn thinking_blocks_are_ignored_and_text_is_kept() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::new([
        ScriptedStep::Thinking("scratch".into()),
        ScriptedStep::Text("visible-answer".into()),
    ])));
    let a = Agent::new(AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    let parent = Session::new(std::path::PathBuf::from("."), "sys".into());
    let out = run(&a, &parent, "script", "m", "k", "what").await.unwrap();
    assert!(out.contains("visible-answer"), "{out}");
    assert!(!out.contains("scratch"), "{out}");
}

#[tokio::test]
async fn readonly_read_tool_runs_and_returns_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "side-read-body").unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: json!({"path": "note.txt"}),
            }],
            vec![ScriptedStep::Text("after-read".into())],
        ],
    )));
    let a = Agent::new(AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    let parent = Session::new(dir.path().to_path_buf(), "sys".into());
    let out = run(&a, &parent, "script", "m", "k", "read it")
        .await
        .unwrap();
    assert!(out.contains("after-read"), "{out}");
}
