//! Internal `://` paths (`skill://`, `agent://`) resolved by FS-shaped tools.

use crate::tool::ToolContext;
use whycode_core::types::ToolResult;
use whycode_skill::SkillRegistry;

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
    let dir = std::path::Path::new(&ctx.working_dir)
        .join(".whycode")
        .join("agents");
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
            "No agent artifact '{id}'. Finished task/swarm workers write `.whycode/agents/<id>.md`."
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
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    #[test]
    fn unknown_scheme_is_none() {
        let ctx = ToolContext::new("/tmp");
        assert!(read_internal("src/main.rs", &ctx).is_none());
        assert!(read_internal("http://example.com", &ctx).is_none());
    }

    #[test]
    fn skill_url_rejects_bad_names() {
        let ctx = ToolContext::new("/tmp");
        let r = read_internal("skill://", &ctx).unwrap();
        assert!(r.is_error);
        let r = read_internal("skill://a/b", &ctx).unwrap();
        assert!(r.is_error);
    }

    #[test]
    fn skill_url_loads_project_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(".skills");
        std::fs::create_dir(&skills).unwrap();
        std::fs::write(
            skills.join("demo.skill.md"),
            "---\nname: demo\ndescription: d\n---\n\nTHE BODY\n",
        )
        .unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy());
        let r = read_internal("skill://demo", &ctx).unwrap();
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("THE BODY"));
        let miss = read_internal("skill://nope", &ctx).unwrap();
        assert!(miss.is_error);
    }

    #[test]
    fn agent_url_lists_and_reads() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join(".whycode").join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("task-1.md"), "findings here").unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy());
        let list = read_internal("agent://", &ctx).unwrap();
        assert!(!list.is_error);
        assert!(list.content.contains("task-1"));
        let body = read_internal("agent://task-1", &ctx).unwrap();
        assert!(body.content.contains("findings here"));
        let bad = read_internal("agent://../x", &ctx).unwrap();
        assert!(bad.is_error);
        let missing = read_internal("agent://nope", &ctx).unwrap();
        assert!(missing.is_error);
    }
}
