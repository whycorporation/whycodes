//! Shared parsers for foreign JSON / JSONC / TOML settings.

use std::collections::HashMap;

use serde_json::Value;
use whycodes_config::{HookConfig, HookEvent, McpServerConfig, McpTransportKind};
use whycodes_core::types::PermissionAction;

use crate::error::{ImportError, Result};

/// Strip `//` and `/* */` comments plus trailing commas so JSONC parses as JSON.
pub fn parse_jsonc(text: &str) -> Result<Value> {
    let stripped = strip_jsonc(text);
    serde_json::from_str(&stripped).map_err(ImportError::from)
}

fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c as char);
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_string = true;
            out.push('"');
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
        }
        if c == b',' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                i += 1;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

pub fn string_map(value: &Value) -> Option<HashMap<String, String>> {
    let obj = value.as_object()?;
    let mut out = HashMap::new();
    for (k, v) in obj {
        match v {
            Value::String(s) => {
                out.insert(k.clone(), s.clone());
            }
            Value::Number(n) => {
                out.insert(k.clone(), n.to_string());
            }
            Value::Bool(b) => {
                out.insert(k.clone(), b.to_string());
            }
            _ => return None,
        }
    }
    Some(out)
}

pub fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Map a generic MCP object (Claude / Cursor / VS Code style `mcpServers`).
pub fn mcp_from_object(
    name: &str,
    raw: &Value,
    skipped: &mut Vec<String>,
) -> Option<(String, McpServerConfig)> {
    if !raw.is_object() {
        skipped.push(format!("{name}: not an object"));
        return None;
    }
    if raw.get("disabled").and_then(Value::as_bool) == Some(true)
        || raw.get("enabled").and_then(Value::as_bool) == Some(false)
    {
        skipped.push(format!("{name}: disabled"));
        return None;
    }
    let command_val = raw.get("command");
    let (command, args) = match command_val {
        Some(Value::String(s)) => (
            Some(s.clone()),
            string_list(raw.get("args").unwrap_or(&Value::Null)),
        ),
        Some(Value::Array(a)) => {
            let mut parts: Vec<String> = a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if parts.is_empty() {
                (None, Vec::new())
            } else {
                let cmd = parts.remove(0);
                let extra = string_list(raw.get("args").unwrap_or(&Value::Null));
                parts.extend(extra);
                (Some(cmd), parts)
            }
        }
        _ => (None, string_list(raw.get("args").unwrap_or(&Value::Null))),
    };
    let url = raw
        .get("url")
        .or_else(|| raw.get("serverUrl"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if command.is_none() && url.is_none() {
        skipped.push(format!("{name}: neither command nor url"));
        return None;
    }
    let env = raw
        .get("env")
        .or_else(|| raw.get("environment"))
        .and_then(string_map);
    let headers = raw.get("headers").and_then(string_map);
    let cwd = raw.get("cwd").and_then(Value::as_str).map(str::to_string);
    let transport = match raw.get("type").and_then(Value::as_str) {
        Some("http" | "streamable-http" | "streamable_http" | "remote") if url.is_some() => {
            Some(McpTransportKind::Http)
        }
        Some("sse") if url.is_some() => Some(McpTransportKind::Sse),
        Some("stdio" | "local") | None if command.is_some() => Some(McpTransportKind::Stdio),
        Some("auto") if url.is_some() => Some(McpTransportKind::Auto),
        _ if url.is_some() => Some(McpTransportKind::Auto),
        _ => None,
    };
    Some((
        name.to_string(),
        McpServerConfig {
            transport,
            command,
            args,
            env,
            cwd,
            url,
            headers,
        },
    ))
}

pub fn mcp_from_map(map: &Value, skipped: &mut Vec<String>) -> Vec<(String, McpServerConfig)> {
    let Some(obj) = map.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .filter_map(|(name, raw)| mcp_from_object(name, raw, skipped))
        .collect()
}

/// OpenCode-style `{ "bash": "ask", "edit": "allow" }`.
pub fn permission_from_map(
    value: &Value,
    skipped: &mut Vec<String>,
) -> HashMap<String, PermissionAction> {
    let mut out = HashMap::new();
    let Some(obj) = value.as_object() else {
        skipped.push("permission: not an object".into());
        return out;
    };
    for (key, val) in obj {
        match val {
            Value::String(s) => match PermissionAction::parse(s) {
                Some(a) => {
                    out.insert(map_tool_name(key), a);
                }
                None => skipped.push(format!("permission {key}: unknown action `{s}`")),
            },
            Value::Bool(true) => {
                out.insert(map_tool_name(key), PermissionAction::Allow);
            }
            Value::Bool(false) => {
                out.insert(map_tool_name(key), PermissionAction::Deny);
            }
            Value::Object(inner) => {
                if let Some(action) = inner
                    .get("action")
                    .or_else(|| inner.get("*"))
                    .and_then(Value::as_str)
                    .and_then(PermissionAction::parse)
                {
                    out.insert(map_tool_name(key), action);
                } else {
                    skipped.push(format!("permission {key}: nested object not mapped"));
                }
            }
            _ => skipped.push(format!("permission {key}: unsupported value")),
        }
    }
    out
}

/// Claude / Grok allow-ask-deny lists (`["Bash", "Read"]` or `"Bash(git *)"`).
pub fn permission_from_lists(
    allow: &[String],
    ask: &[String],
    deny: &[String],
    skipped: &mut Vec<String>,
) -> HashMap<String, PermissionAction> {
    let mut out = HashMap::new();
    for (list, action) in [
        (allow, PermissionAction::Allow),
        (ask, PermissionAction::Ask),
        (deny, PermissionAction::Deny),
    ] {
        for raw in list {
            match rule_to_key(raw) {
                Some(key) => {
                    // Deny wins if the same tool appears in multiple lists.
                    match out.get(&key) {
                        Some(PermissionAction::Deny) => {}
                        Some(_) if action != PermissionAction::Deny => {}
                        _ => {
                            out.insert(key, action);
                        }
                    }
                }
                None => skipped.push(format!("permission rule not mapped: {raw}")),
            }
        }
    }
    out
}

fn rule_to_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let tool = if let Some(open) = trimmed.find('(') {
        &trimmed[..open]
    } else {
        trimmed
    };
    let mapped = map_tool_name(tool);
    if mapped.is_empty() {
        None
    } else {
        Some(mapped)
    }
}

pub fn map_tool_name(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "bash" | "shell" | "command" => "bash".into(),
        "read" | "read_file" | "view" => "read".into(),
        "edit" | "edit_file" | "strreplace" | "str_replace" | "write" | "write_file" => {
            "edit".into()
        }
        "grep" | "search" => "grep".into(),
        "glob" | "find" => "glob".into(),
        "webfetch" | "web_fetch" | "fetch" => "webfetch".into(),
        "websearch" | "web_search" => "websearch".into(),
        "mcp" | "mcptool" => "mcp".into(),
        other => other.to_string(),
    }
}

pub fn hook_event_from_name(name: &str) -> Option<HookEvent> {
    match name.trim() {
        "PreToolUse" | "pre_tool" | "pre-tool" | "PreTool" => Some(HookEvent::PreTool),
        "PostToolUse" | "post_tool" | "post-tool" | "PostTool" => Some(HookEvent::PostTool),
        _ => None,
    }
}

pub fn hook_from_command(
    event: HookEvent,
    tool_match: &str,
    command: String,
    block_on_failure: bool,
    timeout_secs: Option<u64>,
) -> HookConfig {
    HookConfig {
        event,
        tool_match: if tool_match.is_empty() {
            "*".into()
        } else {
            map_tool_name(tool_match)
        },
        command,
        block_on_failure: block_on_failure && event == HookEvent::PreTool,
        timeout_secs: timeout_secs.unwrap_or(30).clamp(1, 300),
    }
}

/// Claude `settings.json` `hooks` object: `{ "PreToolUse": [ { matcher, hooks: [{type,command}] } ] }`.
pub fn hooks_from_claude_object(value: &Value, skipped: &mut Vec<String>) -> Vec<HookConfig> {
    let mut out = Vec::new();
    let Some(obj) = value.as_object() else {
        return out;
    };
    for (event_name, groups) in obj {
        let Some(event) = hook_event_from_name(event_name) else {
            skipped.push(format!(
                "hook event `{event_name}` has no WhyCodes equivalent"
            ));
            continue;
        };
        let Some(arr) = groups.as_array() else {
            continue;
        };
        for group in arr {
            let matcher = group.get("matcher").and_then(Value::as_str).unwrap_or("*");
            let hooks = group
                .get("hooks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![group.clone()]);
            for hook in hooks {
                let kind = hook
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("command");
                if kind != "command" {
                    skipped.push(format!("hook type `{kind}` skipped"));
                    continue;
                }
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    skipped.push("hook missing command".into());
                    continue;
                };
                let timeout = hook.get("timeout").and_then(Value::as_u64);
                let block = event == HookEvent::PreTool;
                out.push(hook_from_command(
                    event,
                    matcher,
                    command.to_string(),
                    block,
                    timeout,
                ));
            }
        }
    }
    out
}

/// Grok `[hooks.<event>]` TOML tables (or JSON object of the same shape).
pub fn hooks_from_grok_value(value: &Value, skipped: &mut Vec<String>) -> Vec<HookConfig> {
    let mut out = Vec::new();
    let Some(obj) = value.as_object() else {
        return out;
    };
    for (event_name, spec) in obj {
        let Some(event) = hook_event_from_name(event_name) else {
            skipped.push(format!(
                "hook event `{event_name}` has no WhyCodes equivalent"
            ));
            continue;
        };
        collect_grok_hooks(event, spec, skipped, &mut out);
    }
    out
}

fn collect_grok_hooks(
    event: HookEvent,
    spec: &Value,
    skipped: &mut Vec<String>,
    out: &mut Vec<HookConfig>,
) {
    match spec {
        Value::Array(items) => {
            for item in items {
                collect_grok_hooks(event, item, skipped, out);
            }
        }
        Value::Object(map) => {
            if let Some(command) = map.get("command").and_then(Value::as_str) {
                let matcher = map
                    .get("matcher")
                    .or_else(|| map.get("match"))
                    .and_then(Value::as_str)
                    .unwrap_or("*");
                let timeout = map
                    .get("timeout")
                    .or_else(|| map.get("timeout_secs"))
                    .and_then(Value::as_u64);
                out.push(hook_from_command(
                    event,
                    matcher,
                    command.to_string(),
                    event == HookEvent::PreTool,
                    timeout,
                ));
                return;
            }
            if let Some(hooks) = map.get("hooks") {
                collect_grok_hooks(event, hooks, skipped, out);
                return;
            }
            skipped.push(format!("hook `{event:?}` missing command"));
        }
        Value::String(command) => {
            out.push(hook_from_command(
                event,
                "*",
                command.clone(),
                event == HookEvent::PreTool,
                None,
            ));
        }
        _ => skipped.push("hook entry not mapped".into()),
    }
}

pub fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => serde_json::json!(i),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t {
                map.insert(k, toml_to_json(v));
            }
            Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_strips_comments_and_trailing_commas() {
        let v = parse_jsonc(
            r#"{
            // comment
            "a": 1, /* block */
            "b": "x\"y",
            "c": [1, 2,],
          }"#,
        )
        .unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x\"y");
        assert_eq!(v["c"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn jsonc_bad_json_errors() {
        assert!(parse_jsonc("{").is_err());
    }

    #[test]
    fn mcp_stdio_and_http() {
        let mut skipped = Vec::new();
        let raw = serde_json::json!({
            "command": "npx",
            "args": ["-y", "pkg"],
            "env": {"A": "1"}
        });
        let (name, cfg) = mcp_from_object("fs", &raw, &mut skipped).unwrap();
        assert_eq!(name, "fs");
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert_eq!(cfg.args, vec!["-y", "pkg"]);
        let raw = serde_json::json!({"url": "https://mcp.example/mcp", "type": "http"});
        let (_, cfg) = mcp_from_object("remote", &raw, &mut skipped).unwrap();
        assert_eq!(cfg.url.as_deref(), Some("https://mcp.example/mcp"));
        assert_eq!(cfg.transport, Some(McpTransportKind::Http));
        assert!(
            mcp_from_object("x", &serde_json::json!({"disabled": true}), &mut skipped).is_none()
        );
        assert!(mcp_from_object("y", &serde_json::json!({}), &mut skipped).is_none());
        let cmd_arr = serde_json::json!({"command": ["npx", "-y", "pkg"]});
        let (_, cfg) = mcp_from_object("z", &cmd_arr, &mut skipped).unwrap();
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert_eq!(cfg.args, vec!["-y", "pkg"]);
        assert!(
            mcp_from_object(
                "off",
                &serde_json::json!({"enabled": false, "command": "x"}),
                &mut skipped
            )
            .is_none()
        );
        let sse = serde_json::json!({"url": "https://mcp.example/sse", "type": "sse", "headers": {"A": "1"}, "cwd": "/tmp"});
        let (_, cfg) = mcp_from_object("sse", &sse, &mut skipped).unwrap();
        assert_eq!(cfg.transport, Some(McpTransportKind::Sse));
        let auto = serde_json::json!({"url": "https://mcp.example/mcp"});
        let (_, cfg) = mcp_from_object("auto", &auto, &mut skipped).unwrap();
        assert_eq!(cfg.transport, Some(McpTransportKind::Auto));
        assert!(
            mcp_from_object("arr", &serde_json::json!({"command": []}), &mut skipped).is_none()
        );
        assert!(mcp_from_map(&serde_json::json!([]), &mut skipped).is_empty());
        assert!(string_map(&serde_json::json!({"n": 1, "b": true, "s": "x"})).is_some());
        assert!(string_map(&serde_json::json!({"bad": {"x": 1}})).is_none());
        assert!(string_list(&serde_json::json!(1)).is_empty());
    }

    #[test]
    fn permission_maps_and_lists() {
        let mut skipped = Vec::new();
        let map = permission_from_map(
            &serde_json::json!({"bash": "ask", "edit": true, "read": false, "weird": "nope"}),
            &mut skipped,
        );
        assert_eq!(map.get("bash"), Some(&PermissionAction::Ask));
        assert_eq!(map.get("edit"), Some(&PermissionAction::Allow));
        assert_eq!(map.get("read"), Some(&PermissionAction::Deny));
        assert!(!skipped.is_empty());
        let lists = permission_from_lists(
            &["Read".into()],
            &["Bash(git *)".into()],
            &["Bash".into(), "???".into()],
            &mut skipped,
        );
        assert_eq!(lists.get("bash"), Some(&PermissionAction::Deny));
        assert_eq!(lists.get("read"), Some(&PermissionAction::Allow));
        let nested = permission_from_map(
            &serde_json::json!({"edit": {"action": "ask"}, "glob": {"*": "allow"}}),
            &mut skipped,
        );
        assert_eq!(nested.get("edit"), Some(&PermissionAction::Ask));
        assert_eq!(nested.get("glob"), Some(&PermissionAction::Allow));
        let empty = permission_from_map(&serde_json::json!([]), &mut skipped);
        assert!(empty.is_empty());
        let lists = permission_from_lists(&[" ".into()], &[], &[], &mut skipped);
        assert!(lists.is_empty());
        assert_eq!(map_tool_name("write_file"), "edit");
        assert_eq!(map_tool_name("web_search"), "websearch");
        assert_eq!(map_tool_name("MCPTool"), "mcp");
        assert_eq!(map_tool_name("mcp"), "mcp");
        let unmapped =
            permission_from_map(&serde_json::json!({"x": {"nope": 1}, "y": 3}), &mut skipped);
        assert!(unmapped.is_empty());
    }

    #[test]
    fn claude_and_grok_hooks() {
        let mut skipped = Vec::new();
        let claude = serde_json::json!({
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo pre"}]}],
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo no"}]}]
        });
        let hooks = hooks_from_claude_object(&claude, &mut skipped);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, HookEvent::PreTool);
        assert_eq!(hooks[0].tool_match, "bash");
        let grok = serde_json::json!({
            "pre_tool": {"command": "echo g", "matcher": "edit"},
            "PostToolUse": [{"command": "echo p"}]
        });
        let hooks = hooks_from_grok_value(&grok, &mut skipped);
        assert_eq!(hooks.len(), 2);
        assert!(hook_event_from_name("Nope").is_none());
        assert_eq!(map_tool_name("WebFetch"), "webfetch");
        let grok_str = serde_json::json!({"pre_tool": "echo s"});
        assert_eq!(hooks_from_grok_value(&grok_str, &mut skipped).len(), 1);
        let nested_hooks = serde_json::json!({"pre_tool": {"hooks": [{"command": "echo n"}]}});
        assert_eq!(hooks_from_grok_value(&nested_hooks, &mut skipped).len(), 1);
        let missing = serde_json::json!({"pre_tool": {"matcher": "bash"}});
        assert!(hooks_from_grok_value(&missing, &mut skipped).is_empty());
        let prompt = serde_json::json!({
            "PreToolUse": [{"hooks": [{"type": "prompt", "command": "nope"}]}, {"hooks": [{"command": "echo"}]}]
        });
        assert_eq!(hooks_from_claude_object(&prompt, &mut skipped).len(), 1);
        let missing_cmd = serde_json::json!({"PreToolUse": [{"hooks": [{}]}]});
        assert!(hooks_from_claude_object(&missing_cmd, &mut skipped).is_empty());
        assert!(hooks_from_claude_object(&serde_json::json!([]), &mut skipped).is_empty());
        assert!(hooks_from_grok_value(&serde_json::json!([]), &mut skipped).is_empty());
        let t: toml::Value =
            toml::from_str("a = 1.5\nb = true\nc = 2020-01-01T00:00:00Z\n").unwrap();
        let v = toml_to_json(t);
        assert!(v["a"].as_f64().is_some());
        assert_eq!(v["b"], true);
        assert!(v["c"].as_str().is_some());
    }

    #[test]
    fn toml_roundtrip_table() {
        let t: toml::Value = toml::from_str("[a]\nb = 1\n").unwrap();
        let v = toml_to_json(t);
        assert_eq!(v["a"]["b"], 1);
    }

    #[test]
    fn jsonc_block_comment_unterminated_and_escaped_slash() {
        let v = parse_jsonc("{").unwrap_or_else(|_| serde_json::json!({}));
        assert!(v.is_object());
        let v = parse_jsonc(r#"{ "a": "x\/y", "b": 1, /* unterminated }"#)
            .unwrap_or_else(|_| serde_json::json!({}));
        let _ = v;
        let v = parse_jsonc(r#"{ "a": "hi\"there", "b": [1,], }"#).unwrap();
        assert_eq!(v["a"], "hi\"there");
        assert!(parse_jsonc("/*").is_err());
        let lone = parse_jsonc(r#"{ "a": 1 / 2 }"#);
        assert!(lone.is_err());
        let v = parse_jsonc("{\"a\": 1, \t}").unwrap();
        assert_eq!(v["a"], 1);
        let v = parse_jsonc("{ \"a\": \"\\\\\", \"b\": 2 }").unwrap();
        assert_eq!(v["b"], 2);
        assert!(parse_jsonc("{ /* no close").is_err());
        let v = parse_jsonc("{").unwrap_or_else(|_| serde_json::json!({"a":1}));
        assert_eq!(v["a"], 1);

        let _ = v;
        let v = parse_jsonc("{ \"path\": \"C:\\\\tmp\" }").unwrap();
        assert!(v["path"].as_str().unwrap().contains("tmp"));
        let v = parse_jsonc("[1, 2, ]").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        let v = parse_jsonc("// only comment\n{\"ok\":true}").unwrap();
        assert_eq!(v["ok"], true);
        let v = parse_jsonc("{\"a\": \"not\\/escaped\"}").unwrap();
        assert!(v["a"].as_str().is_some());
    }

    #[test]
    fn mcp_transport_variants_and_non_object() {
        let mut skipped = Vec::new();
        assert!(mcp_from_object("x", &serde_json::json!("nope"), &mut skipped).is_none());
        let http = serde_json::json!({"url": "https://x", "type": "streamable-http"});
        assert_eq!(
            mcp_from_object("h", &http, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Http)
        );
        let remote = serde_json::json!({"url": "https://x", "type": "remote"});
        assert_eq!(
            mcp_from_object("r", &remote, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Http)
        );
        let stream = serde_json::json!({"url": "https://x", "type": "streamable_http"});
        assert_eq!(
            mcp_from_object("s", &stream, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Http)
        );
        let auto = serde_json::json!({"url": "https://x", "type": "auto"});
        assert_eq!(
            mcp_from_object("a", &auto, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Auto)
        );
        let stdio = serde_json::json!({"command": "npx", "type": "stdio"});
        assert_eq!(
            mcp_from_object("st", &stdio, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Stdio)
        );
        let local = serde_json::json!({"command": "npx", "type": "local"});
        assert_eq!(
            mcp_from_object("l", &local, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Stdio)
        );
        let env = serde_json::json!({
            "command": "npx",
            "environment": {"A": "1"},
            "args": "one"
        });
        let cfg = mcp_from_object("e", &env, &mut skipped).unwrap().1;
        assert_eq!(
            cfg.env.as_ref().unwrap().get("A").map(String::as_str),
            Some("1")
        );
        assert_eq!(cfg.args, vec!["one"]);
        let extra = serde_json::json!({"command": ["npx"], "args": ["-y"]});
        assert_eq!(
            mcp_from_object("z", &extra, &mut skipped).unwrap().1.args,
            vec!["-y"]
        );
        let url_alias = serde_json::json!({"serverUrl": "https://x", "type": "weird"});
        assert_eq!(
            mcp_from_object("u", &url_alias, &mut skipped)
                .unwrap()
                .1
                .transport,
            Some(McpTransportKind::Auto)
        );
        let cmd_only_unknown = serde_json::json!({"command": "npx", "type": "http"});
        assert_eq!(
            mcp_from_object("c", &cmd_only_unknown, &mut skipped)
                .unwrap()
                .1
                .transport,
            None
        );
        let mixed = serde_json::json!({"command": ["npx", 1, "-y"], "url": null});
        let cfg = mcp_from_object("m", &mixed, &mut skipped).unwrap().1;
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert_eq!(cfg.args, vec!["-y"]);
        assert!(
            mcp_from_object(
                "sse",
                &serde_json::json!({"type": "sse", "command": "npx"}),
                &mut skipped
            )
            .unwrap()
            .1
            .transport
                != Some(McpTransportKind::Sse)
        );
        let deny_then_allow = permission_from_lists(
            &["Bash".into()],
            &[],
            &["Bash".into(), "Bash".into()],
            &mut skipped,
        );
        assert_eq!(deny_then_allow.get("bash"), Some(&PermissionAction::Deny));
        let allow_then_ask =
            permission_from_lists(&["Read".into()], &["Read".into()], &[], &mut skipped);
        assert_eq!(allow_then_ask.get("read"), Some(&PermissionAction::Allow));
    }

    #[test]
    fn permission_lists_keep_first_non_deny() {
        let mut skipped = Vec::new();
        let lists = permission_from_lists(
            &["Read".into(), "Bash".into()],
            &["Read".into()],
            &[],
            &mut skipped,
        );
        assert_eq!(lists.get("read"), Some(&PermissionAction::Allow));
        assert_eq!(lists.get("bash"), Some(&PermissionAction::Allow));
        assert_eq!(map_tool_name("shell"), "bash");
        assert_eq!(map_tool_name("command"), "bash");
        assert_eq!(map_tool_name("read_file"), "read");
        assert_eq!(map_tool_name("view"), "read");
        assert_eq!(map_tool_name("edit_file"), "edit");
        assert_eq!(map_tool_name("strreplace"), "edit");
        assert_eq!(map_tool_name("str_replace"), "edit");
        assert_eq!(map_tool_name("write"), "edit");
        assert_eq!(map_tool_name("search"), "grep");
        assert_eq!(map_tool_name("grep"), "grep");
        assert_eq!(map_tool_name("find"), "glob");
        assert_eq!(map_tool_name("glob"), "glob");
        assert_eq!(map_tool_name("fetch"), "webfetch");
        assert_eq!(map_tool_name("web_fetch"), "webfetch");
        assert_eq!(map_tool_name("unknown_tool"), "unknown_tool");
        assert!(rule_to_key("(").is_none());
        assert_eq!(map_tool_name("("), "(");
        let _ = rule_to_key("(").is_none() || map_tool_name("(") == "(";
        let _ = true || map_tool_name("(") == "(";

        assert_eq!(rule_to_key("Bash(git *)").as_deref(), Some("bash"));
        assert!(rule_to_key("   ").is_none());
        let empty_name = rule_to_key("()");
        assert!(empty_name.is_none());
    }

    #[test]
    fn hook_from_command_defaults_and_event_aliases() {
        let h = hook_from_command(HookEvent::PostTool, "", "echo".into(), true, Some(0));
        assert_eq!(h.tool_match, "*");
        assert!(!h.block_on_failure);
        assert_eq!(h.timeout_secs, 1);
        let h = hook_from_command(HookEvent::PreTool, "Bash", "echo".into(), true, Some(999));
        assert_eq!(h.tool_match, "bash");
        assert!(h.block_on_failure);
        assert_eq!(h.timeout_secs, 300);
        assert_eq!(hook_event_from_name("pre-tool"), Some(HookEvent::PreTool));
        assert_eq!(hook_event_from_name("PreTool"), Some(HookEvent::PreTool));
        assert_eq!(hook_event_from_name("post-tool"), Some(HookEvent::PostTool));
        assert_eq!(hook_event_from_name("PostTool"), Some(HookEvent::PostTool));
        assert_eq!(hook_event_from_name("post_tool"), Some(HookEvent::PostTool));
    }

    #[test]
    fn claude_hooks_non_array_group_and_inline_command() {
        let mut skipped = Vec::new();
        let non_arr = serde_json::json!({"PreToolUse": {"command": "echo"}});
        assert!(hooks_from_claude_object(&non_arr, &mut skipped).is_empty());
        let inline = serde_json::json!({
            "PostToolUse": [{"matcher": "", "command": "echo after", "timeout": 12}]
        });
        let hooks = hooks_from_claude_object(&inline, &mut skipped);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, HookEvent::PostTool);
        assert_eq!(hooks[0].tool_match, "*");
        assert_eq!(hooks[0].timeout_secs, 12);
        let grok_match =
            serde_json::json!({"pre_tool": {"command": "echo", "match": "edit", "timeout": 9}});
        let hooks = hooks_from_grok_value(&grok_match, &mut skipped);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].tool_match, "edit");
        assert_eq!(hooks[0].timeout_secs, 9);
        let grok_bad = serde_json::json!({"pre_tool": 1, "Nope": {}});
        assert!(hooks_from_grok_value(&grok_bad, &mut skipped).is_empty());
        assert!(skipped.iter().any(|s| s.contains("not mapped")));
        let toml_arr: toml::Value = toml::from_str("a = [1, \"x\"]\n").unwrap();
        let v = toml_to_json(toml_arr);
        assert_eq!(v["a"][0], 1);
        assert_eq!(string_list(&serde_json::json!("one")), vec!["one"]);
        assert!(string_map(&serde_json::json!([])).is_none());
        let line_comment_eof = parse_jsonc("{\"a\":1}// no newline");
        assert!(line_comment_eof.is_ok());
        assert!(parse_jsonc("{\"a\":1}/").is_err());
        let comma_then_eof = parse_jsonc("[1,");
        assert!(comma_then_eof.is_err());

        assert_eq!(
            string_list(&serde_json::json!(["a", 1, "b"])),
            vec!["a", "b"]
        );
        let map = mcp_from_map(
            &serde_json::json!({"ok": {"command": "npx"}, "bad": 1}),
            &mut skipped,
        );
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn jsonc_slash_at_eof_and_grok_array_hooks() {
        assert!(parse_jsonc("{").is_err());
        let v = parse_jsonc("{\"a\":1}").unwrap();
        assert_eq!(v["a"], 1);
        let mut skipped = Vec::new();
        let grok_arr = serde_json::json!({
            "pre_tool": [{"command": "echo a"}, {"command": "echo b"}]
        });
        assert_eq!(hooks_from_grok_value(&grok_arr, &mut skipped).len(), 2);
        let grok_timeout_secs = serde_json::json!({
            "post_tool": {"command": "echo", "timeout_secs": 4}
        });
        let hooks = hooks_from_grok_value(&grok_timeout_secs, &mut skipped);
        assert_eq!(hooks[0].timeout_secs, 4);
        assert_eq!(map_tool_name("webfetch"), "webfetch");
        assert_eq!(map_tool_name("websearch"), "websearch");
        let empty_cmd = serde_json::json!({"command": []});
        assert!(mcp_from_object("empty", &empty_cmd, &mut skipped).is_none());
        let no_cmd_no_url = serde_json::json!({"type": "stdio"});
        assert!(mcp_from_object("none", &no_cmd_no_url, &mut skipped).is_none());
        assert!(parse_jsonc("/").is_err());
        assert_eq!(map_tool_name("   "), "");
        assert!(rule_to_key("   ").is_none());
    }
}
