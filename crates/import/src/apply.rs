//! Merge extracted settings into a WhyCodes `Config`. Existing keys win.

use whycodes_config::Config;

use crate::types::{Extracted, ImportPlan};

pub fn plan(config: &Config, extracted: &[Extracted], force: bool) -> ImportPlan {
    let mut plan = ImportPlan::default();
    for item in extracted {
        plan.warnings.extend(item.skipped.iter().cloned());
        for (name, server) in &item.mcp {
            if !force && config.mcp_servers.contains_key(name) {
                plan.mcp_skip
                    .push((name.clone(), format!("already have `{name}`")));
                continue;
            }
            if plan.mcp_add.iter().any(|(n, _)| n == name) {
                plan.mcp_skip
                    .push((name.clone(), format!("already queued `{name}`")));
                continue;
            }
            plan.mcp_add.push((name.clone(), server.clone()));
        }
        for (tool, action) in &item.permission {
            if !force && config.permission.contains_key(tool) {
                plan.permission_skip
                    .push((tool.clone(), format!("already have `{tool}`")));
                continue;
            }
            if plan.permission_add.iter().any(|(t, _)| t == tool) {
                plan.permission_skip
                    .push((tool.clone(), format!("already queued `{tool}`")));
                continue;
            }
            plan.permission_add.push((tool.clone(), *action));
        }
        for hook in &item.hooks {
            let dup_existing = config.hooks.iter().any(|h| {
                h.event == hook.event
                    && h.command == hook.command
                    && h.tool_match == hook.tool_match
            });
            let dup_queued = plan.hooks_add.iter().any(|h| {
                h.event == hook.event
                    && h.command == hook.command
                    && h.tool_match == hook.tool_match
            });
            if dup_existing || dup_queued {
                plan.hooks_skip
                    .push(format!("{} `{}`", hook.event_label(), hook.command));
                continue;
            }
            plan.hooks_add.push(hook.clone());
        }
    }
    plan
}

trait HookEventLabel {
    fn event_label(&self) -> &'static str;
}

impl HookEventLabel for whycodes_config::HookConfig {
    fn event_label(&self) -> &'static str {
        match self.event {
            whycodes_config::HookEvent::PreTool => "pre_tool",
            whycodes_config::HookEvent::PostTool => "post_tool",
        }
    }
}

pub fn apply(config: &mut Config, plan: &ImportPlan) {
    for (name, server) in &plan.mcp_add {
        config.mcp_servers.insert(name.clone(), server.clone());
    }
    for (tool, action) in &plan.permission_add {
        config.permission.insert(tool.clone(), *action);
    }
    config.hooks.extend(plan.hooks_add.iter().cloned());
}

/// Load current user config (defaults if missing), merge the plan, save.
pub fn apply_and_save(plan: &ImportPlan) -> crate::error::Result<std::path::PathBuf> {
    let mut config = whycodes_config::Config::load()
        .map_err(|e| crate::error::ImportError::Msg(e.to_string()))?;
    apply(&mut config, plan);
    config
        .save()
        .map_err(|e| crate::error::ImportError::Msg(e.to_string()))?;
    let path = whycodes_config::Config::default_path()
        .map_err(|e| crate::error::ImportError::Msg(e.to_string()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Product;
    use std::path::PathBuf;
    use whycodes_config::{HookConfig, HookEvent, McpServerConfig};
    use whycodes_core::types::PermissionAction;

    fn mcp(name: &str) -> (String, McpServerConfig) {
        (
            name.into(),
            McpServerConfig {
                transport: None,
                command: Some("npx".into()),
                args: vec![],
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        )
    }

    #[test]
    fn existing_keys_win_unless_force() {
        let mut cfg = Config::default();
        let (n, s) = mcp("fs");
        cfg.mcp_servers.insert(n.clone(), s);
        cfg.permission.insert("bash".into(), PermissionAction::Ask);
        cfg.hooks.push(HookConfig {
            event: HookEvent::PreTool,
            tool_match: "bash".into(),
            command: "echo hi".into(),
            block_on_failure: true,
            timeout_secs: 30,
        });
        let extracted = Extracted {
            product: Product::Claude,
            path: PathBuf::from("/x"),
            mcp: vec![mcp("fs"), mcp("git")],
            permission: {
                let mut m = std::collections::HashMap::new();
                m.insert("bash".into(), PermissionAction::Deny);
                m.insert("read".into(), PermissionAction::Allow);
                m
            },
            hooks: vec![
                HookConfig {
                    event: HookEvent::PreTool,
                    tool_match: "bash".into(),
                    command: "echo hi".into(),
                    block_on_failure: true,
                    timeout_secs: 30,
                },
                HookConfig {
                    event: HookEvent::PostTool,
                    tool_match: "*".into(),
                    command: "echo after".into(),
                    block_on_failure: false,
                    timeout_secs: 30,
                },
            ],
            skipped: vec!["ignored event".into()],
        };
        let p = plan(&cfg, std::slice::from_ref(&extracted), false);
        assert_eq!(p.mcp_add.len(), 1);
        assert_eq!(p.mcp_skip.len(), 1);
        assert_eq!(p.permission_add.len(), 1);
        assert_eq!(p.permission_skip.len(), 1);
        assert_eq!(p.hooks_add.len(), 1);
        assert_eq!(p.hooks_skip.len(), 1);
        assert!(!p.warnings.is_empty());
        let forced = plan(&cfg, std::slice::from_ref(&extracted), true);
        assert_eq!(forced.mcp_add.len(), 2);
        apply(&mut cfg, &p);
        assert!(cfg.mcp_servers.contains_key("git"));
        assert_eq!(cfg.permission.get("read"), Some(&PermissionAction::Allow));
        assert_eq!(cfg.hooks.len(), 2);
        // queued duplicate skipped
        let again = plan(&cfg, &[extracted], false);
        assert!(again.mcp_add.is_empty());
    }
}
