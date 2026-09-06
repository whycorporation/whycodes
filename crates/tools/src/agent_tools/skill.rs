use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;
use whycodes_skill::registry::SkillRegistry;

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
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "List or load project/user skills. Prefer `read skill://<name>` for the body; \
         `action=list` shows names and descriptions only."
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let action = args["action"].as_str().unwrap_or("");
            let project = std::path::Path::new(&ctx.working_dir);

            match action {
                "list" => {
                    let registry = match SkillRegistry::load_for_project(project) {
                        Ok(r) => r,
                        Err(e) => {
                            return ToolResult {
                                tool_call_id: String::new(),
                                content: format!("Error loading skills: {e}"),
                                is_error: true,
                            };
                        }
                    };

                    ToolResult {
                        tool_call_id: String::new(),
                        content: format_skill_list(
                            registry
                                .skills
                                .iter()
                                .map(|s| (s.name.as_str(), s.description.as_str()))
                                .collect(),
                        ),
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

                    let registry = match SkillRegistry::load_for_project(project) {
                        Ok(r) => r,
                        Err(e) => {
                            return ToolResult {
                                tool_call_id: String::new(),
                                content: format!("Error loading skills: {e}"),
                                is_error: true,
                            };
                        }
                    };

                    match registry.get_ignore_ascii_case(name) {
                        Some(skill) => ToolResult {
                            tool_call_id: String::new(),
                            content: format_loaded_skill(
                                &skill.name,
                                &skill.description,
                                &skill.prompt,
                            ),
                            is_error: false,
                        },
                        None => ToolResult {
                            tool_call_id: String::new(),
                            content: skill_not_found(name),
                            is_error: true,
                        },
                    }
                }
                _ => ToolResult {
                    tool_call_id: String::new(),
                    content: unknown_skill_action(action),
                    is_error: true,
                },
            }
        })
    }
}

fn skill_description_label(description: &str) -> &str {
    if description.is_empty() {
        "(no description)"
    } else {
        description
    }
}

fn format_skill_list(skills: Vec<(&str, &str)>) -> String {
    if skills.is_empty() {
        return "No skills found.".to_string();
    }
    let mut lines = vec![format!("Available skills ({}):", skills.len())];
    for (name, description) in skills {
        lines.push(format!(
            "  - {name}: {}",
            skill_description_label(description)
        ));
    }
    lines.join("\n")
}

fn format_loaded_skill(name: &str, description: &str, prompt: &str) -> String {
    format!("Loaded skill '{name}':\n\n{description}\n\n{prompt}")
}

fn skill_not_found(name: &str) -> String {
    format!("Skill '{name}' not found. Use action='list' to see available skills.")
}

fn unknown_skill_action(action: &str) -> String {
    format!("Unknown action '{action}'. Valid actions: list, load")
}

#[cfg(test)]
#[path = "skill_tests.rs"]
mod tests;
