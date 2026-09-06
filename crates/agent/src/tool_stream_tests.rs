use super::*;
use serde_json::json;

#[test]
fn openai_style_stream_merges_index_deltas() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use(
        "call_abc".into(),
        "websearch".into(),
        Value::String(String::new()),
    );
    a.on_tool_use_delta("0", r#"{"query":"#);
    a.on_tool_use_delta("0", r#""nuxt latest"}"#);
    let calls = a.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "websearch");
    assert_eq!(calls[0].arguments["query"], "nuxt latest");
}

#[test]
fn first_chunk_may_include_full_json_string() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use(
        "c1".into(),
        "websearch".into(),
        Value::String(r#"{"query":"nuxt"}"#.into()),
    );
    let calls = a.finish();
    assert_eq!(calls[0].arguments["query"], "nuxt");
}

#[test]
fn anthropic_style_empty_id_deltas() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("tu_1".into(), "bash".into(), json!({}));
    a.on_tool_use_delta("", r#"{"command":"#);
    a.on_tool_use_delta("", r#""ls"}"#);
    let calls = a.finish();
    assert_eq!(calls[0].arguments["command"], "ls");
}

#[test]
fn parallel_tools_by_index() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("c0".into(), "a".into(), Value::Null);
    a.on_tool_use("c1".into(), "b".into(), Value::Null);
    a.on_tool_use_delta("1", r#"{"x":1}"#);
    a.on_tool_use_delta("0", r#"{"y":2}"#);
    let calls = a.finish();
    assert_eq!(calls[0].arguments["y"], 2);
    assert_eq!(calls[1].arguments["x"], 1);
}

#[test]
fn pre_parsed_object_kept_without_deltas() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("c".into(), "read".into(), json!({"path": "x.rs"}));
    let calls = a.finish();
    assert_eq!(calls[0].arguments["path"], "x.rs");
}

#[test]
fn empty_assembler_and_non_object_input() {
    let a = ToolCallAssembler::new();
    assert!(a.is_empty());
    assert!(a.pending_snapshots().is_empty());
    assert!(a.last_updated().is_none());

    let mut a = ToolCallAssembler::new();
    a.on_tool_use("n1".into(), "read".into(), json!(12));
    a.on_tool_use_delta("n1", "");
    a.on_tool_use_delta("missing", r#"{"path":"x"}"#);
    a.active = None;
    a.on_tool_use_delta("", r#"{"ignored":true}"#);
    a.on_tool_use_delta("0", r#"{"path":"x.rs"}"#);
    let snaps = a.pending_snapshots();
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].2.contains("path") || snaps[0].2.contains("12"));
    let last = a.last_updated().expect("active");
    assert_eq!(last.0, "n1");
    let calls = a.finish();
    assert_eq!(calls.len(), 1);
}

#[test]
fn numeric_alias_id_inserts_key_on_delta() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("n1".into(), "read".into(), json!({}));
    a.on_tool_use_delta("00", r#"{"path":"x.rs"}"#);
    let snaps = a.pending_snapshots();
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].2.contains("path"), "{snaps:?}");
    let calls = a.finish();
    assert_eq!(calls.len(), 1);
}

#[test]
fn pending_snapshots_use_preparsed_object() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("c".into(), "read".into(), json!({"path": "x.rs"}));
    let snaps = a.pending_snapshots();
    assert!(snaps[0].2.contains("x.rs"), "{snaps:?}");
    a.on_tool_use("c2".into(), "grep".into(), Value::Null);
    let snaps = a.pending_snapshots();
    assert_eq!(snaps.len(), 2);
    assert!(snaps[1].2.is_empty());
    let calls = a.finish();
    assert_eq!(calls[1].arguments, json!({}));
}

#[test]
fn named_id_delta_reuses_existing_key() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("call_abc".into(), "read".into(), json!({}));
    a.on_tool_use_delta("call_abc", r#"{"path":"x.rs"}"#);
    a.on_tool_use_delta("call_abc", r#""}"#);
    let snaps = a.pending_snapshots();
    assert_eq!(snaps[0].0, "call_abc");
    assert!(snaps[0].2.contains("path"), "{snaps:?}");
    let calls = a.finish();
    assert_eq!(calls.len(), 1);
}

#[test]
fn new_named_alias_id_inserts_when_active_is_set() {
    let mut a = ToolCallAssembler::new();
    a.on_tool_use("n1".into(), "read".into(), json!({}));
    a.on_tool_use_delta("alias-x", r#"{"path":"x.rs"}"#);
    let snaps = a.pending_snapshots();
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].2.contains("path"), "{snaps:?}");
    a.on_tool_use_delta("alias-x", r#","offset":1}"#);
    let calls = a.finish();
    assert_eq!(calls.len(), 1);
}
