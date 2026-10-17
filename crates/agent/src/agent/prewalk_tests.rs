use super::*;
use crate::events::{TurnEvent, TurnOpts};
use serde_json::json;
use std::sync::Arc;
use whycodes_core::types::{AgentInfo, AgentMode, LlmRequest, PermissionSet};
use whycodes_llm::{ProviderRegistry, ScriptedProvider, ScriptedStep};
use whycodes_session::session::Session;

type ModelCalls = Arc<std::sync::Mutex<Vec<String>>>;

/// A scripted provider that records the model of every stream it opens.
struct ModelLog {
    inner: ScriptedProvider,
    calls: ModelCalls,
}

impl LlmProvider for ModelLog {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn default_base_url(&self) -> &str {
        self.inner.default_base_url()
    }
    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        self.inner.complete(request, api_key, model)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(model.to_string());
        self.inner.stream(request, api_key, model)
    }
}

fn info() -> AgentInfo {
    AgentInfo {
        name: "build".into(),
        description: "test".into(),
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
        system_prompt: Some("You are a test agent.".into()),
        temperature: None,
        top_p: None,
    }
}

fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: input,
    }
}

fn tool(id: &str, name: &str, input: serde_json::Value) -> ScriptedStep {
    ScriptedStep::ToolCall {
        id: id.into(),
        name: name.into(),
        input,
    }
}

fn registry(name: &str, batches: Vec<Vec<ScriptedStep>>, calls: &ModelCalls) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ModelLog {
        inner: ScriptedProvider::batched(name, batches),
        calls: Arc::clone(calls),
    }));
    registry
}

fn agent(
    config: &whycodes_config::Config,
    name: &str,
    batches: Vec<Vec<ScriptedStep>>,
    calls: &ModelCalls,
) -> Agent {
    Agent::new(info())
        .with_config(config)
        .with_provider_registry(registry(name, batches, calls))
}

fn prewalk_config() -> whycodes_config::Config {
    let mut config = whycodes_config::Config::default();
    config.session.prewalk = Some(true);
    config
}

/// Read, write, answer: the plan, the first edit, the cheap step.
fn edit_turn() -> Vec<Vec<ScriptedStep>> {
    vec![
        vec![tool("r1", "read", json!({"path": "a.txt"}))],
        vec![tool(
            "w1",
            "write",
            json!({"path": "b.txt", "content": "x"}),
        )],
        vec![ScriptedStep::Text("done".into())],
    ]
}

fn statuses(rx: &mut tokio::sync::mpsc::UnboundedReceiver<TurnEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Status(s) = ev {
            out.push(s);
        }
    }
    out
}

async fn run(agent: &Agent, dir: &Path, model: &str) -> (String, Vec<String>) {
    let mut session = Session::new(dir.to_path_buf(), "test".into());
    session.add_user_message("please fix the bug in a.txt");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let text = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "openai",
                model,
                api_key: "sk-a",
                max_turns: Some(8),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .unwrap();
    (text, statuses(&mut rx))
}

fn take(calls: &ModelCalls) -> Vec<String> {
    std::mem::take(&mut *calls.lock().unwrap())
}

#[test]
fn edits_and_unsafe_shell_are_mutating() {
    let dir = Path::new("/work/proj");
    for name in ["edit", "write", "apply_patch", "ast_edit"] {
        assert!(is_mutating_call(&call(name, json!({})), dir), "{name}");
    }
    assert!(!is_mutating_call(&call("read", json!({"path": "a"})), dir));
    assert!(!is_mutating_call(
        &call("bash", json!({"command": "ls"})),
        dir
    ));
    assert!(!is_mutating_call(&call("bash", json!({})), dir));
    assert!(is_mutating_call(
        &call("bash", json!({"command": "rm -rf build"})),
        dir
    ));
}

#[tokio::test]
async fn hands_off_after_the_first_edit_and_until_restart() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    let calls = ModelCalls::default();
    let mut batches = edit_turn();
    batches.push(vec![ScriptedStep::Text("again".into())]);
    batches.push(vec![ScriptedStep::Text("planned".into())]);
    let agent = agent(&prewalk_config(), "openai", batches, &calls);
    assert!(agent.prewalk_enabled());

    let (text, status) = run(&agent, dir.path(), "gpt-4o").await;
    assert_eq!(text, "done");
    assert_eq!(
        take(&calls),
        ["gpt-4o", "gpt-4o", "gpt-4o-mini"],
        "the edit step itself ran on the strong model"
    );
    assert!(
        status
            .iter()
            .any(|s| s == "Prewalk: handing off to openai/gpt-4o-mini after the plan"),
        "{status:?}"
    );

    let (_, status) = run(&agent, dir.path(), "gpt-4o").await;
    assert_eq!(take(&calls), ["gpt-4o-mini"]);
    assert!(
        status
            .iter()
            .any(|s| s.starts_with("Prewalk: continuing on openai/gpt-4o-mini")),
        "{status:?}"
    );

    assert!(agent.prewalk_restart());
    assert!(!agent.prewalk_restart(), "nothing left to clear");
    let (text, _) = run(&agent, dir.path(), "gpt-4o").await;
    assert_eq!(text, "planned");
    assert_eq!(take(&calls), ["gpt-4o"]);
}

#[tokio::test]
async fn a_new_strong_route_plans_again() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    let calls = ModelCalls::default();
    let mut batches = edit_turn();
    batches.push(vec![ScriptedStep::Text("other".into())]);
    let agent = agent(&prewalk_config(), "openai", batches, &calls);
    run(&agent, dir.path(), "gpt-4o").await;
    take(&calls);
    run(&agent, dir.path(), "gpt-4.1").await;
    assert_eq!(take(&calls), ["gpt-4.1"]);
}

#[tokio::test]
async fn read_only_turns_never_hand_off() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    let calls = ModelCalls::default();
    let batches = vec![
        vec![tool("r1", "read", json!({"path": "a.txt"}))],
        vec![tool("b1", "bash", json!({"command": "ls"}))],
        vec![ScriptedStep::Text("explained".into())],
    ];
    let agent = agent(&prewalk_config(), "openai", batches, &calls);
    let (_, status) = run(&agent, dir.path(), "gpt-4o").await;
    assert_eq!(take(&calls), ["gpt-4o", "gpt-4o", "gpt-4o"]);
    assert!(!status.iter().any(|s| s.contains("Prewalk")), "{status:?}");
    assert!(!agent.prewalk_restart());
}

#[tokio::test]
async fn disabled_by_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    let calls = ModelCalls::default();
    let config = whycodes_config::Config::default();
    let agent = agent(&config, "openai", edit_turn(), &calls);
    assert!(!agent.prewalk_enabled());
    run(&agent, dir.path(), "gpt-4o").await;
    assert_eq!(take(&calls), ["gpt-4o", "gpt-4o", "gpt-4o"]);
}

#[tokio::test]
async fn plans_on_model_plan_and_hands_off_to_model_smol() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    let calls = ModelCalls::default();
    let mut config = prewalk_config();
    config.session.model_plan = Some("o3".into());
    config.session.model_smol = Some("cheap-coder".into());
    let agent = agent(&config, "openai", edit_turn(), &calls);
    run(&agent, dir.path(), "gpt-4o").await;
    assert_eq!(take(&calls), ["o3", "o3", "cheap-coder"]);
}

#[tokio::test]
async fn no_cheap_target_stays_on_the_strong_model() {
    let _env = crate::CREDENTIAL_ENV_LOCK.lock().await;
    unsafe { std::env::remove_var("WCPWNOKEY_API_KEY") };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    // No small sibling for this provider; an unregistered provider; a
    // registered provider without a key.
    let cases: [(&str, &str, Option<&str>); 3] = [
        ("wcpwlocal", "big", None),
        ("openai", "gpt-4o", Some("nowhere/x")),
        ("openai", "gpt-4o", Some("wcpwnokey/x")),
    ];
    for (provider, model, smol) in cases {
        let calls = ModelCalls::default();
        let mut config = prewalk_config();
        config.session.model_smol = smol.map(str::to_string);
        let mut registry = registry(provider, edit_turn(), &calls);
        registry.register(Box::new(ScriptedProvider::named("wcpwnokey", [])));
        let agent = Agent::new(info())
            .with_config(&config)
            .with_provider_registry(registry);
        let mut session = Session::new(dir.path().to_path_buf(), "test".into());
        session.add_user_message("please fix the bug in a.txt");
        agent
            .run_turn(&mut session, provider, model, "sk-a", Some(8))
            .await
            .unwrap();
        assert_eq!(take(&calls), [model, model, model], "{provider} {smol:?}");
        assert!(!agent.prewalk_restart());
    }
}

#[tokio::test]
async fn subagent_hands_off_after_its_first_edit() {
    use crate::subagent::{SubagentRunner, SubagentTask};
    let dir = tempfile::tempdir().unwrap();
    let task = || SubagentTask {
        goal: "fix it".into(),
        context: None,
        tools: None,
        max_turns: 6,
    };
    for (target, expect) in [
        (("openai", "gpt-4o-mini"), "gpt-4o-mini"),
        (("nowhere", "x"), "gpt-4o"),
    ] {
        let calls = ModelCalls::default();
        let batches = vec![
            vec![tool("r1", "read", json!({"path": "a.txt"}))],
            vec![tool("w1", "edit", json!({"path": "a.txt"}))],
            vec![ScriptedStep::Text("done".into())],
        ];
        let runner = SubagentRunner::new(
            Arc::new(registry("openai", batches, &calls)),
            Arc::new(whycodes_tools::executor::ToolExecutor::new()),
            info(),
            dir.path().to_path_buf(),
            whycodes_core::SandboxSettings::off(),
            whycodes_core::network::NetworkPolicy::unrestricted(),
        )
        .with_prewalk(Some((target.0.to_string(), target.1.to_string())));
        let result = runner.run(task(), "openai", "gpt-4o", "sk-a").await;
        assert!(result.success, "{}", result.output);
        assert_eq!(take(&calls), ["gpt-4o", "gpt-4o", expect], "{target:?}");
    }
}
