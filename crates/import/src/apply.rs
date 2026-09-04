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

fn map_msg(e: impl ToString) -> crate::error::ImportError {
    crate::error::ImportError::Msg(e.to_string())
}

pub fn apply_and_save(plan: &ImportPlan) -> crate::error::Result<std::path::PathBuf> {
    let mut config = whycodes_config::Config::load().map_err(map_msg)?;
    apply(&mut config, plan);
    config.save().map_err(map_msg)?;
    let path = whycodes_config::Config::default_path().map_err(map_msg)?;
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

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn isolated_home(f: impl FnOnce(&std::path::Path)) {
        let _guard = lock_env();
        isolated_home_locked(f);
    }

    fn isolated_home_locked(f: impl FnOnce(&std::path::Path)) {
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        f(dir.path());
        restore_home(prev);
    }

    fn restore_home(prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
        }
    }

    #[test]
    fn isolated_home_restores_previous_whycodes_home() {
        let _guard = lock_env();
        unsafe { std::env::set_var("WHYCODES_HOME", "/tmp/whycodes-import-prev-home") };
        isolated_home_locked(|_| {
            assert_ne!(
                std::env::var_os("WHYCODES_HOME").as_deref(),
                Some(std::ffi::OsStr::new("/tmp/whycodes-import-prev-home"))
            );
        });
        assert_eq!(
            std::env::var_os("WHYCODES_HOME").as_deref(),
            Some(std::ffi::OsStr::new("/tmp/whycodes-import-prev-home"))
        );
        restore_home(Some("/tmp/whycodes-import-prev-home".into()));
        restore_home(None);
    }

    #[test]
    fn lock_env_recovers_from_poison() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = ENV_LOCK.lock().unwrap();
            panic!("poison");
        }));
        let _g = lock_env();
    }

    fn hook(event: HookEvent, tool: &str, command: &str) -> HookConfig {
        HookConfig {
            event,
            tool_match: tool.into(),
            command: command.into(),
            block_on_failure: event == HookEvent::PreTool,
            timeout_secs: 30,
        }
    }

    #[test]
    fn queued_duplicates_and_force_overwrite() {
        let cfg = Config::default();
        let a = Extracted {
            product: Product::Claude,
            path: PathBuf::from("/a"),
            mcp: vec![mcp("fs")],
            permission: {
                let mut m = std::collections::HashMap::new();
                m.insert("bash".into(), PermissionAction::Ask);
                m
            },
            hooks: vec![hook(HookEvent::PreTool, "bash", "echo hi")],
            skipped: vec![],
        };
        let b = Extracted {
            product: Product::Codex,
            path: PathBuf::from("/b"),
            mcp: vec![mcp("fs")],
            permission: {
                let mut m = std::collections::HashMap::new();
                m.insert("bash".into(), PermissionAction::Deny);
                m
            },
            hooks: vec![hook(HookEvent::PreTool, "bash", "echo hi")],
            skipped: vec!["partial skip".into()],
        };
        let p = plan(&cfg, &[a.clone(), b], false);
        assert_eq!(p.mcp_add.len(), 1);
        assert_eq!(p.mcp_skip.len(), 1);
        assert!(p.mcp_skip[0].1.contains("already queued"));
        assert_eq!(p.permission_add.len(), 1);
        assert_eq!(p.permission_skip.len(), 1);
        assert!(p.permission_skip[0].1.contains("already queued"));
        assert_eq!(p.hooks_add.len(), 1);
        assert_eq!(p.hooks_skip.len(), 1);
        assert!(p.hooks_skip[0].contains("pre_tool"));
        assert!(p.warnings.iter().any(|w| w.contains("partial skip")));

        let mut existing = Config::default();
        let (n, s) = mcp("fs");
        existing.mcp_servers.insert(n, s);
        existing
            .permission
            .insert("bash".into(), PermissionAction::Allow);
        existing
            .hooks
            .push(hook(HookEvent::PostTool, "*", "echo after"));
        let later = Extracted {
            product: Product::OpenCode,
            path: PathBuf::from("/c"),
            mcp: vec![mcp("fs")],
            permission: {
                let mut m = std::collections::HashMap::new();
                m.insert("bash".into(), PermissionAction::Deny);
                m
            },
            hooks: vec![hook(HookEvent::PostTool, "*", "echo after")],
            skipped: vec![],
        };
        let skipped = plan(&existing, std::slice::from_ref(&later), false);
        assert!(skipped.mcp_add.is_empty());
        assert!(skipped.permission_add.is_empty());
        assert!(skipped.hooks_add.is_empty());
        assert!(skipped.hooks_skip[0].contains("post_tool"));
        let forced = plan(&existing, &[later], true);
        assert_eq!(forced.mcp_add.len(), 1);
        assert_eq!(forced.permission_add.len(), 1);
        assert!(forced.hooks_add.is_empty());
    }

    #[test]
    fn apply_and_save_writes_isolated_home() {
        isolated_home(|home| {
            let mut p = ImportPlan::default();
            p.mcp_add.push(mcp("git"));
            p.permission_add
                .push(("read".into(), PermissionAction::Allow));
            p.hooks_add
                .push(hook(HookEvent::PostTool, "*", "echo after"));
            let path = apply_and_save(&p).unwrap();
            assert_eq!(path, home.join("config.toml"));
            assert!(path.exists());
            let loaded = Config::load().unwrap();
            assert!(loaded.mcp_servers.contains_key("git"));
            assert_eq!(
                loaded.permission.get("read"),
                Some(&PermissionAction::Allow)
            );
            assert_eq!(loaded.hooks.len(), 1);
        });
    }

    #[test]
    fn apply_and_save_maps_load_error() {
        isolated_home(|home| {
            std::fs::write(home.join("config.toml"), "[[[").unwrap();
            let err = apply_and_save(&ImportPlan::default()).unwrap_err();
            assert!(err.to_string().contains("TOML") || err.to_string().contains("parse"));
        });
    }

    #[test]
    fn apply_and_save_maps_save_error() {
        isolated_home(|home| {
            std::fs::create_dir(home.join("config.toml")).unwrap();
            let mut p = ImportPlan::default();
            p.mcp_add.push(mcp("git"));
            let err = apply_and_save(&p).unwrap_err();
            assert!(!err.to_string().is_empty());
        });
    }

    #[test]
    fn map_msg_wraps_display() {
        let err = map_msg("boom");
        assert!(err.to_string().contains("boom"));
        let err = map_msg(std::io::Error::other("io"));
        assert!(err.to_string().contains("io"));
    }
}
