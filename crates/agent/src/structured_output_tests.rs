use super::*;

fn findings_schema() -> Value {
    prepare(&json!({
        "type": "object",
        "properties": {
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "line": {"type": "integer"},
                        "severity": {"enum": ["low", "high"]}
                    },
                    "required": ["path"]
                }
            },
            "score": {"type": "number"},
            "ok": {"type": "boolean"},
            "note": {"type": ["string", "null"]}
        },
        "required": ["findings"]
    }))
    .unwrap()
}

#[test]
fn prepare_adds_the_unchecked_field_once() {
    let s = prepare(&json!({})).unwrap();
    assert_eq!(s["type"], "object");
    assert_eq!(s["required"], json!([UNCHECKED_FIELD]));
    assert_eq!(
        s["properties"][UNCHECKED_FIELD],
        json!({"type": "array", "items": {"type": "string"}})
    );
    let again = prepare(&s).unwrap();
    assert_eq!(again, s);

    let custom = prepare(&json!({
        "type": "object",
        "properties": {UNCHECKED_FIELD: {"type": "array"}},
        "required": ["a"]
    }))
    .unwrap();
    assert_eq!(
        custom["properties"][UNCHECKED_FIELD],
        json!({"type": "array"})
    );
    assert_eq!(custom["required"], json!(["a", UNCHECKED_FIELD]));
}

#[test]
fn prepare_rejects_non_object_schemas() {
    assert!(prepare(&json!("x")).unwrap_err().contains("JSON object"));
    assert!(
        prepare(&json!({"type": "array"}))
            .unwrap_err()
            .contains("type object")
    );
    assert!(
        prepare(&json!({"properties": []}))
            .unwrap_err()
            .contains("properties")
    );
    assert!(
        prepare(&json!({"required": "a"}))
            .unwrap_err()
            .contains("required")
    );
}

#[test]
fn from_arg_accepts_objects_and_json_strings() {
    assert_eq!(from_arg(None), Ok(None));
    assert_eq!(from_arg(Some(&Value::Null)), Ok(None));
    let obj = from_arg(Some(&json!({"type": "object"}))).unwrap().unwrap();
    assert_eq!(obj["required"], json!([UNCHECKED_FIELD]));
    let text = from_arg(Some(&json!("{\"properties\": {}}")))
        .unwrap()
        .unwrap();
    assert_eq!(text, obj);
    assert!(
        from_arg(Some(&json!("{nope")))
            .unwrap_err()
            .starts_with("not JSON")
    );
    assert!(from_arg(Some(&json!(3))).is_err());
}

#[test]
fn instruction_and_retry_message_carry_schema_and_errors() {
    let s = prepare(&json!({})).unwrap();
    let text = instruction(&s);
    assert!(text.contains("# Structured result"), "{text}");
    assert!(text.contains("```json\n{"), "{text}");
    assert!(text.contains(UNCHECKED_FIELD), "{text}");
    let retry = retry_message(&["/: a".into(), "/b: c".into()]);
    assert!(retry.contains("- /: a\n- /b: c"), "{retry}");
}

#[test]
fn extract_takes_the_last_json_block() {
    let text = "intro\n```json\n{\"a\": 1}\n```\nmiddle\n```json\n{\"a\": 2}\n```\n";
    assert_eq!(extract(text), Some("{\"a\": 2}"));
    assert_eq!(strip_block(text), "intro\n```json\n{\"a\": 1}\n```\nmiddle");
    assert_eq!(extract("no block"), None);
    assert_eq!(extract("```json"), None, "no newline after the fence");
    assert_eq!(extract("```json\n{\"open\": true}"), None, "unclosed");
    assert_eq!(strip_block("  plain  "), "plain");
}

#[test]
fn check_reports_missing_unparsable_and_invalid_blocks() {
    let s = findings_schema();
    assert_eq!(
        check(&s, "nothing here"),
        Err(vec!["no fenced ```json block found".to_string()])
    );
    let bad = check(&s, "```json\n{oops\n```").unwrap_err();
    assert!(bad[0].starts_with("invalid JSON"), "{bad:?}");
    let invalid = check(&s, "```json\n{\"findings\": []}\n```").unwrap_err();
    assert_eq!(
        invalid,
        vec![format!("/: missing required field `{UNCHECKED_FIELD}`")]
    );
    let ok = check(
        &s,
        "done\n```json\n{\"findings\": [], \"what_i_did_not_check\": []}\n```",
    )
    .unwrap();
    assert_eq!(ok["findings"], json!([]));
}

#[test]
fn validate_covers_types_enum_properties_and_items() {
    let s = findings_schema();
    let value = json!({
        "findings": [
            {"path": "a.rs", "line": 3, "severity": "high"},
            {"line": 2.5, "severity": "mid"},
            "x"
        ],
        "score": "high",
        "ok": 1,
        "note": null,
        "what_i_did_not_check": [1]
    });
    let mut errors = validate(&s, &value);
    errors.sort();
    assert_eq!(
        errors,
        vec![
            "/findings/1/line: expected \"integer\", got number",
            "/findings/1/severity: must be one of [\"low\",\"high\"]",
            "/findings/1: missing required field `path`",
            "/findings/2: expected \"object\", got string",
            "/ok: expected \"boolean\", got number",
            "/score: expected \"number\", got string",
            "/what_i_did_not_check/0: expected \"string\", got number",
        ]
    );
    // A float with no fraction counts as an integer; null and unknown types.
    assert!(validate(&json!({"type": "integer"}), &json!(4.0)).is_empty());
    assert!(validate(&json!({"type": "null"}), &json!(null)).is_empty());
    assert_eq!(
        validate(&json!({"type": "date"}), &json!({})),
        vec!["/: expected \"date\", got object".to_string()]
    );
    assert_eq!(
        validate(&json!({"type": ["array"]}), &json!(true)),
        vec!["/: expected [\"array\"], got boolean".to_string()]
    );
    assert_eq!(
        validate(&json!({"type": "string"}), &json!(null)),
        vec!["/: expected \"string\", got null".to_string()]
    );
    assert_eq!(
        validate(&json!({"type": "object"}), &json!([])),
        vec!["/: expected \"object\", got array".to_string()]
    );
    // Keys with `/` and `~` are escaped in the pointer.
    let escaped = json!({"properties": {"a/b~c": {"type": "string"}}});
    assert_eq!(
        validate(&escaped, &json!({"a/b~c": 1})),
        vec!["/a~1b~0c: expected \"string\", got number".to_string()]
    );
}

#[test]
fn note_shows_value_or_errors() {
    let ok = Structured {
        value: Some(json!({"a": 1})),
        errors: Vec::new(),
    };
    let note = ok.note("task-1");
    assert!(
        note.contains("`read agent://task-1/<json-pointer>`"),
        "{note}"
    );
    assert!(note.ends_with("{\"a\":1}"), "{note}");
    let bad = Structured {
        value: None,
        errors: vec!["/: x".into(), "/a: y".into()],
    };
    assert_eq!(
        bad.note("w"),
        "\n\nstructured: null\nStructured result errors:\n- /: x\n- /a: y"
    );
}
