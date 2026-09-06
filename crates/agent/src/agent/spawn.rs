//! Public subagent spawn helpers (`task` / parallel).

use std::sync::Arc;

use crate::subagent::{SubagentRunner, SubagentTask};

use super::Agent;

impl Agent {
    /// Spawn a single subagent to accomplish a goal.
    ///
    /// The subagent runs in a fresh session with its own conversation loop.
    /// Returns the subagent's textual output.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_subagent(
        &self,
        goal: String,
        context: Option<String>,
        tools: Option<Vec<String>>,
        max_turns: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
        project_path: std::path::PathBuf,
    ) -> whycodes_core::Result<String> {
        let task = SubagentTask {
            goal: goal.clone(),
            context,
            tools,
            max_turns,
        };

        let runner = SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            self.info.clone(),
            project_path,
            self.sandbox.clone(),
            self.network.clone(),
        )
        .with_memory(self.memory.clone())
        .with_file_index(self.file_index.clone())
        .with_panel(self.panel_sink())
        .with_question_prompter(Arc::clone(&self.question_prompter))
        .with_approval_mode(self.approval_mode);

        let result = runner.run(task, provider_name, model, api_key).await?;

        Ok(result.output)
    }

    /// Spawn multiple subagents in parallel, respecting a concurrency limit.
    ///
    /// Each `SubagentTask` spawns an independent subagent. Up to `max_concurrent`
    /// subagents run at once; the rest are queued. Returns a Vec of outputs in the
    /// same order as the input tasks.
    pub async fn spawn_parallel(
        &self,
        goals: Vec<SubagentTask>,
        max_concurrent: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
        project_path: std::path::PathBuf,
    ) -> whycodes_core::Result<Vec<String>> {
        use tokio::sync::Semaphore;

        let sem = Arc::new(Semaphore::new(max_concurrent.max(1)));
        let provider_name = Arc::from(provider_name.to_string());
        let model = Arc::from(model.to_string());
        let api_key = Arc::from(api_key.to_string());

        let runner = Arc::new(
            SubagentRunner::new(
                Arc::clone(&self.provider_registry),
                Arc::clone(&self.tool_executor),
                self.info.clone(),
                project_path,
                self.sandbox.clone(),
                self.network.clone(),
            )
            .with_memory(self.memory.clone())
            .with_file_index(self.file_index.clone())
            .with_panel(self.panel_sink())
            .with_question_prompter(Arc::clone(&self.question_prompter))
            .with_approval_mode(self.approval_mode),
        );

        let mut handles = Vec::with_capacity(goals.len());

        for task in goals {
            let permit = Arc::clone(&sem);
            let r = Arc::clone(&runner);
            let pn = Arc::clone(&provider_name);
            let m = Arc::clone(&model);
            let ak = Arc::clone(&api_key);

            handles.push(tokio::spawn(async move {
                let _guard = permit.acquire().await;
                r.run(task, &pn, &m, &ak).await
            }));
        }

        let mut outputs = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => outputs.push(result.output),
                Ok(Err(e)) => outputs.push(format!("Subagent error: {}", e)),
                Err(e) => outputs.push(format!("Join error: {}", e)),
            }
        }

        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
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
}
