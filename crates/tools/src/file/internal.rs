//! Internal `://` paths (`skill://`, `agent://`, `proc://`, `rule://`,
//! `pr://`, `issue://`) resolved by FS-shaped tools. `mcp://` is served by
//! the agent loop, which owns the MCP clients.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::tool::ToolContext;
use whycodes_core::types::ToolResult;
use whycodes_skill::SkillRegistry;

/// Longest tail of a background job log returned by `proc://<id>`.
const PROC_TAIL_CHARS: usize = 16_000;

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
            Some(skill) => {
                let note = skill.location_note();
                let note = if note.is_empty() {
                    note
                } else {
                    format!("\n\n{note}")
                };
                ok(&format!(
                    "# skill://{}\n{}{note}\n\n{}",
                    skill.name, skill.description, skill.prompt
                ))
            }
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

fn skill_registry_error(e: impl std::fmt::Display) -> ToolResult {
    skill_load_error(&e.to_string())
}

fn skill_load_error(e: &str) -> ToolResult {
    err(&format!("Error loading skills: {e}"))
}

fn load_skill_registry(project: &std::path::Path) -> Result<SkillRegistry, ToolResult> {
    SkillRegistry::load_for_project(project).map_err(skill_registry_error)
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

/// Resolve the schemes that need no network: `skill://`, `agent://`,
/// `proc://`, `rule://`. `None` for a plain file path.
pub fn read_internal(path: &str, ctx: &ToolContext) -> Option<ToolResult> {
    if let Some(rest) = path.strip_prefix("skill://") {
        return Some(read_skill(rest, ctx));
    }
    if let Some(rest) = path.strip_prefix("agent://") {
        return Some(read_agent(rest, ctx));
    }
    if let Some(rest) = path.strip_prefix("proc://") {
        return Some(read_proc(rest, ctx));
    }
    if let Some(rest) = path.strip_prefix("rule://") {
        return Some(read_rule(rest, ctx));
    }
    if path.starts_with("mcp://") {
        return Some(err(
            "mcp:// is served by the agent loop (it owns the MCP clients). Outside it, use \
             `mcp_list_resources` / `mcp_read_resource`.",
        ));
    }
    None
}

/// [`read_internal`] plus the GitHub schemes (`pr://`, `issue://`), which
/// need the network and so an async context.
pub async fn read_internal_async(path: &str, ctx: &ToolContext) -> Option<ToolResult> {
    use super::internal_github::{Kind, read_github};
    if let Some(rest) = path.strip_prefix("pr://") {
        return Some(read_github(Kind::Pr, rest, ctx).await);
    }
    if let Some(rest) = path.strip_prefix("issue://") {
        return Some(read_github(Kind::Issue, rest, ctx).await);
    }
    read_internal(path, ctx)
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
     - read — files, skill://, agent://, proc://, rule://, pr://, issue://, mcp://\n\
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
             When a background job ends, its exit and last output arrive as a message\n\
             before your next step (unless `wait` or `bg read` already showed it).\n\
             Long-running non-catastrophic commands auto-detach after 20s unless\n\
             `[tools.bash] auto_background = false`; a `timeout` shorter than that still\n\
             stops the command. Catastrophic commands (rm -rf /,\n\
             git clean of home, …) stay in the foreground and still hit the permission gate.\n\
             Child processes do not inherit provider API keys. Detached output lives under\n\
             `.whycodes/scratch`, not the OS temp dir."
        }
        "read" => {
            "Read a text file with line numbers. Prefer project-relative paths.\n\n\
             Outline: a source file over 400 lines read without `offset`/`limit` returns its\n\
             declaration lines (fn/struct/class/impl/headings, indentation kept) with line\n\
             numbers and content tags, then `[file has N lines; showing its outline. ...]`.\n\
             The tags work with `edit`. Read a range next with `offset`/`limit`.\n\
             `outline: false` always pages; `outline: true` outlines a file of any length.\n\
             Extensions repomap does not map, files over 512 KB, or files with no\n\
             declarations page as usual.\n\n\
             Internal URLs (a bare scheme lists what it can read):\n\
             - `skill://<name>` — a skill body.\n\
             - `agent://<id>` — a finished task/swarm artifact; `agent://<id>/<json-pointer>` one\n\
             value of its structured result (e.g. `agent://task-1/findings/0/path`);\n\
             `agent://tools/<name>` this guide set.\n\
             - `proc://<job-id>` — the stored log of a background shell job\n\
             (`.whycodes/scratch/<id>.log`); `bg` lists live jobs.\n\
             - `rule://<name>` — a project rule file (`.whycodes/rules/*.md`, `.cursor/rules/*.mdc`,\n\
             `.clinerules/*.md`, `.github/instructions/*.md`).\n\
             - `pr://<number>` / `pr://<number>/diff` — a pull request of the `origin` GitHub\n\
             remote (metadata and body, or the diff); `pr://` lists the open ones (first page).\n\
             - `issue://<number>` — an issue of the same remote; `issue://` lists the open ones.\n\
             GitHub reads use the same token discovery and network allowlist as `github_pr`.\n\
             - `mcp://<server>/<uri>` — an MCP resource; `mcp://<server>` lists that server's\n\
             resources, `mcp://` the connected servers.\n\n\
             A `.ipynb` reads as its cells: `# %% [code] cell:N` before each source,\n\
             outputs summarized in one `# out:` line. `edit` / `write` take that text."
        }
        "edit" => {
            "Make targeted edits. Prefer `from`/`to`/`insert_after` tags from `read`/`grep`.\n\
             `old_string` is the fallback: exact match, then unique whitespace-tolerant match.\n\
             A `.ipynb` is edited as `read` shows it. A changed code cell loses its outputs;\n\
             a `# %% [code] cell:new` marker adds a cell, removing a block deletes one."
        }
        "write" => {
            "Write content to a file, creating it if it doesn't exist.\n\
             For a `.ipynb`, cell-marker text (as `read` shows it) becomes notebook JSON;\n\
             raw JSON is written as given."
        }
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
    let name = name.trim().trim_matches('/');
    if name.is_empty() || name.contains("..") {
        return err("skill:// requires a skill name (e.g. skill://demo).");
    }
    if let Some((skill, topic)) = name.split_once('/') {
        return match whycodes_skill::bundled::render_topic(skill, topic) {
            Ok(body) => ok(&body),
            Err(e) => err(&e),
        };
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
    if let Some((id, pointer)) = id.split_once('/') {
        return read_agent_value(&dir, id, pointer);
    }
    if !valid_id(id) {
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

fn valid_id(id: &str) -> bool {
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn read_proc(id: &str, ctx: &ToolContext) -> ToolResult {
    let dir = whycodes_core::project_scratch_dir(Path::new(&ctx.working_dir));
    let id = id.trim().trim_start_matches('/');
    if id.is_empty() {
        return list_proc_logs(&dir);
    }
    if !valid_id(id) {
        return err("proc:// job id may only contain letters, digits, '-' and '_'.");
    }
    match std::fs::read(dir.join(format!("{id}.log"))) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            ok(&format!(
                "# proc://{id}\n\n{}",
                tail_chars(&text, PROC_TAIL_CHARS)
            ))
        }
        Err(e) => err(&format!(
            "No stored log for background job '{id}' ({e}). `bg` (action=list) shows the jobs \
             of this session and `bg` (action=read, id=…) their output; `read proc://` lists \
             the logs under .whycodes/scratch."
        )),
    }
}

fn tail_chars(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let skip = total - max;
    let tail: String = text.chars().skip(skip).collect();
    format!("…({skip} earlier chars omitted)\n{tail}")
}

fn list_proc_logs(dir: &Path) -> ToolResult {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ok("No background job logs yet. `bash` with background=true writes one per job.");
    };
    let mut rows: Vec<(String, u64, Option<SystemTime>)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("log") {
                return None;
            }
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            let meta = e.metadata().ok()?;
            Some((stem, meta.len(), meta.modified().ok()))
        })
        .collect();
    rows.sort();
    if rows.is_empty() {
        return ok("No background job logs yet.");
    }
    let mut out =
        String::from("Background job logs (read with `read proc://<id>`; live jobs: `bg`):\n");
    for (id, len, modified) in rows {
        out.push_str(&format!(
            "- {id}  {}  modified {}\n",
            size_label(len),
            age_label(modified)
        ));
    }
    ok(&out)
}

fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn age_label(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return "at an unknown time".to_string();
    };
    let secs = SystemTime::now()
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// `(stem, path)` of every project rule file, in precedence order.
fn rule_files(project: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for (dir, ext) in whycodes_core::project_rule_dirs(project) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            })
            .collect();
        found.sort();
        for path in found {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push((stem, path));
        }
    }
    out
}

fn rule_label(path: &Path, project: &Path) -> String {
    path.strip_prefix(project)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

const RULE_DIRS_HINT: &str =
    ".whycodes/rules/*.md, .cursor/rules/*.mdc, .clinerules/*.md, .github/instructions/*.md";

fn read_rule(name: &str, ctx: &ToolContext) -> ToolResult {
    let project = Path::new(&ctx.working_dir);
    let name = name.trim().trim_start_matches('/');
    let rules = rule_files(project);
    if name.is_empty() {
        return list_rules(project, &rules);
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return err("rule:// takes a rule name (e.g. rule://style); `read rule://` lists them.");
    }
    match rules
        .iter()
        .find(|(stem, _)| stem.eq_ignore_ascii_case(name))
    {
        Some((stem, path)) => rule_body(stem, path, project),
        None => err(&format!(
            "No project rule '{name}'. `read rule://` lists them ({RULE_DIRS_HINT})."
        )),
    }
}

fn rule_body(stem: &str, path: &Path, project: &Path) -> ToolResult {
    let label = rule_label(path, project);
    match std::fs::read(path) {
        Ok(bytes) => ok(&format!(
            "# rule://{stem} ({label})\n\n{}",
            String::from_utf8_lossy(&bytes)
        )),
        Err(e) => err(&format!("Cannot read rule '{stem}' at {label}: {e}")),
    }
}

fn list_rules(project: &Path, rules: &[(String, PathBuf)]) -> ToolResult {
    if rules.is_empty() {
        return ok(&format!(
            "No project rules. Put Markdown files under .whycodes/rules/ (also read: {RULE_DIRS_HINT})."
        ));
    }
    let mut out = String::from("Project rules (read with `read rule://<name>`):\n");
    for (stem, path) in rules {
        out.push_str(&format!("- {stem} — {}\n", rule_label(path, project)));
    }
    ok(&out)
}

/// `agent://<id>/<json-pointer>`: one value of a worker's structured result
/// (`<id>.json`), pretty-printed. An empty pointer gives the whole result.
fn read_agent_value(dir: &std::path::Path, id: &str, pointer: &str) -> ToolResult {
    if !valid_id(id) {
        return err("agent:// id may only contain letters, digits, '-' and '_'.");
    }
    let Ok(raw) = std::fs::read_to_string(dir.join(format!("{id}.json"))) else {
        return err(&format!(
            "agent://{id} has no structured result; only a task with an output schema writes `.whycodes/agents/{id}.json`."
        ));
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("agent://{id} structured result is not JSON: {e}")),
    };
    let pointer = pointer.trim_end_matches('/');
    let found = if pointer.is_empty() {
        Some(&value)
    } else {
        value.pointer(&format!("/{pointer}"))
    };
    match found {
        Some(v) => ok(&format!("{v:#}")),
        None => err(&format!("No value at `/{pointer}` in agent://{id}.")),
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
