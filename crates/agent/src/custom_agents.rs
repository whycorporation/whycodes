//! Markdown custom subagents for `task` / `swarm`.
//!
//! A definition is a Markdown file with frontmatter, the shape the common
//! `.claude/agents/` convention uses:
//!
//! ```text
//! ---
//! name: reviewer
//! description: Reviews a diff for bugs
//! tools: Read, Grep, Glob
//! model: inherit
//! output:
//!   type: object
//!   properties:
//!     findings:
//!       type: array
//!       items: { "type": "string" }
//! ---
//! You are a careful code reviewer…
//! ```
//!
//! Search order, later wins on a name clash: `~/.whycodes/agents`,
//! `~/.claude/agents`, `<project>/.claude/agents`, `<project>/.whycodes/agents`.
//! `.whycodes/agents/` also holds finished workers' output (`agent://<id>`);
//! those files have no frontmatter `description` and are skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use whycodes_core::types::PermissionSet;

/// Subagent types that need no definition file.
pub const BUILTIN_TYPES: [&str; 3] = ["general", "explore", "scout"];

/// Model tier aliases found in shared definition files. They are not model
/// ids, so they fall back to the normal worker model.
const MODEL_ALIASES: [&str; 3] = ["sonnet", "opus", "haiku"];

#[derive(Debug, Clone, PartialEq)]
pub struct CustomAgent {
    pub name: String,
    pub description: String,
    /// System prompt (the Markdown body).
    pub prompt: String,
    /// Allowed tools; `None` keeps the parent's tool set.
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    /// `inherit`, `provider/model`, or a bare model id.
    pub model: Option<String>,
    /// Prepared result schema from the `output` key (see
    /// [`crate::structured_output`]); `None` when absent or invalid.
    pub output: Option<serde_json::Value>,
    pub path: PathBuf,
}

impl CustomAgent {
    /// The parent's permissions narrowed by this definition. A definition can
    /// only take tools away: `tools` intersects with the parent's allow list,
    /// and `deny` (todo / nesting tools) is always added.
    pub fn permission(&self, parent: &PermissionSet, deny: &[&str]) -> PermissionSet {
        let mut perm = parent.clone();
        if let Some(tools) = &self.tools {
            let allowed: Vec<String> = match &parent.allowed_tools {
                Some(p) => tools.iter().filter(|t| p.contains(t)).cloned().collect(),
                None => tools.clone(),
            };
            perm.allowed_tools = Some(allowed);
        }
        let mut denied = perm.denied_tools.unwrap_or_default();
        for t in self
            .disallowed_tools
            .iter()
            .map(String::as_str)
            .chain(deny.iter().copied())
        {
            if !denied.iter().any(|d| d == t) {
                denied.push(t.to_string());
            }
        }
        perm.denied_tools = Some(denied);
        perm
    }

    /// Provider and model for this agent. `worker` is what a built-in worker
    /// would use; `inherit` keeps the session's own route.
    pub fn route(&self, provider: &str, model: &str, worker: (String, String)) -> (String, String) {
        let raw = self.model.as_deref().map(str::trim).unwrap_or("");
        if raw.is_empty() || MODEL_ALIASES.contains(&raw) {
            return worker;
        }
        if raw == "inherit" {
            return (provider.to_string(), model.to_string());
        }
        crate::routing::resolve_override(provider, model, Some(raw))
    }
}

/// Definition directories, lowest precedence first.
pub fn dirs(project: &Path) -> Vec<PathBuf> {
    let mut out = vec![whycodes_core::paths::config_dir().join("agents")];
    out.extend(whycodes_core::paths::user_home().map(|h| h.join(".claude").join("agents")));
    out.push(project.join(".claude").join("agents"));
    out.push(whycodes_core::project_dir(project).join("agents"));
    out
}

/// All definitions visible from `project`, sorted by name.
pub fn load(project: &Path) -> Vec<CustomAgent> {
    load_from(&dirs(project))
}

/// Definitions from `dirs`; a later directory replaces an earlier name.
pub fn load_from(dirs: &[PathBuf]) -> Vec<CustomAgent> {
    let mut by_name: HashMap<String, CustomAgent> = HashMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md") && p.is_file())
            .collect();
        paths.sort();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                tracing::debug!(path = %path.display(), "custom agent unreadable");
                continue;
            };
            match parse(&text, &path) {
                Some(agent) => {
                    by_name.insert(agent.name.clone(), agent);
                }
                None => tracing::debug!(path = %path.display(), "not a custom agent definition"),
            }
        }
    }
    let mut out: Vec<CustomAgent> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Resolve a `subagent_type`. Built-ins give `Ok(None)`; an unknown name is
/// an error that lists what exists.
pub fn resolve(kind: &str, agents: &[CustomAgent]) -> Result<Option<CustomAgent>, String> {
    if BUILTIN_TYPES.contains(&kind) {
        return Ok(None);
    }
    match agents.iter().find(|a| a.name == kind) {
        Some(a) => Ok(Some(a.clone())),
        None => {
            let names: Vec<&str> = BUILTIN_TYPES
                .iter()
                .copied()
                .chain(agents.iter().map(|a| a.name.as_str()))
                .collect();
            Err(format!(
                "Unknown subagent_type `{kind}`. Available: {}.",
                names.join(", ")
            ))
        }
    }
}

/// System-prompt section naming the custom agents; empty when there are none.
pub fn catalog_markdown(agents: &[CustomAgent]) -> String {
    if agents.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# Custom agents\n\nBesides general / explore / scout, `task` and `swarm` accept these `subagent_type` values:\n",
    );
    for a in agents {
        let desc: String = a
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let desc: String = desc.chars().take(200).collect();
        out.push_str(&format!("- `{}`: {desc}\n", a.name));
    }
    out
}

/// Parse one definition. `None` when the file has no frontmatter, no
/// `description`, an invalid name, or a built-in name.
pub fn parse(text: &str, path: &Path) -> Option<CustomAgent> {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n")?;
    let i = rest.find("\n---")?;
    let (front, body) = (&rest[..i], &rest[i + 4..]);
    let body = body.split_once('\n').map(|(_, b)| b).unwrap_or("").trim();
    let fields = frontmatter(front);

    let description = fields
        .get("description")
        .map(Field::text)
        .unwrap_or_default();
    if description.trim().is_empty() {
        return None;
    }
    let name = match fields.get("name").map(Field::text) {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => path.file_stem()?.to_string_lossy().into_owned(),
    };
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid || BUILTIN_TYPES.contains(&name.as_str()) {
        tracing::warn!(path = %path.display(), name, "custom agent name rejected");
        return None;
    }
    let tools = fields
        .get("tools")
        .map(tool_names)
        .filter(|t| !t.is_empty());
    let disallowed_tools = ["disallowedTools", "disallowed_tools"]
        .iter()
        .find_map(|k| fields.get(*k))
        .map(tool_names)
        .unwrap_or_default();
    let model = fields
        .get("model")
        .map(Field::text)
        .filter(|m| !m.is_empty());
    let output = output_schema(front).and_then(|raw| {
        match yaml_value(&raw).and_then(|v| crate::structured_output::prepare(&v)) {
            Ok(schema) => Some(schema),
            Err(e) => {
                let file = path.display();
                tracing::warn!("custom agent {file}: `output` schema ignored: {e}");
                None
            }
        }
    });
    let prompt = if body.is_empty() {
        crate::agent::Agent::system_prompt_for("general")
    } else {
        body.to_string()
    };
    Some(CustomAgent {
        name,
        description: description.trim().to_string(),
        prompt,
        tools,
        disallowed_tools,
        model,
        output,
        path: path.to_path_buf(),
    })
}

/// The raw `output:` entry: its inline value plus the indented lines under it.
fn output_schema(front: &str) -> Option<String> {
    let mut lines = front.lines();
    let first = lines.find(|l| l.starts_with("output:"))?;
    let mut raw = first["output:".len()..].trim().to_string();
    for line in lines.take_while(|l| l.trim().is_empty() || l.starts_with([' ', '\t'])) {
        raw.push('\n');
        raw.push_str(line);
    }
    Some(raw)
}

/// Parse the YAML subset a schema needs: nested `key: value` maps, `- item`
/// lists (of scalars or maps), inline JSON or `[a, b]` flow values, and plain,
/// quoted, numeric, boolean or null scalars. Block scalars and anchors are
/// not supported.
fn yaml_value(raw: &str) -> Result<serde_json::Value, String> {
    if raw.trim_start().starts_with(['{', '[']) {
        return yaml_scalar(raw.trim());
    }
    let mut lines: Vec<(usize, String)> = raw
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| (l.len() - l.trim_start().len(), l.trim().to_string()))
        .collect();
    let Some(&(indent, _)) = lines.first() else {
        return Err("`output` is empty".into());
    };
    let mut i = 0;
    let value = yaml_node(&mut lines, &mut i, indent)?;
    match lines.get(i) {
        Some((_, line)) => Err(format!("unexpected indentation at `{line}`")),
        None => Ok(value),
    }
}

fn yaml_node(
    lines: &mut [(usize, String)],
    i: &mut usize,
    indent: usize,
) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    if lines[*i].1 == "-" || lines[*i].1.starts_with("- ") {
        let mut items = Vec::new();
        while let Some((at, line)) = lines.get(*i).cloned() {
            if at != indent || !(line == "-" || line.starts_with("- ")) {
                break;
            }
            let rest = line[1..].trim_start().to_string();
            if rest.is_empty() {
                *i += 1;
                items.push(yaml_child(lines, i, indent, false)?);
            } else if yaml_key(&rest).is_some() && !rest.starts_with(['{', '[']) {
                // `- key: value` opens a map whose keys line up with `key`.
                let inner = at + (line.len() - rest.len());
                lines[*i] = (inner, rest);
                items.push(yaml_node(lines, i, inner)?);
            } else {
                *i += 1;
                items.push(yaml_scalar(&rest)?);
            }
        }
        return Ok(Value::Array(items));
    }
    let mut map = serde_json::Map::new();
    while let Some((at, line)) = lines.get(*i).cloned() {
        if at != indent {
            break;
        }
        let Some((key, value)) = yaml_key(&line) else {
            return Err(format!("expected `key: value`, got `{line}`"));
        };
        *i += 1;
        let value = if value.is_empty() {
            yaml_child(lines, i, indent, true)?
        } else {
            yaml_scalar(value)?
        };
        map.insert(unquote(key), value);
    }
    Ok(Value::Object(map))
}

/// The value nested under a `key:` or `-` line at `indent`: a deeper block,
/// a list at the same indent (only under a `key:`, `same_level_list`), or null.
fn yaml_child(
    lines: &mut [(usize, String)],
    i: &mut usize,
    indent: usize,
    same_level_list: bool,
) -> Result<serde_json::Value, String> {
    match lines.get(*i) {
        Some((at, _)) if *at > indent => {
            let at = *at;
            yaml_node(lines, i, at)
        }
        Some((at, line)) if same_level_list && *at == indent && line.starts_with("- ") => {
            yaml_node(lines, i, indent)
        }
        _ => Ok(serde_json::Value::Null),
    }
}

/// `key: value` split at the first `:` followed by a space or the end.
fn yaml_key(line: &str) -> Option<(&str, &str)> {
    let quoted = line.starts_with(['"', '\'']);
    let start = if quoted {
        line[1..].find(line.as_bytes()[0] as char)? + 2
    } else {
        0
    };
    let at = start + line[start..].find(':')?;
    let rest = &line[at + 1..];
    (rest.is_empty() || rest.starts_with(' ')).then(|| (line[..at].trim(), rest.trim()))
}

fn yaml_scalar(raw: &str) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    if raw.starts_with(['{', '[', '"']) {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            return Ok(v);
        }
        if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            let items = inner.split(',').map(str::trim).filter(|s| !s.is_empty());
            return items
                .map(yaml_scalar)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array);
        }
        if raw.starts_with('{') {
            return Err(format!("inline maps must be JSON: `{raw}`"));
        }
    }
    Ok(match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" | "~" => Value::Null,
        _ => match raw.parse::<serde_json::Number>() {
            Ok(n) => Value::Number(n),
            Err(_e) => Value::String(unquote(raw)),
        },
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Field {
    Text(String),
    List(Vec<String>),
}

impl Field {
    fn text(&self) -> String {
        match self {
            Field::Text(s) => s.clone(),
            Field::List(items) => items.join(", "),
        }
    }

    fn items(&self) -> Vec<String> {
        match self {
            Field::Text(s) => s.split(',').map(|t| t.trim().to_string()).collect(),
            Field::List(items) => items.clone(),
        }
    }
}

/// The YAML subset agent files use: `key: value`, quoted values, inline
/// `[a, b]` lists, `- item` lists, and `|` / `>` block text.
fn frontmatter(block: &str) -> HashMap<String, Field> {
    let mut map: HashMap<String, Field> = HashMap::new();
    let mut key: Option<String> = None;
    let mut fold = " ";
    for line in block.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if let (true, Some(k)) = (indented, &key) {
            let entry = map.entry(k.clone()).or_insert(Field::Text(String::new()));
            match (trimmed.strip_prefix("- "), entry) {
                (Some(item), Field::List(items)) => items.push(unquote(item)),
                (Some(item), slot @ Field::Text(_)) if slot.text().is_empty() => {
                    *slot = Field::List(vec![unquote(item)]);
                }
                (_, Field::Text(s)) => {
                    if !s.is_empty() {
                        s.push_str(fold);
                    }
                    s.push_str(trimmed);
                }
                (None, Field::List(items)) => items.push(unquote(trimmed)),
            }
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let k = k.trim().to_string();
        let v = v.trim();
        fold = if v.starts_with('|') { "\n" } else { " " };
        let field = if v.starts_with('|') || v.starts_with('>') || v.is_empty() {
            Field::Text(String::new())
        } else if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            Field::List(
                inner
                    .split(',')
                    .map(unquote)
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        } else {
            Field::Text(unquote(v))
        };
        map.insert(k.clone(), field);
        key = Some(k);
    }
    map
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let quoted = s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')));
    if quoted {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// PascalCase tool names from shared definition files mapped to WhyCodes ones.
fn tool_names(field: &Field) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in field.items() {
        for name in map_tool(&raw) {
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

fn map_tool(raw: &str) -> Vec<String> {
    let t = raw.trim();
    if let Some((server, tool)) = t.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
        // Same sanitising as the bridge, so dotted, slashed or long server
        // and tool names match the registered `{server}_{tool}`.
        return vec![crate::mcp_load::bridge_tool_name(server, tool)];
    }
    let mapped: &[&str] = match t {
        "Bash" | "bash" | "shell" => &["shell", "bash"],
        "LS" => &["list"],
        "MultiEdit" | "NotebookEdit" => &["edit"],
        "TodoWrite" => &["todowrite"],
        _ => return vec![t.to_ascii_lowercase()],
    };
    mapped.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
#[path = "custom_agents_tests.rs"]
mod tests;
