use super::*;
use serde_json::json;
use whycodes_core::types::{PermissionAction, ToolResult};

/// Minimal fake tool for registration/definition tests.
struct FakeTool {
    name: String,
    desc: &'static str,
    allowed: bool,
}
impl Tool for FakeTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        self.desc
    }

    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            ToolResult {
                tool_call_id: "fake".into(),
                content: format!("fake-executed:{}", args),
                is_error: false,
            }
        })
    }

    fn is_allowed(&self, _permissions: &PermissionSet) -> bool {
        self.allowed
    }
}

fn fake(name: &str, allowed: bool) -> Box<dyn Tool> {
    Box::new(FakeTool {
        name: name.to_string(),
        desc: "a fake tool",
        allowed,
    })
}

#[test]
fn new_registers_builtin_tools() {
    let ex = ToolExecutor::new();
    let names = ex.tool_names();
    // Sorted.
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    // File + search core.
    for n in [
        "read",
        "write",
        "edit",
        "grep",
        "glob",
        "list",
        "repomap",
        "apply_patch",
        "bash",
        "shell",
        "todowrite",
        "todoread",
        "todo",
    ] {
        assert!(names.contains(&n.to_string()), "missing {n}");
    }
    // Web / github / lsp / plan-family.
    for n in [
        "webfetch",
        "browser",
        "github_issue",
        "github_pr",
        "lsp",
        "panel",
        "plan",
        "checkpoint",
        "rewind",
    ] {
        assert!(names.contains(&n.to_string()), "missing {n}");
    }
    // No duplicates from alias registration.
    assert_eq!(
        names.len(),
        names.iter().collect::<std::collections::HashSet<_>>().len()
    );
    assert!(
        names.len() >= 30,
        "expected >= 30 tools, got {}",
        names.len()
    );
}

#[test]
fn default_matches_new() {
    let a = ToolExecutor::new();
    let b = ToolExecutor::default();
    assert_eq!(a.tool_names(), b.tool_names());
    let _ = skipped_plugin_toml("boom", "plugins.toml load skipped");
    assert!(skip_empty_plugin_cfg("", "echo"));
    assert!(skip_empty_plugin_cfg("ok", " "));
    assert!(!skip_empty_plugin_cfg("ok", "echo"));
    let empty_cfg = whycodes_skill::PluginConfig {
        name: String::new(),
        command: "echo".into(),
        description: String::new(),
        parameters: None,
        working_dir: None,
    };
    assert!(keep_plugin_cfg(empty_cfg).is_none());
    let ok_cfg = whycodes_skill::PluginConfig {
        name: "ok".into(),
        command: "echo".into(),
        description: String::new(),
        parameters: None,
        working_dir: None,
    };
    assert!(keep_plugin_cfg(ok_cfg).is_some());
    assert!(
        keep_plugin_spec(
            String::new(),
            "echo".into(),
            String::new(),
            None,
            std::path::PathBuf::from("."),
        )
        .is_none()
    );
    assert!(
        keep_plugin_spec(
            "ok".into(),
            "echo".into(),
            String::new(),
            None,
            std::path::PathBuf::from("."),
        )
        .is_some()
    );
    let _ = load_plugin_toml(None);
}

#[test]
fn register_and_register_as() {
    let mut ex = ToolExecutor::new();
    ex.register(fake("fake_tool", true));
    assert!(ex.get("fake_tool").is_some());
    assert_eq!(ex.get("fake_tool").unwrap().description(), "a fake tool");

    // register_as overrides the tool's own name.
    ex.register_as("alias_name", fake("real_name", true));
    assert!(ex.get("alias_name").is_some());
    assert!(ex.get("real_name").is_none());
}

#[test]
fn configure_lsp_replaces_the_builtin_lsp_tool() {
    let mut ex = ToolExecutor::new();
    let overlay = whycodes_lsp::LspSettings {
        idle_timeout_ms: Some(1_000),
        servers: Default::default(),
    };
    ex.configure_lsp(&overlay);
    assert!(ex.get("lsp").is_some());
}

#[test]
fn get_unknown_returns_none() {
    let ex = ToolExecutor::new();
    assert!(ex.get("definitely_not_a_tool").is_none());
}

#[test]
fn definitions_sorted_by_default() {
    let ex = ToolExecutor::new();
    let defs = ex.get_definitions(&PermissionSet::default());
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert!(defs.iter().any(|d| d.name == "read"));
    assert!(defs.iter().all(|d| !d.description.is_empty()));
    assert!(defs.iter().all(|d| d.parameters["type"] == "object"));
    // Default PermissionSet denies shell/network/write categories.
    assert!(
        defs.iter()
            .all(|d| d.name != "bash" && d.name != "webfetch" && d.name != "write")
    );
}

#[test]
fn definitions_include_category_tools_when_permissive() {
    let ex = ToolExecutor::new();
    let perms = PermissionSet {
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        ..PermissionSet::default()
    };
    let defs = ex.get_definitions(&perms);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"bash"));
    assert!(names.contains(&"shell"));
    assert!(names.contains(&"write"));
    assert!(names.contains(&"webfetch"));
    // Browser resolves to Ask, which still keeps it in the schema.
    assert!(names.contains(&"browser"));
}

#[test]
fn definitions_filtered_by_permission_rules() {
    let ex = ToolExecutor::new();
    // Exact deny rule.
    let mut perms = PermissionSet::default();
    perms
        .rules
        .insert("webfetch".into(), PermissionAction::Deny);
    let defs = ex.get_definitions(&perms);
    assert!(defs.iter().all(|d| d.name != "webfetch"));
    assert!(defs.iter().any(|d| d.name == "read"));
    // Glob deny `github_*`.
    let mut perms = PermissionSet::default();
    perms
        .rules
        .insert("github_*".to_string(), PermissionAction::Deny);
    let defs = ex.get_definitions(&perms);
    assert!(defs.iter().all(|d| !d.name.starts_with("github_")));
    // Legacy allow list: only listed tools survive (writes need the flag).
    let perms = PermissionSet {
        allowed_tools: Some(vec!["read".into(), "write".into()]),
        allow_file_writes: true,
        ..PermissionSet::default()
    };
    let defs = ex.get_definitions(&perms);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["read", "write"]);
    // Category flag: no shell.
    let perms = PermissionSet {
        allow_shell: false,
        ..PermissionSet::default()
    };
    let defs = ex.get_definitions(&perms);
    assert!(defs.iter().all(|d| d.name != "bash" && d.name != "shell"));
}

#[test]
fn definitions_core_profile_limits_surface() {
    let ex = ToolExecutor::new();
    // Permissive perms so only the profile decides what shows up.
    let perms = PermissionSet {
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        ..PermissionSet::default()
    };
    let core = ex.get_definitions_profile(&perms, crate::profile::ToolProfile::Core);
    let full = ex.get_definitions_profile(&perms, crate::profile::ToolProfile::Full);
    let core_names: Vec<&str> = core.iter().map(|d| d.name.as_str()).collect();
    assert!(core_names.contains(&"read"));
    assert!(core_names.contains(&"bash"));
    assert!(!core_names.contains(&"webfetch"));
    assert!(!core_names.contains(&"browser"));
    assert!(!core_names.contains(&"lsp"));
    assert!(core.len() < full.len());
    assert!(full.iter().any(|d| d.name == "webfetch"));
    // Core keeps todo aliases under real names.
    assert!(core_names.contains(&"todowrite"));
    assert!(core_names.contains(&"todo"));
    assert!(!core_names.contains(&"todo_write"));
    assert!(!core_names.contains(&"apply_patch"));
    assert!(core_names.contains(&"bg"));
    assert!(!core_names.contains(&"shell"));
    assert!(!core_names.contains(&"memory"));
    assert!(!core_names.contains(&"swarm"));
}

#[test]
fn definitions_profile_extra_activates_deferred_tools() {
    let ex = ToolExecutor::new();
    let core =
        ex.get_definitions_profile(&PermissionSet::default(), crate::profile::ToolProfile::Core);
    assert!(core.iter().all(|d| d.name != "worktree"));
    let extra = ex.get_definitions_profile_extra(
        &PermissionSet::default(),
        crate::profile::ToolProfile::Core,
        &["worktree".to_string()],
    );
    assert!(extra.iter().any(|d| d.name == "worktree"));
    let dup = ex.get_definitions_profile_extra(
        &PermissionSet::default(),
        crate::profile::ToolProfile::Core,
        &["worktree".into(), "lsp".into(), "worktree".into()],
    );
    assert!(dup.iter().any(|d| d.name == "worktree"));
    assert!(dup.iter().any(|d| d.name == "lsp"));
    let again = ex.get_definitions_profile_extra(
        &PermissionSet::default(),
        crate::profile::ToolProfile::Core,
        &["lsp".into(), "worktree".into()],
    );
    assert!(Arc::ptr_eq(&dup, &again));
}

#[test]
fn definitions_cache_reuses_arc_until_register() {
    let mut ex = ToolExecutor::new();
    let perms = PermissionSet::default();
    let a = ex.get_definitions(&perms);
    let b = ex.get_definitions(&perms);
    assert!(
        Arc::ptr_eq(&a, &b),
        "same permissions should hit the schema cache"
    );
    let extra_a = ex.get_definitions_profile_extra(
        &perms,
        crate::profile::ToolProfile::Core,
        &["worktree".to_string()],
    );
    let extra_b = ex.get_definitions_profile_extra(
        &perms,
        crate::profile::ToolProfile::Core,
        &["worktree".to_string()],
    );
    assert!(Arc::ptr_eq(&extra_a, &extra_b));
    assert!(!Arc::ptr_eq(&a, &extra_a));
    ex.register(fake("ghost", true));
    let c = ex.get_definitions(&perms);
    assert!(
        !Arc::ptr_eq(&a, &c),
        "register must invalidate the schema cache"
    );
    assert!(c.iter().any(|d| d.name == "ghost"));
}

#[test]
fn deferred_catalog_lists_non_core_tools() {
    let ex = ToolExecutor::new();
    let perms = PermissionSet {
        allow_network: true,
        ..PermissionSet::default()
    };
    let cat = ex.deferred_catalog(&perms);
    let names: Vec<&str> = cat.iter().map(|(n, _)| n.as_str()).collect();
    // Sorted and deduped.
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert!(names.contains(&"webfetch"));
    assert!(names.contains(&"worktree"));
    assert!(!names.contains(&"read"));
    assert!(!names.contains(&"bash"));
    assert!(!names.contains(&"repomap"));
    // Every entry has a description.
    assert!(cat.iter().all(|(_, d)| !d.is_empty()));
    // Permission filtering applies (network off hides webfetch again).
    let cat = ex.deferred_catalog(&PermissionSet::default());
    assert!(cat.iter().all(|(n, _)| n != "webfetch"));
}

#[test]
fn is_allowed_used_for_definition_filtering() {
    // A tool whose is_allowed is false disappears even with default perms.
    let mut ex = ToolExecutor::new();
    ex.register(fake("ghost", false));
    let defs = ex.get_definitions(&PermissionSet::default());
    assert!(defs.iter().all(|d| d.name != "ghost"));
}

#[tokio::test]
async fn execute_unknown_tool_errors() {
    let ex = ToolExecutor::new();
    let call = ToolCall {
        id: "t1".into(),
        name: "no_such_tool".into(),
        arguments: json!({}),
    };
    let ctx = ToolContext::new("/tmp");
    let res = ex.execute(&call, &ctx, &PermissionSet::default()).await;
    assert!(res.is_error);
    assert_eq!(res.tool_call_id, "t1");
    assert!(res.content.contains("Unknown tool"));
    assert!(res.content.contains("Available tools"));
    assert!(res.content.contains("read"));
}

#[tokio::test]
async fn execute_denied_tool_errors() {
    let ex = ToolExecutor::new();
    let call = ToolCall {
        id: "t2".into(),
        name: "read".into(),
        arguments: json!({}),
    };
    let perms = PermissionSet {
        rules: [("read".to_string(), PermissionAction::Deny)]
            .into_iter()
            .collect(),
        ..PermissionSet::default()
    };
    let ctx = ToolContext::new("/tmp");
    let res = ex.execute(&call, &ctx, &perms).await;
    assert!(res.is_error);
    assert!(res.content.contains("not allowed"));
}

#[tokio::test]
async fn execute_runs_allowed_tool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "line one\nline two\n").unwrap();
    let ex = ToolExecutor::new();
    let call = ToolCall {
        id: "t3".into(),
        name: "read".into(),
        arguments: json!({ "path": "hello.txt" }),
    };
    let ctx = ToolContext::new(dir.path().display().to_string());
    let res = ex.execute(&call, &ctx, &PermissionSet::default()).await;
    assert!(!res.is_error, "{}", res.content);
    assert!(res.content.contains("line one"));
    assert!(res.content.contains("line two"));
}

#[tokio::test]
async fn execute_unknown_in_isolated_executor_lists_registered() {
    // A custom-registered tool must show up in the unknown-tool suggestion list.
    let mut ex = ToolExecutor::new();
    ex.register(fake("my_custom_tool", true));
    let call = ToolCall {
        id: "t4".into(),
        name: "my_custom_tool".into(),
        arguments: json!({ "x": 1 }),
    };
    let ctx = ToolContext::new("/tmp");
    let res = ex.execute(&call, &ctx, &PermissionSet::default()).await;
    assert!(!res.is_error);
    assert!(res.content.contains("fake-executed"));
}

#[test]
fn register_config_plugins_project_toml() {
    // Isolate global config so the developer machine's plugins.toml cannot leak in.
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let dir = tempfile::tempdir().unwrap();
    let why = dir.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"[[plugins]]
name = "shout"
command = "echo SHOUT"
description = "Shouts back"

[[plugins]]
name = "empty-cmd"
command = ""
description = "Has no command"
"#,
    )
    .unwrap();
    let mut ex = ToolExecutor::new();
    let n = ex.register_config_plugins(Some(dir.path()));
    assert!(n >= 1, "expected at least the shout plugin, got {n}");
    // PluginShellTool registers under a `plugin_` prefixed name.
    let shout = ex.get("plugin_shout");
    assert!(shout.is_some(), "shout plugin tool not registered");
    assert_eq!(shout.unwrap().description(), "Shouts back");
    // Empty-command plugins are skipped.
    assert!(ex.get("plugin_empty-cmd").is_none());
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
}

#[test]
fn register_config_plugins_skips_without_project_dir() {
    // No project dir: falls back to isolated (empty) global config only.
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let mut ex = ToolExecutor::new();
    let n = ex.register_config_plugins(None);
    assert_eq!(n, 0);
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
}

#[test]
fn register_config_plugins_invalid_toml_and_empty_json_name() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("plugins.toml"), "[[plugins]]\nname = [").unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };

    let dir = tempfile::tempdir().unwrap();
    let why = dir.path().join(".whycodes");
    std::fs::create_dir_all(why.join("plugins").join("blank")).unwrap();
    std::fs::write(why.join("plugins.toml"), "[[plugins]]\nname = [").unwrap();
    std::fs::write(
        why.join("plugins").join("blank").join("plugin.json"),
        r#"{"name":"","command":"echo"}"#,
    )
    .unwrap();
    let mut ex = ToolExecutor::new();
    let _ = ex.register_config_plugins(Some(dir.path()));
    let n = ex.register_config_plugins(None);
    assert_eq!(n, 0);
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
}
