use super::*;
use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};
use whycodes_llm::{ProviderRegistry, ScriptedProvider, ScriptedStep};

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

fn scripted(steps: impl IntoIterator<Item = ScriptedStep>) -> Agent {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::repeating("script", steps)));
    Agent::new(info()).with_provider_registry(registry)
}

#[tokio::test]
async fn spawn_subagent_returns_text() {
    let dir = tempfile::tempdir().unwrap();
    let agent = scripted([ScriptedStep::Text("worker done".into())]);
    let out = agent
        .spawn_subagent(
            "inspect the project".into(),
            Some("use the notes".into()),
            None,
            3,
            "script",
            "m",
            "k",
            dir.path().to_path_buf(),
        )
        .await
        .expect("spawn");
    assert!(
        out.contains("worker") || out.to_lowercase().contains("subagent"),
        "{out}"
    );
}

#[tokio::test]
async fn spawn_subagent_wraps_provider_error() {
    let dir = tempfile::tempdir().unwrap();
    let agent = scripted([ScriptedStep::FailOpen("boom".into())]);
    let out = agent
        .spawn_subagent(
            "inspect".into(),
            None,
            Some(vec!["read".into()]),
            2,
            "script",
            "m",
            "k",
            dir.path().to_path_buf(),
        )
        .await
        .expect("spawn always Ok wrapping");
    assert!(
        out.to_lowercase().contains("subagent error") || out.to_lowercase().contains("boom"),
        "{out}"
    );
}

#[tokio::test]
async fn spawn_parallel_zero_concurrent_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let agent = scripted([ScriptedStep::Text("ok".into())]);
    let outs = agent
        .spawn_parallel(
            vec![
                SubagentTask {
                    goal: "one".into(),
                    context: None,
                    tools: None,
                    max_turns: 2,
                },
                SubagentTask {
                    goal: "two".into(),
                    context: Some("ctx".into()),
                    tools: Some(vec!["read".into()]),
                    max_turns: 2,
                },
            ],
            0,
            "script",
            "m",
            "k",
            dir.path().to_path_buf(),
        )
        .await
        .expect("parallel");
    assert_eq!(outs.len(), 2);
}

#[tokio::test]
async fn spawn_parallel_join_error_from_aborted_task() {
    use crate::subagent::SubagentRunner;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let dir = tempfile::tempdir().unwrap();
    let agent = scripted([ScriptedStep::Hang(std::time::Duration::from_secs(30))]);
    let runner = Arc::new(SubagentRunner::new(
        Arc::clone(&agent.provider_registry),
        Arc::clone(&agent.tool_executor),
        agent.info.clone(),
        dir.path().to_path_buf(),
        agent.sandbox.clone(),
        agent.network.clone(),
    ));
    let sem = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&sem);
    let r = Arc::clone(&runner);
    let handle = tokio::spawn(async move {
        let _guard = permit.acquire().await;
        r.run(
            SubagentTask {
                goal: "hang".into(),
                context: None,
                tools: None,
                max_turns: 2,
            },
            "script",
            "m",
            "k",
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    handle.abort();
    let joined = handle.await;
    assert!(joined.is_err(), "{joined:?}");
    let msg = format!("Join error: {}", joined.unwrap_err());
    assert!(msg.to_lowercase().contains("join") || msg.to_lowercase().contains("cancel"));
}

struct PanicOnStreamProvider;

impl whycodes_llm::LlmProvider for PanicOnStreamProvider {
    fn name(&self) -> &str {
        "panic-stream"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::llm("complete-only")) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        panic!("spawn-parallel join coverage");
    }
}

#[tokio::test]
async fn spawn_parallel_join_error_when_worker_panics() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(PanicOnStreamProvider));
    let agent = Agent::new(info()).with_provider_registry(registry);
    let outs = agent
        .spawn_parallel(
            vec![SubagentTask {
                goal: "panic".into(),
                context: None,
                tools: None,
                max_turns: 1,
            }],
            1,
            "panic-stream",
            "m",
            "k",
            dir.path().to_path_buf(),
        )
        .await
        .expect("join error is Ok(vec)");
    assert_eq!(outs.len(), 1);
    assert!(
        outs[0].to_lowercase().contains("join error") || outs[0].to_lowercase().contains("panic"),
        "{}",
        outs[0]
    );
}
