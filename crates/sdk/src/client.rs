use std::path::PathBuf;
use whycode_agent::agent::Agent;
use whycode_config::Config;
use whycode_core::types::{AgentInfo, AgentMode, PermissionSet, ToolDefinition};
use whycode_session::session::Session;

/// A client for programmatic access to whycode
pub struct WhycodeClient {
    agent: Agent,
    config: Config,
}

impl Default for WhycodeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl WhycodeClient {
    /// Create a new client from default config
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_default();
        let agent_info = config
            .default_agent()
            .cloned()
            .unwrap_or_else(|| AgentInfo {
                name: "build".to_string(),
                description: "Default agent".to_string(),
                mode: AgentMode::Primary,
                permission: PermissionSet::default(),
                model: None,
                system_prompt: None,
                temperature: None,
                top_p: None,
            });
        Self {
            agent: Agent::new(agent_info),
            config,
        }
    }

    /// Create a new session in a project directory
    pub fn create_session(&self, project_path: PathBuf) -> Session {
        Session::new(project_path, self.agent.system_prompt())
    }

    /// List available tools
    pub fn list_tools(&self) -> Vec<ToolDefinition> {
        whycode_tools::executor::ToolExecutor::new()
            .get_definitions(&whycode_core::types::PermissionSet::default())
    }

    /// List configured models
    pub fn list_models(&self) -> Vec<String> {
        self.config
            .models
            .values()
            .map(|m| format!("{}/{}", m.provider_id, m.model_id))
            .collect()
    }
}
