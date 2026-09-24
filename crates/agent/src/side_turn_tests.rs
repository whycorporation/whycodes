use super::*;
use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};

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
