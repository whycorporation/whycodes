use whycode_core::types::ToolDefinition;

/// Validate tool call parameters against a tool's JSON Schema definition
pub fn validate_tool_params(
    tool: &ToolDefinition,
    params: &serde_json::Value,
) -> Result<(), String> {
    if let Some(required) = tool.parameters.get("required").and_then(|r| r.as_array()) {
        for field in required {
            if let Some(field_name) = field.as_str()
                && params.get(field_name).is_none()
            {
                return Err(format!("Missing required parameter: {}", field_name));
            }
        }
    }

    if let Some(properties) = tool
        .parameters
        .get("properties")
        .and_then(|p| p.as_object())
    {
        for (key, prop) in properties {
            if let Some(value) = params.get(key)
                && let Some(expected_type) = prop.get("type").and_then(|t| t.as_str())
            {
                let mismatch = match expected_type {
                    "string" => !value.is_string(),
                    "integer" | "number" => !value.is_number(),
                    "boolean" => !value.is_boolean(),
                    "array" => !value.is_array(),
                    "object" => !value.is_object(),
                    _ => false,
                };
                if mismatch {
                    return Err(format!("{}: expected {}", key, expected_type));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool(required: Vec<&str>, props: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: "test".to_string(),
            description: "test tool".to_string(),
            parameters: json!({"type": "object", "required": required, "properties": props}),
        }
    }

    #[test]
    fn test_missing_required() {
        let tool = make_tool(vec!["name"], json!({"name": {"type": "string"}}));
        assert!(validate_tool_params(&tool, &json!({})).is_err());
    }

    #[test]
    fn test_all_required_present() {
        let tool = make_tool(vec!["path"], json!({"path": {"type": "string"}}));
        assert!(validate_tool_params(&tool, &json!({"path": "/tmp"})).is_ok());
    }

    #[test]
    fn test_type_mismatch() {
        let tool = make_tool(vec![], json!({"count": {"type": "integer"}}));
        assert!(validate_tool_params(&tool, &json!({"count": "nope"})).is_err());
    }

    #[test]
    fn test_type_match() {
        let tool = make_tool(
            vec![],
            json!({"count": {"type": "integer"}, "name": {"type": "string"}}),
        );
        assert!(validate_tool_params(&tool, &json!({"count": 42, "name": "ok"})).is_ok());
    }

    #[test]
    fn test_extra_fields_allowed() {
        let tool = make_tool(vec![], json!({"a": {"type": "string"}}));
        assert!(validate_tool_params(&tool, &json!({"a": "ok", "b": "extra"})).is_ok());
    }

    #[test]
    fn no_schema_constraints_accepts_anything() {
        let tool = ToolDefinition {
            name: "bare".into(),
            description: "no schema".into(),
            parameters: json!({}),
        };
        assert!(validate_tool_params(&tool, &json!({"x": 1})).is_ok());
    }

    #[test]
    fn required_non_string_entries_are_ignored() {
        let tool = ToolDefinition {
            name: "t".into(),
            description: "t".into(),
            parameters: json!({
                "required": [1, "name"],
                "properties": {"name": {"type": "string"}}
            }),
        };
        assert!(validate_tool_params(&tool, &json!({})).is_err());
        assert!(validate_tool_params(&tool, &json!({"name": "ok"})).is_ok());
    }

    #[test]
    fn property_without_type_is_not_checked() {
        let tool = make_tool(vec![], json!({"flag": {}}));
        assert!(validate_tool_params(&tool, &json!({"flag": "anything"})).is_ok());
    }

    #[test]
    fn unknown_json_type_does_not_mismatch() {
        let tool = make_tool(vec![], json!({"x": {"type": "null"}}));
        assert!(validate_tool_params(&tool, &json!({"x": 1})).is_ok());
    }

    #[test]
    fn boolean_array_object_and_number_types() {
        let tool = make_tool(
            vec![],
            json!({
                "ok": {"type": "boolean"},
                "items": {"type": "array"},
                "meta": {"type": "object"},
                "n": {"type": "number"}
            }),
        );
        assert!(
            validate_tool_params(
                &tool,
                &json!({"ok": true, "items": [1], "meta": {"a": 1}, "n": 1.5})
            )
            .is_ok()
        );
        assert!(validate_tool_params(&tool, &json!({"ok": "no"})).is_err());
        assert!(validate_tool_params(&tool, &json!({"items": {}})).is_err());
        assert!(validate_tool_params(&tool, &json!({"meta": []})).is_err());
        assert!(validate_tool_params(&tool, &json!({"n": "1"})).is_err());
    }

    #[test]
    fn missing_property_value_skips_type_check() {
        let tool = make_tool(
            vec![],
            json!({"a": {"type": "string"}, "b": {"type": "boolean"}}),
        );
        assert!(validate_tool_params(&tool, &json!({"a": "only-a"})).is_ok());
    }
}
