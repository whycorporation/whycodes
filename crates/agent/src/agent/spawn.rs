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
    #[test]
    fn spawn_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
