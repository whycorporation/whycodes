use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use whycodes_core::types::ToolResult;

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
    /// Working directory for the child process (`plugin.json` directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
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
        if let Some(ref dir) = self.config.working_dir {
            cmd.current_dir(dir);
        }

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
        #[cfg(windows)]
        let command = "echo hello %PLUGIN_ARG_NAME%";
        #[cfg(not(windows))]
        let command = "echo hello $PLUGIN_ARG_NAME";
        let config = PluginConfig {
            name: "echo".into(),
            command: command.into(),
            description: "test echo".into(),
            parameters: None,
            working_dir: None,
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
            working_dir: None,
        };
        let plugin = Plugin::new(config);
        let result = plugin
            .execute(&HashMap::new(), &PluginContext::default())
            .await;
        assert!(result.is_error);
    }

    #[test]
    fn accessors_expose_config() {
        let params = serde_json::json!({"type": "object"});
        let plugin = Plugin::new(PluginConfig {
            name: "n".into(),
            command: "true".into(),
            description: "desc".into(),
            parameters: Some(params.clone()),
            working_dir: Some("/tmp".into()),
        });
        assert_eq!(plugin.name(), "n");
        assert_eq!(plugin.description(), "desc");
        assert_eq!(plugin.parameters(), Some(&params));
    }

    #[tokio::test]
    async fn execute_uses_working_dir_and_workspace_env() {
        let dir = tempfile::tempdir().unwrap();
        let config = PluginConfig {
            name: "cwd".into(),
            command: "pwd; echo ws=$PLUGIN_WORKSPACE".into(),
            description: "d".into(),
            parameters: None,
            working_dir: Some(dir.path().display().to_string()),
        };
        let plugin = Plugin::new(config);
        let ctx = PluginContext {
            workspace_path: Some(dir.path().display().to_string()),
        };
        let result = plugin.execute(&HashMap::new(), &ctx).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(
            result.content.contains(&dir.path().display().to_string()),
            "{}",
            result.content
        );
        assert!(result.content.contains("ws="), "{}", result.content);
    }

    #[tokio::test]
    async fn execute_failure_prefers_stderr_then_stdout() {
        let stderr_cfg = PluginConfig {
            name: "se".into(),
            command: "echo out; echo err >&2; exit 1".into(),
            description: "d".into(),
            parameters: None,
            working_dir: None,
        };
        let stderr_result = Plugin::new(stderr_cfg)
            .execute(&HashMap::new(), &PluginContext::default())
            .await;
        assert!(stderr_result.is_error);
        assert!(
            stderr_result.content.contains("err"),
            "{}",
            stderr_result.content
        );

        let stdout_cfg = PluginConfig {
            name: "so".into(),
            command: "echo only-out; exit 1".into(),
            description: "d".into(),
            parameters: None,
            working_dir: None,
        };
        let stdout_result = Plugin::new(stdout_cfg)
            .execute(&HashMap::new(), &PluginContext::default())
            .await;
        assert!(stdout_result.is_error);
        assert!(
            stdout_result.content.contains("only-out"),
            "{}",
            stdout_result.content
        );
    }

    #[tokio::test]
    async fn execute_spawn_error() {
        let config = PluginConfig {
            name: "gone".into(),
            command: "true".into(),
            description: "d".into(),
            parameters: None,
            working_dir: Some("/no/such/plugin-cwd-whycodes".into()),
        };
        let result = Plugin::new(config)
            .execute(&HashMap::new(), &PluginContext::default())
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("Failed to execute plugin"),
            "{}",
            result.content
        );
    }
}
