use crate::agent::Agent;
use crate::permission::PermissionPrompter;
use crate::question::{QuestionError, QuestionPrompter};
use crate::subagent::SubagentTask;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use whycodes_core::types::{
    AgentInfo, AgentMode, ApprovalMode, PermissionAction, PermissionSet, ToolCall,
};
use whycodes_tools::question::QuestionSpec;

fn make_test_agent_info(name: &str) -> AgentInfo {
    AgentInfo {
        name: name.to_string(),
        description: format!("Test agent: {name}"),
        mode: AgentMode::Primary,
        permission: PermissionSet {
            allowed_tools: None,
            denied_tools: None,
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            allowed_paths: None,
            rules: Default::default(),
        },
        model: None,
        system_prompt: Some("You are a test agent.".to_string()),
        temperature: Some(0.5),
        top_p: None,
    }
}

// ─── Shell risk gate ───────────────────────────────────────────────────
//
// Classification itself is covered by `whycodes-command-risk`. These tests
// cover the part only this layer can prove: that the gate sits in front of the
// permission map rather than behind it.

/// An agent whose permission map explicitly allows `bash` — the setting the
/// gate has to survive.
fn agent_with_bash_allowed() -> Agent {
    let mut info = make_test_agent_info("build");
    info.permission
        .rules
        .insert("bash".to_string(), PermissionAction::Allow);
    Agent::new(info)
}

fn bash_call(command: &str) -> whycodes_core::types::ToolCall {
    whycodes_core::types::ToolCall {
        id: "tc-1".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({ "command": command }),
    }
}

async fn run_bash(agent: &Agent, command: &str) -> whycodes_core::types::ToolResult {
    let session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".to_string(),
    );
    let ctx = whycodes_core::ToolContext {
        working_dir: "/work/proj".to_string(),
        session_id: None,
        sandbox: whycodes_core::SandboxSettings::off(),
        network: whycodes_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        todo_sink: None,
        swarm_hub: None,
    };
    agent
        .execute_with_permission(
            &bash_call(command),
            &session,
            &ctx,
            "anthropic",
            "m",
            "k",
            None,
            None,
        )
        .await
}

/// A command classified `Catastrophic` that would do nothing if it ever ran.
///
/// These tests execute against a live `ShellTool`, so a regression that lets a
/// command through must not be able to destroy the machine running the suite.
/// `mkfs.*` is classified by family, and this member of the family does not
/// exist as a binary, so a failure here is a failed assertion rather than a
/// wiped disk. The dangerous strings — `rm -rf /`, `rm -rf ~` — are covered in
/// `whycodes-command-risk`, where classification is a pure function and nothing
/// is executed.
const HARMLESS_CATASTROPHIC: &str = "mkfs.whycodes-test-not-a-real-binary /dev/null";

#[tokio::test]
async fn catastrophic_command_is_refused_despite_bash_being_allowed() {
    let agent = agent_with_bash_allowed();
    let result = run_bash(&agent, HARMLESS_CATASTROPHIC).await;
    assert!(result.is_error);
    assert!(
        result.content.starts_with("Refused:"),
        "the gate must override an explicit `allow`: {}",
        result.content
    );
}

#[tokio::test]
async fn refusal_says_it_cannot_be_approved() {
    let agent = agent_with_bash_allowed();
    let result = run_bash(&agent, HARMLESS_CATASTROPHIC).await;
    assert!(
        result.content.contains("cannot be approved"),
        "a refusal is not a prompt, and should say so: {}",
        result.content
    );
    assert!(
        result.content.contains("mkfs"),
        "the reason should name what triggered it: {}",
        result.content
    );
}

#[tokio::test]
async fn a_catastrophic_command_hidden_in_a_chain_is_still_refused() {
    let agent = agent_with_bash_allowed();
    let result = run_bash(&agent, &format!("echo hi && {HARMLESS_CATASTROPHIC}")).await;
    assert!(result.is_error);
    assert!(result.content.starts_with("Refused:"), "{}", result.content);
}

/// The gate has to run *before* the permission map, not after it. With `bash`
/// denied, a working gate answers "Refused:" while a gate that runs too late
/// answers "Permission denied" — so the message distinguishes them, and the
/// deny is a second backstop against execution.
#[tokio::test]
async fn the_gate_runs_before_the_permission_map() {
    let mut info = make_test_agent_info("build");
    info.permission
        .rules
        .insert("bash".to_string(), PermissionAction::Deny);
    let agent = Agent::new(info);
    let result = run_bash(&agent, HARMLESS_CATASTROPHIC).await;
    assert!(
        result.content.starts_with("Refused:"),
        "expected the risk gate to answer first, got: {}",
        result.content
    );
}

#[tokio::test]
async fn deny_still_wins_for_non_shell_tools() {
    let mut info = make_test_agent_info("build");
    info.permission
        .rules
        .insert("read".to_string(), PermissionAction::Deny);
    let agent = Agent::new(info);

    let session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".to_string(),
    );
    let ctx = whycodes_core::ToolContext {
        working_dir: "/work/proj".to_string(),
        session_id: None,
        sandbox: whycodes_core::SandboxSettings::off(),
        network: whycodes_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        todo_sink: None,
        swarm_hub: None,
    };
    let call = whycodes_core::types::ToolCall {
        id: "tc-2".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({ "path": "x" }),
    };
    let result = agent
        .execute_with_permission(&call, &session, &ctx, "anthropic", "m", "k", None, None)
        .await;
    assert!(result.is_error);
    assert!(
        result.content.contains("Permission denied"),
        "{}",
        result.content
    );
}

struct CountingDenyPrompter {
    asks: AtomicUsize,
}

impl PermissionPrompter for CountingDenyPrompter {
    fn ask<'a>(
        &'a self,
        _tool_name: &'a str,
        _detail: &'a str,
    ) -> crate::permission::PermissionAskFuture<'a> {
        Box::pin(async move {
            self.asks.fetch_add(1, Ordering::SeqCst);
            false
        })
    }
}

struct CountingCancelQuestionPrompter {
    asks: AtomicUsize,
}

impl QuestionPrompter for CountingCancelQuestionPrompter {
    fn ask(&self, _questions: Vec<QuestionSpec>) -> crate::question::QuestionAskFuture<'_> {
        Box::pin(async move {
            self.asks.fetch_add(1, Ordering::SeqCst);
            Err(QuestionError::Cancelled)
        })
    }
}

fn ask_rule_agent(tool: &str) -> Agent {
    let mut info = make_test_agent_info("build");
    info.permission
        .rules
        .insert(tool.to_string(), PermissionAction::Ask);
    Agent::new(info)
}

fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "tc-ask".into(),
        name: name.into(),
        arguments,
    }
}

async fn run_named(agent: &Agent, call: ToolCall) -> whycodes_core::types::ToolResult {
    let session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".to_string(),
    );
    let ctx = whycodes_core::ToolContext {
        working_dir: "/work/proj".to_string(),
        session_id: None,
        sandbox: whycodes_core::SandboxSettings::off(),
        network: whycodes_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        todo_sink: None,
        swarm_hub: None,
    };
    agent
        .execute_with_permission(&call, &session, &ctx, "anthropic", "m", "k", None, None)
        .await
}

#[tokio::test]
async fn auto_skips_permission_ask_without_calling_prompter() {
    let prompter = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut agent = ask_rule_agent("webfetch").with_permission_prompter(prompter.clone());
    agent.set_approval_mode(ApprovalMode::Auto);
    let result = run_named(
        &agent,
        tool_call(
            "webfetch",
            serde_json::json!({"url": "https://example.com"}),
        ),
    )
    .await;
    assert_eq!(prompter.asks.load(Ordering::SeqCst), 0);
    assert!(
        !result.content.contains("User denied permission"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn important_skips_low_risk_ask_but_prompts_high_risk() {
    let low = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut agent = ask_rule_agent("webfetch").with_permission_prompter(low.clone());
    agent.set_approval_mode(ApprovalMode::Important);
    let _ = run_named(
        &agent,
        tool_call(
            "webfetch",
            serde_json::json!({"url": "https://example.com"}),
        ),
    )
    .await;
    assert_eq!(low.asks.load(Ordering::SeqCst), 0);

    let high = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut agent = ask_rule_agent("browser").with_permission_prompter(high.clone());
    agent.set_approval_mode(ApprovalMode::Important);
    let result = run_named(&agent, tool_call("browser", serde_json::json!({}))).await;
    assert_eq!(high.asks.load(Ordering::SeqCst), 1);
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );

    let outside = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = make_test_agent_info("build");
    info.permission
        .rules
        .insert("write".into(), PermissionAction::Ask);
    let mut agent = Agent::new(info).with_permission_prompter(outside.clone());
    agent.set_approval_mode(ApprovalMode::Important);
    let result = run_named(
        &agent,
        tool_call(
            "write",
            serde_json::json!({"path": "../secret.txt", "content": "x"}),
        ),
    )
    .await;
    assert_eq!(outside.asks.load(Ordering::SeqCst), 1);
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );

    let bash = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = make_test_agent_info("build");
    info.permission
        .rules
        .insert("bash".into(), PermissionAction::Ask);
    let mut agent = Agent::new(info).with_permission_prompter(bash.clone());
    agent.set_approval_mode(ApprovalMode::Important);
    let result = run_named(&agent, bash_call("rm -rf /tmp/scratch")).await;
    assert_eq!(bash.asks.load(Ordering::SeqCst), 1);
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn manual_prompts_every_permission_ask() {
    let prompter = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut agent = ask_rule_agent("webfetch").with_permission_prompter(prompter.clone());
    agent.set_approval_mode(ApprovalMode::Manual);
    let result = run_named(
        &agent,
        tool_call(
            "webfetch",
            serde_json::json!({"url": "https://example.com"}),
        ),
    )
    .await;
    assert_eq!(prompter.asks.load(Ordering::SeqCst), 1);
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn auto_answers_question_without_ui_prompter() {
    let prompter = Arc::new(CountingCancelQuestionPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut agent =
        Agent::new(make_test_agent_info("build")).with_question_prompter(prompter.clone());
    agent.set_approval_mode(ApprovalMode::Auto);
    let result = run_named(
        &agent,
        tool_call(
            "question",
            serde_json::json!({"question": "Pick", "choices": ["A", "B"]}),
        ),
    )
    .await;
    assert_eq!(prompter.asks.load(Ordering::SeqCst), 0);
    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains('A'), "{}", result.content);
}

#[tokio::test]
async fn important_and_manual_prompt_on_question() {
    for mode in [ApprovalMode::Important, ApprovalMode::Manual] {
        let prompter = Arc::new(CountingCancelQuestionPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut agent =
            Agent::new(make_test_agent_info("build")).with_question_prompter(prompter.clone());
        agent.set_approval_mode(mode);
        let result = run_named(
            &agent,
            tool_call(
                "question",
                serde_json::json!({"question": "Pick", "choices": ["A", "B"]}),
            ),
        )
        .await;
        assert_eq!(prompter.asks.load(Ordering::SeqCst), 1, "{mode}");
        assert!(result.is_error, "{}", result.content);
        assert!(result.content.contains("cancelled"), "{}", result.content);
    }
}

#[test]
fn test_agent_new() {
    let info = make_test_agent_info("test");
    let agent = Agent::new(info);

    // Verify agent info fields
    assert_eq!(agent.info.name, "test");
    assert_eq!(agent.info.description, "Test agent: test");
    assert_eq!(agent.info.mode, AgentMode::Primary);
    assert_eq!(agent.info.temperature, Some(0.5));
    assert!(agent.info.permission.allow_file_writes);
    assert!(agent.info.permission.allow_network);
    assert!(agent.info.permission.allow_shell);
}

#[test]
fn test_agent_system_prompt() {
    let info = make_test_agent_info("build");
    let agent = Agent::new(info);

    // Agent should return the custom system_prompt from AgentInfo plus runtime context
    let prompt = agent.system_prompt();
    assert!(prompt.starts_with("You are a test agent."));
    assert!(prompt.contains("Today's date:"));
}

#[test]
fn test_agent_system_prompt_fallback_to_default() {
    let mut info = make_test_agent_info("build");
    info.system_prompt = None; // No custom prompt set

    let agent = Agent::new(info);

    let prompt = agent.system_prompt();
    // Should fall back to DEFAULT_SYSTEM_PROMPT (from prompts/build.txt)
    assert!(!prompt.is_empty());
    assert!(prompt.contains("you") || prompt.contains("You") || prompt.contains("agent"));
}

#[test]
fn test_agent_system_prompt_for_build() {
    let prompt = Agent::system_prompt_for("build");
    assert!(!prompt.is_empty());
}

#[test]
fn test_agent_system_prompt_for_plan() {
    let prompt = Agent::system_prompt_for("plan");
    assert!(!prompt.is_empty());
    assert!(
        prompt.to_ascii_lowercase().contains("plan"),
        "plan prompt should mention planning"
    );
}

#[test]
fn test_agent_system_prompt_for_ask() {
    let prompt = Agent::system_prompt_for("ask");
    assert!(!prompt.is_empty());
    assert!(
        prompt.to_ascii_lowercase().contains("ask")
            || prompt.to_ascii_lowercase().contains("read-only"),
        "ask prompt should describe ask/read-only mode"
    );
}

#[test]
fn test_agent_system_prompt_for_explore() {
    let prompt = Agent::system_prompt_for("explore");
    assert!(!prompt.is_empty());
}

#[test]
fn test_agent_system_prompt_for_general() {
    let prompt = Agent::system_prompt_for("general");
    assert!(!prompt.is_empty());
}

#[test]
fn test_agent_system_prompt_for_unknown_falls_back() {
    let prompt = Agent::system_prompt_for("nonexistent_agent_xyz");
    // Unknown agents fall back to DEFAULT_SYSTEM_PROMPT
    assert!(!prompt.is_empty());
}

#[test]
fn test_with_runtime_context_injects_date() {
    let prompt = Agent::with_runtime_context("You are a test agent.");
    assert!(prompt.contains("You are a test agent."));
    assert!(prompt.contains("Today's date:"));
    // YYYY-MM-DD
    let date_line = prompt
        .lines()
        .find(|l| l.starts_with("Today's date:"))
        .expect("date line");
    let date = date_line
        .trim_start_matches("Today's date:")
        .trim()
        .trim_end_matches('.');
    assert_eq!(date.len(), 10, "expected YYYY-MM-DD, got {date}");
}

#[test]
fn test_with_runtime_context_is_idempotent() {
    let once = Agent::with_runtime_context("base");
    let twice = Agent::with_runtime_context(&once);
    assert_eq!(once, twice);
    assert_eq!(once.matches("Today's date:").count(), 1);
}

#[test]
fn test_with_agents_md_includes_runtime_context() {
    let dir = std::env::temp_dir().join(format!("whycodes-agent-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let prompt = Agent::with_agents_md("base prompt", &dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(prompt.contains("base prompt"));
    assert!(prompt.contains("Today's date:"));
}

#[test]
fn test_agent_with_builder_methods() {
    use whycodes_llm::provider::ProviderRegistry;
    use whycodes_tools::executor::ToolExecutor;

    let info = make_test_agent_info("builder-test");
    let agent = Agent::new(info)
        .with_provider_registry(ProviderRegistry::default())
        .with_tool_executor(ToolExecutor::new());

    assert_eq!(agent.info.name, "builder-test");
}

// ─── SubagentTask tests ────────────────────────────────────────────────

#[test]
fn test_subagent_task_creation() {
    let task = SubagentTask {
        goal: "Write a test file".to_string(),
        context: Some("Project is a Rust workspace".to_string()),
        tools: Some(vec!["read".to_string(), "write".to_string()]),
        max_turns: 10,
    };

    assert_eq!(task.goal, "Write a test file");
    assert_eq!(task.context.as_deref(), Some("Project is a Rust workspace"));
    assert_eq!(task.tools.as_deref().unwrap().len(), 2);
    assert_eq!(task.max_turns, 10);
}

#[test]
fn test_subagent_task_no_context() {
    let task = SubagentTask {
        goal: "Simple task".to_string(),
        context: None,
        tools: None,
        max_turns: 5,
    };

    assert_eq!(task.goal, "Simple task");
    assert!(task.context.is_none());
    assert!(task.tools.is_none());
    assert_eq!(task.max_turns, 5);
}

#[test]
fn test_subagent_task_debug_format() {
    let task = SubagentTask {
        goal: "Debug me".to_string(),
        context: None,
        tools: None,
        max_turns: 3,
    };
    let debug_str = format!("{task:?}");
    assert!(debug_str.contains("Debug me"));
    assert!(debug_str.contains("max_turns: 3"));
}

// ─── Doom-loop / tool profile ──────────────────────────────────────────

#[test]
fn doom_loop_trips_on_third_identical_call() {
    use std::collections::VecDeque;
    use whycodes_core::types::ToolCall;

    let tc = ToolCall {
        id: "1".into(),
        name: "read".into(),
        arguments: serde_json::json!({"path": "a.rs"}),
    };
    let mut recent = VecDeque::new();
    assert!(!crate::agent::would_doom_loop(
        &recent,
        std::slice::from_ref(&tc)
    ));
    recent.push_back(format!(
        "read|{}",
        serde_json::to_string(&tc.arguments).unwrap()
    ));
    assert!(!crate::agent::would_doom_loop(
        &recent,
        std::slice::from_ref(&tc)
    ));
    recent.push_back(format!(
        "read|{}",
        serde_json::to_string(&tc.arguments).unwrap()
    ));
    // 2 recent + 1 new = 3 → trip
    assert!(crate::agent::would_doom_loop(
        &recent,
        std::slice::from_ref(&tc)
    ));
}

#[test]
fn doom_loop_ignores_mixed_batch() {
    use std::collections::VecDeque;
    use whycodes_core::types::ToolCall;

    let calls = vec![
        ToolCall {
            id: "1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "a.rs"}),
        },
        ToolCall {
            id: "2".into(),
            name: "grep".into(),
            arguments: serde_json::json!({"pattern": "x"}),
        },
    ];
    let mut recent = VecDeque::new();
    for _ in 0..5 {
        recent.push_back("read|{}".into());
    }
    assert!(!crate::agent::would_doom_loop(&recent, &calls));
}

#[test]
fn core_tool_profile_shrinks_definitions() {
    use whycodes_core::types::PermissionSet;
    use whycodes_tools::ToolExecutor;
    use whycodes_tools::ToolProfile;

    let ex = ToolExecutor::new();
    let perms = PermissionSet {
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        ..Default::default()
    };
    let core = ex.get_definitions_profile(&perms, ToolProfile::Core);
    let full = ex.get_definitions_profile(&perms, ToolProfile::Full);
    assert!(
        core.len() < full.len(),
        "core={} full={}",
        core.len(),
        full.len()
    );
    assert!(core.iter().any(|d| d.name == "read"));
    assert!(!core.iter().any(|d| d.name == "webfetch"));
    assert!(full.iter().any(|d| d.name == "webfetch"));
}

fn scripted_agent(steps: impl IntoIterator<Item = whycodes_llm::ScriptedStep>) -> Agent {
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::new(steps)));
    Agent::new(make_test_agent_info("build")).with_provider_registry(registry)
}

fn scripted_session(user: &str) -> whycodes_session::session::Session {
    let mut session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".into(),
    );
    session.add_user_message(user);
    session
}

#[tokio::test]
async fn scripted_text_turn_returns_assistant_text() {
    let agent = scripted_agent([whycodes_llm::ScriptedStep::Text("hello from script".into())]);
    let mut session = scripted_session("please say hello");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .expect("turn");
    assert!(out.contains("hello from script"), "got: {out}");
}

#[tokio::test]
async fn scripted_unknown_provider_errors() {
    let agent = scripted_agent([whycodes_llm::ScriptedStep::Text("x".into())]);
    let mut session = scripted_session("hi there friend");
    let err = agent
        .run_turn(&mut session, "no-such-provider", "m", "k", Some(2))
        .await
        .expect_err("unknown provider");
    assert!(
        err.to_string().to_lowercase().contains("provider") || err.to_string().contains("no-such"),
        "{err}"
    );
}

#[tokio::test]
async fn scripted_fail_open_surfaces_provider_error() {
    let agent = scripted_agent([whycodes_llm::ScriptedStep::FailOpen("boom".into())]);
    let mut session = scripted_session("please explain rust");
    let err = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await;
    assert!(err.is_err(), "expected provider error, got {err:?}");
}

#[tokio::test]
async fn scripted_tool_then_exhausted_turns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "secret note").unwrap();
    let agent = scripted_agent([whycodes_llm::ScriptedStep::ToolCall {
        id: "c1".into(),
        name: "read".into(),
        input: serde_json::json!({"path": "note.txt"}),
    }]);
    let mut session =
        whycodes_session::session::Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("please read note.txt and summarize it");
    let err = agent
        .run_turn(&mut session, "script", "m", "k", Some(1))
        .await
        .expect_err("max turns after tool");
    assert!(
        err.to_string().to_lowercase().contains("turn")
            || err.to_string().to_lowercase().contains("exceed"),
        "{err}"
    );
}

#[tokio::test]
async fn scripted_unlimited_turns_continue_after_tool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "secret note").unwrap();
    let agent = scripted_agent([
        whycodes_llm::ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            input: serde_json::json!({"path": "note.txt"}),
        },
        whycodes_llm::ScriptedStep::Text("the note is secret".into()),
    ]);
    let mut session =
        whycodes_session::session::Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("please read note.txt and summarize it");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", None)
        .await
        .expect("unlimited turns should finish after the tool");
    assert!(out.to_lowercase().contains("secret"), "got: {out}");
}

#[tokio::test]
async fn scripted_cancel_before_llm() {
    let agent = scripted_agent([whycodes_llm::ScriptedStep::Text("never".into())]);
    let mut session = scripted_session("please explain the retry loop");
    let cancel = crate::events::new_cancel_flag();
    crate::events::request_cancel(&cancel);
    let err = agent
        .run_turn_with_events(
            &mut session,
            crate::events::TurnOpts {
                provider_name: "script",
                model: "m",
                api_key: "k",
                max_turns: Some(4),
                events: None,
                cancel: Some(cancel),
            },
        )
        .await
        .expect_err("cancelled");
    assert!(err.to_string().to_lowercase().contains("cancel"), "{err}");
}

#[tokio::test]
async fn scripted_thinking_and_text_emits_events() {
    let agent = scripted_agent([
        whycodes_llm::ScriptedStep::Thinking("plan".into()),
        whycodes_llm::ScriptedStep::Text("answer".into()),
        whycodes_llm::ScriptedStep::Usage {
            input_tokens: 3,
            output_tokens: 4,
        },
    ]);
    let mut session = scripted_session("please summarize crates/agent");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            crate::events::TurnOpts {
                provider_name: "script",
                model: "m",
                api_key: "k",
                max_turns: Some(4),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .expect("turn");
    assert!(out.contains("answer"), "{out}");
    let mut saw_intent = false;
    let mut saw_status = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            crate::events::TurnEvent::Intent { .. } => saw_intent = true,
            crate::events::TurnEvent::Status(_) => saw_status = true,
            _ => {}
        }
    }
    assert!(saw_intent, "intent event");
    assert!(saw_status, "status event");
}

#[tokio::test]
async fn compact_session_local_without_key_keeps_last_user() {
    let agent = Agent::new(make_test_agent_info("build"));
    let mut session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".into(),
    );
    session.add_user_message("old request");
    session.add_assistant_message(vec![whycodes_core::types::ContentBlock::Text {
        text: "old answer".into(),
    }]);
    session.add_user_message("fix login");
    let outcome = agent
        .compact_session(&mut session, "script", "m", "", None)
        .await;
    assert!(outcome.dropped_messages());
    assert_eq!(session.messages[0].content.as_text(), Some("fix login"));
    let last = session.messages.last().unwrap().content.as_text().unwrap();
    assert!(
        last.starts_with("This session is being continued"),
        "{last}"
    );
}

#[tokio::test]
async fn compact_session_uses_llm_summary_when_scripted() {
    let summary = "<summary>\n1. Primary Request: fix login\n2. Key Technical Concepts: auth\n\
         3. Files and Code Sections: src/auth.rs\n4. Errors and Fixes: None\n\
         5. Problem Solving: None\n6. All User Messages: fix login\n\
         7. Pending Tasks: None\n8. Current Work: editing auth.rs\n\
         9. Optional Next Step: run tests\n</summary>";
    let agent = scripted_agent([whycodes_llm::ScriptedStep::Text(summary.into())]);
    let mut session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".into(),
    );
    session.add_user_message("old");
    session.add_assistant_message(vec![whycodes_core::types::ContentBlock::Text {
        text: "ack".into(),
    }]);
    session.add_user_message("fix login");
    let outcome = agent
        .compact_session(&mut session, "script", "m", "k", Some("keep auth.rs"))
        .await;
    assert!(outcome.dropped_messages());
    let last = session.messages.last().unwrap().content.as_text().unwrap();
    assert!(last.contains("fix login"), "{last}");
    assert!(last.contains("auth.rs"), "{last}");
    assert!(
        last.starts_with("This session is being continued"),
        "{last}"
    );
}

#[tokio::test]
async fn compact_session_empty_is_noop() {
    let agent = Agent::new(make_test_agent_info("build"));
    let mut session = whycodes_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".into(),
    );
    let outcome = agent
        .compact_session(&mut session, "script", "m", "", None)
        .await;
    assert_eq!(outcome.messages_before, 0);
    assert!(session.messages.is_empty());
}
