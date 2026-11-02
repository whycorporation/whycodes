use super::*;

fn value(text: &str) -> Value {
    toml::from_str(text).unwrap()
}

#[test]
fn only_changed_keys_are_written_and_the_rest_of_the_file_stays() {
    let text = "\
# my settings
default_agent = \"build\"   # keep this comment

[mcp_servers.git]
command = \"npx\"
args = []

[mcp_servers.docs]  # docs server
command = \"node\"
";
    let before = value(text);
    let mut after = before.clone();
    after["mcp_servers"]["git"]["command"] = Value::String("uvx".into());
    after["mcp_servers"]["git"]["args"] =
        Value::Array(vec![Value::String("mcp-server-git".into())]);
    let out = patched_text(text, &before, &after).unwrap();
    assert_eq!(
        out,
        "\
# my settings
default_agent = \"build\"   # keep this comment

[mcp_servers.git]
command = \"uvx\"
args = [\"mcp-server-git\"]

[mcp_servers.docs]  # docs server
command = \"node\"
"
    );
}

#[test]
fn defaults_the_file_never_had_stay_out() {
    let text = "default_agent = \"build\"\n";
    // `before` / `after` are whole serialized configs: both carry defaults.
    let before = value("default_agent = \"build\"\n[session]\nundo_depth = 50\n");
    let mut after = before.clone();
    after["default_agent"] = Value::String("plan".into());
    let out = patched_text(text, &before, &after).unwrap();
    assert_eq!(out, "default_agent = \"plan\"\n");
}

#[test]
fn added_and_removed_entries() {
    let text = "[mcp_servers.old]\ncommand = \"node\"\n";
    let before = value(text);
    let after = value("[mcp_servers.new]\ncommand = \"uvx\"\nargs = [\"x\"]\n");
    let out = patched_text(text, &before, &after).unwrap();
    assert!(!out.contains("old"), "{out}");
    assert!(out.contains("[mcp_servers.new]"), "{out}");
    assert_eq!(value(&out), after);
}

#[test]
fn an_inline_table_is_patched_in_place_and_bad_text_is_an_error() {
    let text = "general = { auto_update = true, theme = \"dark\" }\n";
    let before = value(text);
    let mut after = before.clone();
    after["general"]["auto_update"] = Value::Boolean(false);
    let out = patched_text(text, &before, &after).unwrap();
    assert_eq!(out, "general = { auto_update = false, theme = \"dark\" }\n");

    assert!(patched_text("[[[", &before, &after).is_err());
    // Not tables at the top: nothing to patch.
    let same = patched_text(text, &Value::Integer(1), &Value::Integer(2)).unwrap();
    assert_eq!(same, text);
}
