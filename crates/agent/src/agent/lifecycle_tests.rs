use super::*;
use whycodes_config::HookConfig;
use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};
use whycodes_llm::{ProviderRegistry, ScriptedProvider, ScriptedStep};

/// Own model id: post-turn memory retain stores replies in the process-wide
/// response cache, keyed by model, which sibling tests on `m` would hit.
const MODEL: &str = "lifecycle-m";

fn info() -> AgentInfo {
    AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: AgentMode::Primary,
        permission: PermissionSet {
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

fn hook(event: HookEvent, command: &str, block_on_failure: bool, feedback: bool) -> HookConfig {
    HookConfig {
        event,
        tool_match: "*".into(),
        command: command.into(),
        block_on_failure,
        feedback,
        timeout_secs: 10,
    }
}

/// Appends `word` to `marker`. `cmd /C` mangles quoted args, so the path
/// stays bare there (temp dirs have no spaces).
fn log_to(marker: &std::path::Path, word: &str) -> String {
    if cfg!(windows) {
        format!("echo {word}>>{}", marker.display())
    } else {
        format!("echo {word} >> '{}'", marker.display())
    }
}

fn logged(marker: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .collect()
}

fn agent_with(hooks: Vec<HookConfig>, steps: Vec<ScriptedStep>) -> Agent {
    let config = whycodes_config::Config {
        hooks,
        ..Default::default()
    };
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::repeating("script", steps)));
    Agent::new(info())
        .with_config(&config)
        .with_provider_registry(registry)
}

fn user_text(session: &Session) -> String {
    session
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter_map(|m| m.content.as_text())
        .collect::<Vec<_>>()
        .join("|")
}

#[tokio::test]
async fn turn_runs_start_prompt_and_stop_hooks_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("log.txt");
    let agent = agent_with(
        vec![
            hook(HookEvent::SessionStart, "echo branch is main", false, true),
            hook(
                HookEvent::SessionStart,
                &log_to(&marker, "start"),
                false,
                false,
            ),
            hook(
                HookEvent::UserPromptSubmit,
                "echo today is Monday",
                false,
                true,
            ),
            hook(
                HookEvent::UserPromptSubmit,
                &log_to(&marker, "prompt"),
                false,
                false,
            ),
            hook(HookEvent::Stop, &log_to(&marker, "stop"), false, false),
        ],
        vec![ScriptedStep::Text("retry loop explained".into())],
    );
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("please explain the retry loop");
    let reply = agent
        .run_turn(&mut session, "script", MODEL, "k", Some(2))
        .await
        .unwrap();
    assert_eq!(reply, "retry loop explained");
    let first = user_text(&session);
    assert!(
        first.contains("<hook-context>\nbranch is main\n</hook-context>"),
        "{first}"
    );
    assert!(first.contains("today is Monday"), "{first}");

    session.add_user_message("now explain the backoff in that retry loop");
    agent
        .run_turn(&mut session, "script", MODEL, "k", Some(2))
        .await
        .unwrap();
    assert_eq!(
        logged(&marker),
        ["start", "prompt", "stop", "prompt", "stop"]
    );
}

#[tokio::test]
async fn blocking_prompt_hook_refuses_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("log.txt");
    let agent = agent_with(
        vec![
            hook(HookEvent::UserPromptSubmit, "exit 2", true, false),
            hook(HookEvent::Stop, &log_to(&marker, "stop"), false, false),
        ],
        vec![ScriptedStep::Text("prompt hook never let this run".into())],
    );
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("deploy to prod");
    let err = agent
        .run_turn(&mut session, "script", MODEL, "k", Some(2))
        .await
        .expect_err("blocked");
    assert!(
        err.to_string().contains("user_prompt_submit hook blocked"),
        "{err}"
    );
    assert!(
        logged(&marker).is_empty(),
        "no stop hook after a refused turn"
    );
}

#[tokio::test]
async fn post_tool_feedback_reaches_the_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let config = whycodes_config::Config {
        hooks: vec![hook(HookEvent::PostTool, "echo lint: clean", false, true)],
        ..Default::default()
    };
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "r1".into(),
                name: "read".into(),
                input: serde_json::json!({"path": "n.txt"}),
            }],
            vec![ScriptedStep::Text("n.txt holds the payload".into())],
        ],
    )));
    let agent = Agent::new(info())
        .with_config(&config)
        .with_provider_registry(registry);
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("please read n.txt for me");
    agent
        .run_turn(&mut session, "script", MODEL, "k", Some(4))
        .await
        .unwrap();
    let results: String = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.content.as_text())
        .collect();
    assert!(
        results.contains("[post_tool hook]\nlint: clean"),
        "{results}"
    );
}

#[tokio::test]
async fn compaction_runs_pre_compact_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("log.txt");
    let agent = agent_with(
        vec![hook(
            HookEvent::PreCompact,
            &log_to(&marker, "compact"),
            false,
            false,
        )],
        vec![ScriptedStep::Text("x".into())],
    );
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("old");
    session.add_assistant_message(vec![ContentBlock::Text { text: "ack".into() }]);
    agent
        .compact_session(&mut session, "script", MODEL, "", None)
        .await;
    assert_eq!(logged(&marker), ["compact"]);
}

#[test]
fn turn_context_goes_to_the_latest_user_message() {
    let mut empty = Session::new("/p".into(), "s".into());
    add_turn_context(&mut empty, Some("ignored".into()));
    assert!(empty.messages.is_empty());

    let mut session = Session::new("/p".into(), "s".into());
    session.add_user_message_blocks(vec![ContentBlock::Text { text: "hi".into() }]);
    session.add_assistant_message(vec![ContentBlock::Text { text: "yo".into() }]);
    add_turn_context(&mut session, None);
    add_turn_context(&mut session, Some("ctx".into()));
    let MessageContent::Blocks(blocks) = &session.messages[0].content else {
        panic!("blocks");
    };
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[1], ContentBlock::Text { text } if text.contains("ctx")));
}

/// Shell snippet that prints a JSON object literally.
fn json_echo(json: &str) -> String {
    if cfg!(windows) {
        format!("echo {json}")
    } else {
        format!("echo '{json}'")
    }
}

/// Appends the value of env var `name` to `marker`.
fn log_var_to(marker: &std::path::Path, name: &str) -> String {
    if cfg!(windows) {
        // A space before `>>`: `echo 0>>file` would redirect handle 0.
        format!("echo %{name}% >>{}", marker.display())
    } else {
        format!("echo \"${name}\" >> '{}'", marker.display())
    }
}

fn tool_results(session: &Session) -> String {
    session
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.content.as_text())
        .collect()
}

#[tokio::test]
async fn transform_hook_rewrites_arguments_before_the_gates() {
    let dir = tempfile::tempdir().unwrap();
    let config = whycodes_config::Config {
        hooks: vec![
            HookConfig {
                tool_match: "write".into(),
                ..hook(
                    HookEvent::PreToolTransform,
                    &json_echo(r#"{"path":"moved.txt","content":"x"}"#),
                    false,
                    false,
                )
            },
            HookConfig {
                tool_match: "bash".into(),
                ..hook(
                    HookEvent::PreToolTransform,
                    &json_echo(r#"{"command":"rm -rf /"}"#),
                    false,
                    false,
                )
            },
        ],
        ..Default::default()
    };
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![
                ScriptedStep::ToolCall {
                    id: "w1".into(),
                    name: "write".into(),
                    input: serde_json::json!({"path": "orig.txt", "content": "x"}),
                },
                ScriptedStep::ToolCall {
                    id: "b1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "echo hi"}),
                },
            ],
            vec![ScriptedStep::Text("done".into())],
        ],
    )));
    let agent = Agent::new(info())
        .with_config(&config)
        .with_provider_registry(registry);
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("write the file and say hi");
    agent
        .run_turn(&mut session, "script", MODEL, "k", Some(4))
        .await
        .unwrap();
    let listing: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    assert!(
        dir.path().join("moved.txt").exists(),
        "transformed path is written; dir = {listing:?}; results = {}",
        tool_results(&session)
    );
    assert!(
        !dir.path().join("orig.txt").exists(),
        "original path is not written"
    );
    let results = tool_results(&session);
    assert!(
        results.contains("catastrophic"),
        "the risk gate must judge the transformed command: {results}"
    );
}

#[tokio::test]
async fn stop_hook_gets_turn_and_session_token_counts() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("tokens.txt");
    let agent = agent_with(
        vec![
            hook(
                HookEvent::Stop,
                &log_var_to(&marker, "WHYCODES_TURN_OUTPUT_TOKENS"),
                false,
                false,
            ),
            hook(
                HookEvent::Stop,
                &log_var_to(&marker, "WHYCODES_SESSION_INPUT_TOKENS"),
                false,
                false,
            ),
        ],
        vec![ScriptedStep::Text("counted".into())],
    );
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("count this");
    agent
        .run_turn(&mut session, "script", MODEL, "k", Some(2))
        .await
        .unwrap();
    let lines = logged(&marker);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        line.parse::<u64>()
            .unwrap_or_else(|_| panic!("token counter must be a number: {line:?}"));
    }
    let turn_out: u64 = lines[0].parse().unwrap();
    assert_eq!(turn_out, session.usage.output_tokens);
}

#[tokio::test]
async fn secret_placeholders_are_restored_before_the_tool_runs() {
    let dir = tempfile::tempdir().unwrap();
    let redactor = std::sync::Arc::new(whycodes_llm::Redactor::with_key([4u8; 32]));
    let placeholder = redactor.register("real-api-key-value-123");
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "w1".into(),
                name: "write".into(),
                input: serde_json::json!({"path": "config.env", "content": format!("KEY={placeholder}\n")}),
            }],
            vec![ScriptedStep::Text("written".into())],
        ],
    )));
    let agent = Agent::new(info())
        .with_provider_registry(registry)
        .with_redactor(Some(std::sync::Arc::clone(&redactor)));
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message("write the env file");
    agent
        .run_turn(&mut session, "script", MODEL, "k", Some(4))
        .await
        .unwrap();
    let written = std::fs::read_to_string(dir.path().join("config.env")).unwrap();
    assert_eq!(written, "KEY=real-api-key-value-123\n");

    // No redactor: the placeholder is written literally.
    let dir2 = tempfile::tempdir().unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "w1".into(),
                name: "write".into(),
                input: serde_json::json!({"path": "config.env", "content": format!("KEY={placeholder}\n")}),
            }],
            vec![ScriptedStep::Text("written".into())],
        ],
    )));
    let plain = Agent::new(info())
        .with_provider_registry(registry)
        .with_redactor(None);
    let mut session = Session::new(dir2.path().to_path_buf(), "test".into());
    session.add_user_message("write the env file");
    plain
        .run_turn(&mut session, "script", MODEL, "k", Some(4))
        .await
        .unwrap();
    let literal = std::fs::read_to_string(dir2.path().join("config.env")).unwrap();
    assert!(literal.contains("$$SECRET:"), "{literal}");
}
