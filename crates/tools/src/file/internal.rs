//! Internal `://` paths (`skill://`, `agent://`) resolved by FS-shaped tools.

use crate::tool::ToolContext;
use whycodes_core::types::ToolResult;
use whycodes_skill::SkillRegistry;

fn take_skill_registry(
    result: Result<SkillRegistry, ToolResult>,
) -> Result<SkillRegistry, ToolResult> {
    match result {
        Ok(r) => Ok(r),
        Err(e) => Err(registry_load_failed(e)),
    }
}

fn skill_registry_or_err(
    result: Result<SkillRegistry, ToolResult>,
) -> Result<SkillRegistry, ToolResult> {
    result
}

fn skill_body(result: Result<SkillRegistry, ToolResult>, name: &str) -> ToolResult {
    match skill_registry_or_err(result) {
        Ok(registry) => match registry.get_ignore_ascii_case(name) {
            Some(skill) => ok(&format!(
                "# skill://{}\n{}\n\n{}",
                skill.name, skill.description, skill.prompt
            )),
            None => err(&format!(
                "Skill '{name}' not found. Use the `skill` tool (action=list) or `read skill://`."
            )),
        },
        Err(e) => e,
    }
}

fn registry_load_failed(e: ToolResult) -> ToolResult {
    e
}

fn skill_load_error(e: &str) -> ToolResult {
    err(&format!("Error loading skills: {e}"))
}

fn load_skill_registry(project: &std::path::Path) -> Result<SkillRegistry, ToolResult> {
    SkillRegistry::load_for_project(project).map_err(|e| skill_load_error(&e.to_string()))
}

fn ok(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: false,
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
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

fn read_tool_guide(name: &str) -> ToolResult {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() {
        return ok(&tool_guide_index());
    }
    match tool_guide(name) {
        Some(body) => ok(&format!("# agent://tools/{name}\n\n{body}")),
        None => err(&format!(
            "No long guide for tool `{name}`. Try `read agent://tools` for the index."
        )),
    }
}

fn tool_guide_index() -> String {
    "On-demand tool guides (`read agent://tools/<name>`):\n\
     - bash — shell command, auto-background, bg jobs\n\
     - read — files, skill://, agent://\n\
     - edit — from/to tags and old_string fallback\n\
     - write — create/overwrite a file\n\
     - grep — in-process regex search\n\
     - glob — find files by pattern\n"
        .into()
}

fn tool_guide(name: &str) -> Option<&'static str> {
    Some(match name {
        "bash" | "shell" => {
            "Execute a shell command in the project environment.\n\n\
             Default: wait for stdout/stderr (timeout 120s).\n\
             `background=true` returns a job id immediately; use `bg` to list/read/kill.\n\
             Long-running non-catastrophic commands auto-detach after 20s unless\n\
             `[tools.bash] auto_background = false`. Catastrophic commands (rm -rf /,\n\
             git clean of home, …) stay in the foreground and still hit the permission gate.\n\
             Child processes do not inherit provider API keys. Detached output lives under\n\
             `.whycodes/scratch`, not the OS temp dir."
        }
        "read" => {
            "Read a text file with line numbers. Prefer project-relative paths.\n\
             `skill://<name>` loads a skill; `agent://<id>` re-reads a finished task/swarm\n\
             artifact; `agent://tools/<name>` opens the long tool guide."
        }
        "edit" => {
            "Make targeted edits. Prefer `from`/`to`/`insert_after` tags from `read`/`grep`.\n\
             `old_string` is the fallback: exact match, then unique whitespace-tolerant match."
        }
        "write" => "Write content to a file, creating it if it doesn't exist.",
        "grep" => {
            "Search file contents with regex (in-process). Respects .gitignore;\n\
             skips binaries and heavy dirs. Hits are `path:line tag:text`."
        }
        "glob" => {
            "Find files by glob pattern under the project (e.g. `**/*.rs`).\n\
             Respects .gitignore; skips heavy dirs."
        }
        _ => return None,
    })
}

fn read_skill(name: &str, ctx: &ToolContext) -> ToolResult {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return err("skill:// requires a skill name (e.g. skill://demo).");
    }
    let project = std::path::Path::new(&ctx.working_dir);
    skill_body(take_skill_registry(load_skill_registry(project)), name)
}

fn read_agent(id: &str, ctx: &ToolContext) -> ToolResult {
    let dir = whycodes_core::project_dir(std::path::Path::new(&ctx.working_dir)).join("agents");
    let id = id.trim().trim_start_matches('/');
    if let Some(rest) = id.strip_prefix("tools/") {
        return read_tool_guide(rest);
    }
    if id == "tools" {
        return read_tool_guide("");
    }
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
        Ok(body) if !body.trim().is_empty() => ok(&format!("# agent://{id}\n\n{body}")),
        Ok(_) => err(&format!("agent://{id} is empty.")),
        Err(_e) => err(&format!(
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
    ok(&out)
}

#[cfg(test)]
#[path = "internal_tests.rs"]
mod tests;
