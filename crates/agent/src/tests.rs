#[cfg(test)]
mod tests {
    use crate::agent::Agent;
    use crate::subagent::SubagentTask;
    use whycode_core::types::{AgentInfo, AgentMode, PermissionSet};

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
            },
            model: None,
            system_prompt: Some("You are a test agent.".to_string()),
            temperature: Some(0.5),
            top_p: None,
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

        // Agent should return the custom system_prompt from AgentInfo
        let prompt = agent.system_prompt();
        assert_eq!(prompt, "You are a test agent.");
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
        assert_eq!(
            task.context.as_deref(),
            Some("Project is a Rust workspace")
        );
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
}
