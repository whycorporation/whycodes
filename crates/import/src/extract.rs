//! Product-specific extractors. Called only after the user approved the path.

use std::path::Path;

use serde_json::Value;

use crate::error::{ImportError, Result};
use crate::parse::{
    hooks_from_claude_object, hooks_from_grok_value, mcp_from_map, parse_jsonc,
    permission_from_lists, permission_from_map, string_list, toml_to_json,
};
use crate::types::{Extracted, FoundSource, Product};

pub fn extract(source: &FoundSource) -> Result<Extracted> {
    if source.state == crate::types::SourceState::Symlink {
        return Err(ImportError::SymlinkRejected(
            source.path.display().to_string(),
        ));
    }
    if source.state != crate::types::SourceState::Approved {
        return Err(ImportError::ConsentRequired(
            source.path.display().to_string(),
        ));
    }
    let text = std::fs::read_to_string(&source.path)?;
    match source.product {
        Product::Claude => extract_claude(&source.path, &text),
        Product::OpenCode => extract_opencode(&source.path, &text),
        Product::Grok => extract_toml_product(Product::Grok, &source.path, &text),
        Product::Codex => extract_toml_product(Product::Codex, &source.path, &text),
    }
}

fn extract_claude(path: &Path, text: &str) -> Result<Extracted> {
    let value = parse_jsonc(text).map_err(|e| ImportError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let mut skipped = Vec::new();
    let mut mcp = Vec::new();
    if let Some(map) = value.get("mcpServers").or_else(|| value.get("mcp_servers")) {
        mcp.extend(mcp_from_map(map, &mut skipped));
    }
    // `.claude/mcp.json` is often just `{ "mcpServers": ... }` or a bare map.
    if mcp.is_empty()
        && value.as_object().is_some_and(|o| {
            !o.contains_key("mcpServers")
                && !o.contains_key("permissions")
                && !o.contains_key("hooks")
        })
    {
        mcp.extend(mcp_from_map(&value, &mut skipped));
    }
    let mut permission = Default::default();
    if let Some(perms) = value.get("permissions").or_else(|| value.get("permission")) {
        if perms.get("allow").is_some() || perms.get("deny").is_some() || perms.get("ask").is_some()
        {
            permission = permission_from_lists(
                &string_list(perms.get("allow").unwrap_or(&Value::Null)),
                &string_list(perms.get("ask").unwrap_or(&Value::Null)),
                &string_list(perms.get("deny").unwrap_or(&Value::Null)),
                &mut skipped,
            );
        } else {
            permission = permission_from_map(perms, &mut skipped);
        }
    }
    let mut hooks = Vec::new();
    if let Some(h) = value.get("hooks") {
        hooks = hooks_from_claude_object(h, &mut skipped);
    }
    Ok(Extracted {
        product: Product::Claude,
        path: path.to_path_buf(),
        mcp,
        permission,
        hooks,
        skipped,
    })
}

fn extract_opencode(path: &Path, text: &str) -> Result<Extracted> {
    let value = parse_jsonc(text).map_err(|e| ImportError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let mut skipped = Vec::new();
    let mcp = value
        .get("mcp")
        .or_else(|| value.get("mcpServers"))
        .map(|m| mcp_from_map(m, &mut skipped))
        .unwrap_or_default();
    let permission = value
        .get("permission")
        .or_else(|| value.get("permissions"))
        .map(|p| permission_from_map(p, &mut skipped))
        .unwrap_or_default();
    let hooks = value
        .get("hooks")
        .map(|h| hooks_from_claude_object(h, &mut skipped))
        .unwrap_or_default();
    Ok(Extracted {
        product: Product::OpenCode,
        path: path.to_path_buf(),
        mcp,
        permission,
        hooks,
        skipped,
    })
}

fn extract_toml_product(product: Product, path: &Path, text: &str) -> Result<Extracted> {
    let toml_val: toml::Value = toml::from_str(text).map_err(|e| ImportError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let value = toml_to_json(toml_val);
    let mut skipped = Vec::new();
    let mcp = value
        .get("mcp_servers")
        .or_else(|| value.get("mcp"))
        .map(|m| mcp_from_map(m, &mut skipped))
        .unwrap_or_default();
    let permission = match value.get("permission").or_else(|| value.get("permissions")) {
        Some(p)
            if p.get("allow").is_some() || p.get("deny").is_some() || p.get("ask").is_some() =>
        {
            permission_from_lists(
                &string_list(p.get("allow").unwrap_or(&Value::Null)),
                &string_list(p.get("ask").unwrap_or(&Value::Null)),
                &string_list(p.get("deny").unwrap_or(&Value::Null)),
                &mut skipped,
            )
        }
        Some(p) => permission_from_map(p, &mut skipped),
        None => Default::default(),
    };
    let hooks = value
        .get("hooks")
        .map(|h| {
            let mut from_claude = hooks_from_claude_object(h, &mut skipped);
            if from_claude.is_empty() {
                from_claude = hooks_from_grok_value(h, &mut skipped);
            }
            from_claude
        })
        .unwrap_or_default();
    Ok(Extracted {
        product,
        path: path.to_path_buf(),
        mcp,
        permission,
        hooks,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SourceState;
    use std::path::PathBuf;

    fn approved(product: Product, path: PathBuf) -> FoundSource {
        FoundSource {
            product,
            rel_path: "x",
            path,
            state: SourceState::Approved,
        }
    }

    #[test]
    fn claude_mcp_permission_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
              "mcpServers": {"fs": {"command": "npx", "args": ["-y", "pkg"]}},
              "permissions": {"allow": ["Read"], "deny": ["Bash"]},
              "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}]}
            }"#,
        )
        .unwrap();
        let got = extract(&approved(Product::Claude, path)).unwrap();
        assert_eq!(got.mcp.len(), 1);
        assert_eq!(got.permission.len(), 2);
        assert_eq!(got.hooks.len(), 1);
    }

    #[test]
    fn claude_bare_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"github": {"command": "npx"}}"#).unwrap();
        let got = extract(&approved(Product::Claude, path)).unwrap();
        assert_eq!(got.mcp.len(), 1);
        assert_eq!(got.mcp[0].0, "github");
    }

    #[test]
    fn opencode_jsonc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.jsonc");
        std::fs::write(
            &path,
            r#"{
              // comment
              "mcp": {"fs": {"type": "local", "command": ["npx", "-y", "pkg"]}},
              "permission": {"bash": "ask"}
            }"#,
        )
        .unwrap();
        let got = extract(&approved(Product::OpenCode, path)).unwrap();
        assert_eq!(got.mcp.len(), 1);
        assert_eq!(
            got.permission.get("bash"),
            Some(&whycodes_core::types::PermissionAction::Ask)
        );
    }

    #[test]
    fn grok_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.github]
command = "gh"
args = ["mcp"]

[permission]
deny = ["Bash"]
allow = ["Read"]

[hooks.pre_tool]
command = "echo pre"
matcher = "bash"
"#,
        )
        .unwrap();
        let got = extract(&approved(Product::Grok, path)).unwrap();
        assert_eq!(got.mcp.len(), 1);
        assert_eq!(
            got.permission.get("bash"),
            Some(&whycodes_core::types::PermissionAction::Deny)
        );
        assert_eq!(got.hooks.len(), 1);
    }

    #[test]
    fn extract_requires_consent() {
        let src = FoundSource {
            product: Product::Codex,
            rel_path: "x",
            path: PathBuf::from("/nope"),
            state: SourceState::New,
        };
        assert!(extract(&src).is_err());
        let src = FoundSource {
            product: Product::Codex,
            rel_path: "x",
            path: PathBuf::from("/nope"),
            state: SourceState::Symlink,
        };
        assert!(extract(&src).is_err());
    }

    #[test]
    fn claude_permission_object_and_opencode_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"permissions": {"bash": "ask", "read": "allow"}}"#,
        )
        .unwrap();
        let got = extract(&approved(Product::Claude, path)).unwrap();
        assert_eq!(
            got.permission.get("bash"),
            Some(&whycodes_core::types::PermissionAction::Ask)
        );
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{").unwrap();
        assert!(extract(&approved(Product::Claude, bad)).is_err());
        let oc = dir.path().join("oc.json");
        std::fs::write(&oc, "{").unwrap();
        assert!(extract(&approved(Product::OpenCode, oc)).is_err());
    }

    #[test]
    fn grok_permission_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[permission]
bash = "ask"
read = "allow"

[mcp]
fs = { command = "npx" }
"#,
        )
        .unwrap();
        let got = extract(&approved(Product::Grok, path)).unwrap();
        assert_eq!(
            got.permission.get("bash"),
            Some(&whycodes_core::types::PermissionAction::Ask)
        );
        assert_eq!(got.mcp.len(), 1);
    }

    #[test]
    fn parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "[[[").unwrap();
        assert!(extract(&approved(Product::Codex, path)).is_err());
    }
}
