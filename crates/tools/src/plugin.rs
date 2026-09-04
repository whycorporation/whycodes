//! Shell plugins from `plugins.toml` and `plugin.json` trees as agent tools.

use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::tool::{Tool, ToolContext};
use whycodes_skill::plugin::{Plugin, PluginConfig, PluginContext};

/// One external shell plugin registered as an LLM tool (`plugin_<name>`).
pub struct PluginShellTool {
    plugin: Arc<Plugin>,
    tool_name: String,
}

impl PluginShellTool {
    pub fn from_config(cfg: PluginConfig) -> Self {
        let tool_name = format!("plugin_{}", cfg.name);
        Self {
            plugin: Arc::new(Plugin::new(cfg)),
            tool_name,
        }
    }
}
impl Tool for PluginShellTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        self.plugin.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.plugin.parameters().cloned().unwrap_or_else(|| {
            json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Optional free-form input (PLUGIN_ARG_INPUT)"
                    }
                }
            })
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let mut map = HashMap::new();
            if let Some(obj) = args.as_object() {
                for (k, v) in obj {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    map.insert(k.clone(), s);
                }
            }
            let pctx = PluginContext {
                workspace_path: Some(ctx.working_dir.clone()),
            };
            let mut result = self.plugin.execute(&map, &pctx).await;
            result.tool_call_id = String::new();
            result
        })
    }
}

/// One row for `whycodes plugins` (toml + json, last-wins by tool name).
#[derive(Debug, Clone)]
pub struct ListedPlugin {
    pub tool_name: String,
    pub command: String,
    pub description: String,
    pub origin: String,
}

/// Same merge order as [`crate::executor::ToolExecutor::register_config_plugins`].
pub fn list_shell_plugins(project_dir: Option<&std::path::Path>) -> Vec<ListedPlugin> {
    let mut by_name = std::collections::BTreeMap::new();

    let toml = match project_dir {
        Some(dir) => whycodes_skill::PluginRegistry::load_layered(dir).unwrap_or_default(),
        None => whycodes_skill::PluginRegistry::load_from_config().unwrap_or_default(),
    };
    for cfg in toml.plugins {
        if cfg.name.trim().is_empty() || cfg.command.trim().is_empty() {
            continue;
        }
        by_name.insert(
            cfg.name.clone(),
            ListedPlugin {
                tool_name: format!("plugin_{}", cfg.name),
                command: cfg.command,
                description: cfg.description,
                origin: "plugins.toml".into(),
            },
        );
    }

    let mut mgr = whycodes_plugin::PluginManager::new();
    mgr.discover_standard(project_dir);
    for spec in mgr.shell_specs() {
        if spec.name.trim().is_empty() || spec.command.trim().is_empty() {
            continue;
        }
        by_name.insert(
            spec.name.clone(),
            ListedPlugin {
                tool_name: format!("plugin_{}", spec.name),
                command: spec.command,
                description: spec.description,
                origin: spec.origin,
            },
        );
    }

    by_name.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ToolExecutor;

    #[test]
    fn register_discovers_project_plugin_json() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".whycodes").join("plugins").join("echo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"whycodes_test_echojson","command":"echo from-json","description":"json plugin"}"#,
        )
        .unwrap();

        let mut exec = ToolExecutor::new();
        let n = exec.register_config_plugins(Some(tmp.path()));
        assert!(n >= 1, "registered {n}");
        assert!(
            exec.get("plugin_whycodes_test_echojson").is_some(),
            "tools: {:?}",
            exec.tool_names()
        );
    }

    #[test]
    fn json_overrides_toml_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        let why = tmp.path().join(".whycodes");
        std::fs::create_dir_all(why.join("plugins").join("hello")).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"
[[plugins]]
name = "whycodes_test_hello"
command = "echo toml"
description = "from toml"
"#,
        )
        .unwrap();
        std::fs::write(
            why.join("plugins").join("hello").join("plugin.json"),
            r#"{"name":"whycodes_test_hello","command":"echo json"}"#,
        )
        .unwrap();

        let listed = list_shell_plugins(Some(tmp.path()));
        let hello = listed
            .iter()
            .find(|p| p.tool_name == "plugin_whycodes_test_hello");
        assert!(hello.is_some(), "{listed:?}");
        assert_eq!(hello.unwrap().command, "echo json");
    }

    #[tokio::test]
    async fn plugin_shell_tool_execute_and_list_skips() {
        let cfg = PluginConfig {
            name: "echo".into(),
            command: "echo $PLUGIN_ARG_INPUT $PLUGIN_WORKSPACE".into(),
            description: "d".into(),
            parameters: None,
            working_dir: None,
        };
        let tool = PluginShellTool::from_config(cfg);
        assert_eq!(tool.name(), "plugin_echo");
        assert_eq!(tool.description(), "d");
        let params = tool.parameters();
        assert_eq!(params["properties"]["input"]["type"], "string");
        let out = tool
            .execute(
                serde_json::json!({"input": "hi", "n": 1}),
                &crate::tool::ToolContext::new("/tmp"),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("hi"), "{}", out.content);

        let schema = serde_json::json!({"type": "object"});
        let cfg = PluginConfig {
            name: "typed".into(),
            command: "true".into(),
            description: "t".into(),
            parameters: Some(schema.clone()),
            working_dir: Some("/tmp".into()),
        };
        let tool = PluginShellTool::from_config(cfg);
        assert_eq!(tool.parameters(), schema);

        let tmp = tempfile::tempdir().unwrap();
        let why = tmp.path().join(".whycodes");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"
[[plugins]]
name = ""
command = "echo"
[[plugins]]
name = "ok"
command = ""
"#,
        )
        .unwrap();
        let listed = list_shell_plugins(Some(tmp.path()));
        assert!(listed.iter().all(|p| p.tool_name != "plugin_"));

        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
        let _ = list_shell_plugins(None);
        unsafe {
            match prev {
                Some(v) => std::env::set_var("WHYCODES_HOME", v),
                None => std::env::remove_var("WHYCODES_HOME"),
            }
        }
    }

    #[tokio::test]
    async fn execute_non_object_args_and_empty_json_name() {
        let cfg = PluginConfig {
            name: "echo".into(),
            command: "echo ok".into(),
            description: "d".into(),
            parameters: None,
            working_dir: None,
        };
        let tool = PluginShellTool::from_config(cfg);
        let out = tool
            .execute(
                serde_json::json!("not-an-object"),
                &crate::tool::ToolContext::new("/tmp"),
            )
            .await;
        assert!(!out.is_error || !out.content.is_empty(), "{}", out.content);

        let tmp = tempfile::tempdir().unwrap();
        let why = tmp.path().join(".whycodes");
        std::fs::create_dir_all(why.join("plugins").join("blank")).unwrap();
        std::fs::write(
            why.join("plugins").join("blank").join("plugin.json"),
            r#"{"name":"","command":"echo"}"#,
        )
        .unwrap();
        let listed = list_shell_plugins(Some(tmp.path()));
        assert!(listed.iter().all(|p| p.tool_name != "plugin_"));
    }
}
