use super::*;
use serde_json::json;
use whycodes_core::types::{PermissionSet, ToolCall};

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "1".into(),
        name: name.into(),
        arguments: args,
    }
}

#[test]
fn permission_detail_and_parallel_policy() {
    assert_eq!(format_permission_detail(&json!({})), "(no arguments)");
    assert_eq!(format_permission_detail(&json!({"command": "ls"})), "ls");
    let multi = format_permission_detail(&json!({"path": "a.rs", "limit": 1, "ok": true}));
    assert!(multi.contains("path:"));
    assert!(multi.contains("limit:"));
    let long_str = "x".repeat(80);
    let nested = format_permission_detail(&json!({"note": long_str, "meta": {"k": 1}}));
    assert!(nested.contains("note:"));
    assert!(format_permission_detail(&json!(["a", "b"])).contains("a"));
    let huge = "a".repeat(PERMISSION_DETAIL_MAX + 10);
    assert!(truncate_permission_detail(&huge).ends_with("…"));
    assert_eq!(truncate_permission_detail("short"), "short");
    let mixed = format_permission_detail(&json!({"n": 1, "ok": false, "z": null, "obj": {"a": 1}}));
    assert!(mixed.contains("n:"));
    assert!(mixed.contains("ok:"));
    assert!(mixed.contains("z:"));
    assert!(format_shell_risk_detail("rm -rf /", "destructive").contains("Risk:"));
    let multiline = format_permission_detail(&json!({"note": "line1\nline2"}));
    assert!(multiline.contains("note:"));

    assert!(is_safe_worktree_name("w0"));
    assert!(is_safe_worktree_name("ab-c_1"));
    assert!(!is_safe_worktree_name(""));
    assert!(!is_safe_worktree_name("../x"));
    assert!(!is_safe_worktree_name(&"x".repeat(65)));

    let cwd = std::env::current_dir().unwrap();
    let cwd_s = cwd.to_string_lossy();
    assert!(path_outside_workspace("../secret", &cwd_s));
    assert!(path_outside_workspace("~/.ssh", &cwd_s));
    assert!(!path_outside_workspace("src/lib.rs", &cwd_s));
    assert!(path_outside_workspace("/tmp/outside", &cwd_s));

    assert_eq!(
        file_tool_path(&call("read", json!({"path": "a.rs"}))).as_deref(),
        Some("a.rs")
    );
    assert_eq!(
        file_tool_path(&call("repomap", json!({"path": "src"}))).as_deref(),
        Some("src")
    );
    assert!(file_tool_path(&call("bash", json!({"command": "ls"}))).is_none());

    let perms = PermissionSet {
        allowed_tools: None,
        denied_tools: None,
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    };
    assert!(is_parallel_safe_tool("read", &perms));
    assert!(is_parallel_safe_tool("grep", &perms));
    assert!(is_parallel_safe_tool("glob", &perms));
    assert!(is_parallel_safe_tool("repomap", &perms));
    assert!(!is_parallel_safe_tool("bash", &perms));
    assert!(!is_parallel_safe_tool("write", &perms));
    assert!(!is_parallel_safe_tool("edit", &perms));
    assert!(!is_parallel_safe_tool("shell", &perms));
    assert!(!is_parallel_safe_tool("question", &perms));
    let deny_shell = PermissionSet {
        allowed_tools: None,
        denied_tools: None,
        allow_file_writes: true,
        allow_network: true,
        allow_shell: false,
        allowed_paths: None,
        rules: Default::default(),
    };
    assert!(!is_parallel_safe_tool("bash", &deny_shell));
}

#[test]
fn pretty_one_liner_and_absolute_path_inside_workspace() {
    let compact_arr = format_permission_detail(&json!({"items": [1, 2]}));
    assert!(compact_arr.contains("items:"), "{compact_arr}");
    let empty_arr = format_permission_detail(&json!({"items": []}));
    assert!(
        empty_arr.contains("items: []") || empty_arr.contains("items:[]"),
        "{empty_arr}"
    );
    let cwd = std::env::current_dir().unwrap();
    let cwd_s = cwd.to_string_lossy();
    let inside = cwd.join("Cargo.toml");
    if inside.is_file() {
        assert!(
            !path_outside_workspace(&inside.to_string_lossy(), &cwd_s),
            "canonicalized workspace file must be inside"
        );
    }
    assert!(file_tool_path(&call("grep", json!({"path": "src"}))).is_some());
    assert!(file_tool_path(&call("apply_patch", json!({"path": "a.rs"}))).is_some());
    assert!(file_tool_path(&call("write", json!({"path": ""}))).is_none());
    assert!(SHELL_TOOLS.contains(&"bash"));
    let one_liner = format_permission_detail(&json!({"meta": true}));
    assert!(one_liner.contains("meta:"), "{one_liner}");
    let bool_nested = format_permission_detail(&json!({"flag": serde_json::Value::Bool(true)}));
    assert!(bool_nested.contains("flag:"), "{bool_nested}");
}

#[test]
fn pretty_or_display_falls_back_when_pretty_errors() {
    assert!(pretty_or_display(&json!({"a": 1})).contains("a"));
    assert!(pretty_or_display(&json!([1, 2])).contains("1"));
    let err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert_eq!(pretty_json_or(Err(err), &json!(true)), "true");
    assert_eq!(pretty_json_or(Ok("ok".into()), &json!(true)), "ok");
}
