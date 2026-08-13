use crate::agent::Agent;
use crate::subagent::SubagentTask;
use whycode_core::types::{AgentInfo, AgentMode, PermissionAction, PermissionSet};

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
// Classification itself is covered by `whycode-command-risk`. These tests
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

fn bash_call(command: &str) -> whycode_core::types::ToolCall {
    whycode_core::types::ToolCall {
        id: "tc-1".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({ "command": command }),
    }
}

async fn run_bash(agent: &Agent, command: &str) -> whycode_core::types::ToolResult {
    let session = whycode_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".to_string(),
    );
    let ctx = whycode_core::ToolContext {
        working_dir: "/work/proj".to_string(),
        session_id: None,
        sandbox: whycode_core::SandboxSettings::off(),
        network: whycode_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
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
/// `whycode-command-risk`, where classification is a pure function and nothing
/// is executed.
const HARMLESS_CATASTROPHIC: &str = "mkfs.whycode-test-not-a-real-binary /dev/null";

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

    let session = whycode_session::session::Session::new(
        std::path::PathBuf::from("/work/proj"),
        "test".to_string(),
    );
    let ctx = whycode_core::ToolContext {
        working_dir: "/work/proj".to_string(),
        session_id: None,
        sandbox: whycode_core::SandboxSettings::off(),
        network: whycode_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        swarm_hub: None,
    };
    let call = whycode_core::types::ToolCall {
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
    let dir = std::env::temp_dir().join(format!("whycode-agent-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let prompt = Agent::with_agents_md("base prompt", &dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(prompt.contains("base prompt"));
    assert!(prompt.contains("Today's date:"));
}

#[test]
fn test_agent_with_builder_methods() {
    use whycode_llm::provider::ProviderRegistry;
    use whycode_tools::executor::ToolExecutor;

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
    use whycode_core::types::ToolCall;

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
    use whycode_core::types::ToolCall;

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
    use whycode_core::types::PermissionSet;
    use whycode_tools::ToolExecutor;
    use whycode_tools::ToolProfile;

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
