use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;
use whycode_skill::registry::SkillRegistry;

pub struct SkillTool;

impl Default for SkillTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load and manage skills"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "load"],
                    "description": "Action: 'list' shows all available skills, 'load' loads a specific skill by name"
                },
                "name": {
                    "type": "string",
                    "description": "Name of the skill to load (required when action is 'load')"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");

        match action {
            "list" => {
                let registry = match SkillRegistry::load() {
                    Ok(r) => r,
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error loading skills: {e}"),
                            is_error: true,
                        };
                    }
                };

                if registry.skills.is_empty() {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: "No skills found.".to_string(),
                        is_error: false,
                    };
                }

                let mut lines = Vec::new();
                lines.push(format!("Available skills ({}):", registry.skills.len()));
                for skill in &registry.skills {
                    lines.push(format!(
                        "  - {}: {}",
                        skill.name,
                        if skill.description.is_empty() {
                            "(no description)"
                        } else {
                            &skill.description
                        }
                    ));
                }
                ToolResult {
                    tool_call_id: String::new(),
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
            "load" => {
                let name = args["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: "Error: 'name' is required when action is 'load'".to_string(),
                        is_error: true,
                    };
                }

                let registry = match SkillRegistry::load() {
                    Ok(r) => r,
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error loading skills: {e}"),
                            is_error: true,
                        };
                    }
                };

                match registry.get(name) {
                    Some(skill) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!(
                            "Loaded skill '{}':\n\n{}\n\n{}",
                            skill.name, skill.description, skill.prompt
                        ),
                        is_error: false,
                    },
                    None => ToolResult {
                        tool_call_id: String::new(),
                        content: format!(
                            "Skill '{}' not found. Use action='list' to see available skills.",
                            name
                        ),
                        is_error: true,
                    },
                }
            }
            _ => ToolResult {
                tool_call_id: String::new(),
                content: format!("Unknown action '{}'. Valid actions: list, load", action),
                is_error: true,
            },
        }
    }
}
