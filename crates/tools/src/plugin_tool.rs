//! Shell plugins from `plugins.toml` / PluginRegistry as agent tools.

use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;
use whycode_skill::plugin::{Plugin, PluginConfig, PluginContext};

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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
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
    }
}
