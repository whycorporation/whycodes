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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    #[tokio::test]
    async fn list_and_load_use_project_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(".skills");
        std::fs::create_dir(&skills).unwrap();
        std::fs::write(
            skills.join("demo.skill.md"),
            "---\nname: Demo\ndescription: d\n---\n\nTHE BODY\n",
        )
        .unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy());
        let tool = SkillTool::new();
        let listed = tool.execute(json!({"action": "list"}), &ctx).await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains("Demo"), "{}", listed.content);
        let loaded = tool
            .execute(json!({"action": "load", "name": "demo"}), &ctx)
            .await;
        assert!(!loaded.is_error, "{}", loaded.content);
        assert!(loaded.content.contains("THE BODY"), "{}", loaded.content);
        let missing = tool
            .execute(json!({"action": "load", "name": "nope"}), &ctx)
            .await;
        assert!(missing.is_error);

        let empty_dir = tempfile::tempdir().unwrap();
        let empty_ctx = ToolContext::new(empty_dir.path().to_string_lossy());
        let none = tool.execute(json!({"action": "list"}), &empty_ctx).await;
        assert!(!none.is_error, "{}", none.content);
        assert!(none.content.contains("No skills found"), "{}", none.content);

        let bad = tool.execute(json!({"action": "nope"}), &ctx).await;
        assert!(bad.is_error, "{}", bad.content);
        assert!(bad.content.contains("Unknown action"), "{}", bad.content);
    }
}
