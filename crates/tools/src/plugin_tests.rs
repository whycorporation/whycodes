use super::*;
use crate::executor::ToolExecutor;

#[test]
fn register_discovers_project_plugin_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".whycodes").join("plugins").join("echo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"whycodes_test_echojson","command":"echo from-json","description":"json plugin"}"#,
        )
        .unwrap();

    let mut exec = ToolExecutor::new();
    let n = exec.register_config_plugins(Some(tmp.path()));
    assert!(n >= 1, "registered {n}");
    assert!(
        exec.get("plugin_whycodes_test_echojson").is_some(),
        "tools: {:?}",
        exec.tool_names()
    );
}

#[test]
fn json_overrides_toml_same_name() {
    let tmp = tempfile::tempdir().unwrap();
    let why = tmp.path().join(".whycodes");
    std::fs::create_dir_all(why.join("plugins").join("hello")).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"
[[plugins]]
name = "whycodes_test_hello"
command = "echo toml"
description = "from toml"
"#,
    )
    .unwrap();
    std::fs::write(
        why.join("plugins").join("hello").join("plugin.json"),
        r#"{"name":"whycodes_test_hello","command":"echo json"}"#,
    )
    .unwrap();

    let listed = list_shell_plugins(Some(tmp.path()));
    let hello = listed
        .iter()
        .find(|p| p.tool_name == "plugin_whycodes_test_hello");
    assert!(hello.is_some(), "{listed:?}");
    assert_eq!(hello.unwrap().command, "echo json");
}

#[tokio::test]
async fn plugin_shell_tool_execute_and_list_skips() {
    let cfg = PluginConfig {
        name: "echo".into(),
        command: "echo $PLUGIN_ARG_INPUT $PLUGIN_WORKSPACE".into(),
        description: "d".into(),
        parameters: None,
        working_dir: None,
    };
    let tool = PluginShellTool::from_config(cfg);
    assert_eq!(tool.name(), "plugin_echo");
    assert_eq!(tool.description(), "d");
    let params = tool.parameters();
    assert_eq!(params["properties"]["input"]["type"], "string");
    let out = tool
        .execute(
            serde_json::json!({"input": "hi", "n": 1}),
            &crate::tool::ToolContext::new("/tmp"),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("hi"), "{}", out.content);

    let schema = serde_json::json!({"type": "object"});
    let cfg = PluginConfig {
        name: "typed".into(),
        command: "true".into(),
        description: "t".into(),
        parameters: Some(schema.clone()),
        working_dir: Some("/tmp".into()),
    };
    let tool = PluginShellTool::from_config(cfg);
    assert_eq!(tool.parameters(), schema);

    let tmp = tempfile::tempdir().unwrap();
    let why = tmp.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"
[[plugins]]
name = ""
command = "echo"
[[plugins]]
name = "ok"
command = ""
"#,
    )
    .unwrap();
    let listed = list_shell_plugins(Some(tmp.path()));
    assert!(listed.iter().all(|p| p.tool_name != "plugin_"));

    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let _ = list_shell_plugins(None);
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
}

#[tokio::test]
async fn execute_non_object_args_and_empty_json_name() {
    let cfg = PluginConfig {
        name: "echo".into(),
        command: "echo ok".into(),
        description: "d".into(),
        parameters: None,
        working_dir: None,
    };
    let tool = PluginShellTool::from_config(cfg);
    let out = tool
        .execute(
            serde_json::json!("not-an-object"),
            &crate::tool::ToolContext::new("/tmp"),
        )
        .await;
    assert!(!out.is_error || !out.content.is_empty(), "{}", out.content);

    let tmp = tempfile::tempdir().unwrap();
    let why = tmp.path().join(".whycodes");
    std::fs::create_dir_all(why.join("plugins").join("blank")).unwrap();
    std::fs::write(
        why.join("plugins").join("blank").join("plugin.json"),
        r#"{"name":"","command":"echo"}"#,
    )
    .unwrap();
    let listed = list_shell_plugins(Some(tmp.path()));
    assert!(listed.iter().all(|p| p.tool_name != "plugin_"));
}
