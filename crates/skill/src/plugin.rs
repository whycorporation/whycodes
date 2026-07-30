use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use whycode_core::types::ToolResult;

/// Configuration for a plugin — a user-defined external tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Unique plugin name.
    pub name: String,
    /// Shell command to execute.
    pub command: String,
    /// Human-readable description (used in tool definitions).
    pub description: String,
    /// Optional JSON Schema for the parameters the plugin accepts.
    pub parameters: Option<serde_json::Value>,
}

/// A ready-to-execute plugin instance.
pub struct Plugin {
    config: PluginConfig,
}

impl Plugin {
    /// Create a new plugin from its configuration.
    pub fn new(config: PluginConfig) -> Self {
        Self { config }
    }

    /// Return the plugin's name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Return the plugin's description.
    pub fn description(&self) -> &str {
        &self.config.description
    }

    /// Return the JSON Schema parameters block, if any.
    pub fn parameters(&self) -> Option<&serde_json::Value> {
        self.config.parameters.as_ref()
    }

    /// Build the platform's shell invocation for `command`.
    fn shell_command(command: &str) -> tokio::process::Command {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C");
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c");
            c
        };
        cmd.arg(command);
        cmd
    }

    /// Execute the plugin command.
    ///
    /// `args` are passed as environment variables to the child process (keys
    /// are uppercased and prefixed).  `_ctx` is reserved for future context
    /// injection (e.g. workspace path, session info).
    pub async fn execute(
        &self,
        args: &HashMap<String, String>,
        _ctx: &PluginContext,
    ) -> ToolResult {
        let mut cmd = Self::shell_command(&self.config.command);

        // Inject args as environment variables
        for (k, v) in args {
            cmd.env(format!("PLUGIN_ARG_{}", k.to_uppercase()), v);
        }

        // Inject context as environment variables
        if let Some(ref workspace) = _ctx.workspace_path {
            cmd.env("PLUGIN_WORKSPACE", workspace);
        }

        let tool_call_id = format!("plugin-{}", self.config.name);

        match cmd.output().await {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    ToolResult {
                        tool_call_id,
                        content: stdout,
                        is_error: false,
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let content = if stderr.is_empty() { stdout } else { stderr };
                    ToolResult {
                        tool_call_id,
                        content,
                        is_error: true,
                    }
                }
            }
            Err(e) => ToolResult {
                tool_call_id,
                content: format!("Failed to execute plugin '{}': {}", self.config.name, e),
                is_error: true,
            },
        }
    }
}

/// Context passed to a plugin at execution time.
#[derive(Debug, Clone, Default)]
pub struct PluginContext {
    /// Absolute path to the project workspace.
    pub workspace_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plugin_execute_echo() {
        // Plugin commands are written in the host shell's syntax, so the
        // variable reference differs between cmd.exe and sh.
        let command = if cfg!(windows) {
            "echo hello %PLUGIN_ARG_NAME%"
        } else {
            "echo hello $PLUGIN_ARG_NAME"
        };
        let config = PluginConfig {
            name: "echo".into(),
            command: command.into(),
            description: "test echo".into(),
            parameters: None,
        };
        let plugin = Plugin::new(config);
        let mut args = HashMap::new();
        args.insert("name".into(), "world".into());
        let ctx = PluginContext::default();
        let result = plugin.execute(&args, &ctx).await;
        assert!(!result.is_error);
        assert!(
            result.content.contains("hello world"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn plugin_execute_failure() {
        let config = PluginConfig {
            name: "fail".into(),
            command: "exit 1".into(),
            description: "always fails".into(),
            parameters: None,
        };
        let plugin = Plugin::new(config);
        let result = plugin
            .execute(&HashMap::new(), &PluginContext::default())
            .await;
        assert!(result.is_error);
    }
}
