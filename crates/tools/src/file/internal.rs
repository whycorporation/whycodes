//! Internal `://` paths (`skill://`, `agent://`) resolved by FS-shaped tools.

use crate::tool::ToolContext;
use whycodes_core::types::ToolResult;
use whycodes_skill::SkillRegistry;

fn ok(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: false,
    }
}

fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: true,
    }
}

pub fn read_internal(path: &str, ctx: &ToolContext) -> Option<ToolResult> {
    if let Some(rest) = path.strip_prefix("skill://") {
        return Some(read_skill(rest, ctx));
    }
    if let Some(rest) = path.strip_prefix("agent://") {
        return Some(read_agent(rest, ctx));
    }
    None
}

fn read_skill(name: &str, ctx: &ToolContext) -> ToolResult {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return err("skill:// requires a skill name (e.g. skill://demo).");
    }
    let project = std::path::Path::new(&ctx.working_dir);
    let registry = match SkillRegistry::load_for_project(project) {
        Ok(r) => r,
        Err(e) => return err(format!("Error loading skills: {e}")),
    };
    match registry.get_ignore_ascii_case(name) {
        Some(skill) => ok(format!(
            "# skill://{}\n{}\n\n{}",
            skill.name, skill.description, skill.prompt
        )),
        None => err(format!(
            "Skill '{name}' not found. Use the `skill` tool (action=list) or `read skill://`."
        )),
    }
}

fn read_agent(id: &str, ctx: &ToolContext) -> ToolResult {
    let dir = whycodes_core::project_dir(std::path::Path::new(&ctx.working_dir)).join("agents");
    let id = id.trim().trim_start_matches('/');
    if id.is_empty() {
        return list_agent_artifacts(&dir);
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return err("agent:// id may only contain letters, digits, '-' and '_'.");
    }
    let path = dir.join(format!("{id}.md"));
    match std::fs::read_to_string(&path) {
        Ok(body) if !body.trim().is_empty() => ok(format!("# agent://{id}\n\n{body}")),
        Ok(_) => err(format!("agent://{id} is empty.")),
        Err(_) => err(format!(
            "No agent artifact '{id}'. Finished task/swarm workers write `.whycodes/agents/<id>.md`."
        )),
    }
}

fn list_agent_artifacts(dir: &std::path::Path) -> ToolResult {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ok(
            "No agent artifacts yet. Completed `task` / `swarm` workers write `agent://<id>`.",
        );
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                p.file_stem().map(|s| s.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return ok("No agent artifacts yet.");
    }
    let mut out = String::from("Agent artifacts (read with `read agent://<id>`):\n");
    for n in names {
        out.push_str("- ");
        out.push_str(&n);
        out.push('\n');
    }
    ok(out)
}

#[cfg(test)]
#[path = "internal_tests.rs"]
mod tests;
