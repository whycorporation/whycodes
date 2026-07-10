use whycode_core::types::ToolDefinition;

/// Validate tool call parameters against a tool's JSON Schema definition
pub fn validate_tool_params(tool: &ToolDefinition, params: &serde_json::Value) -> Result<(), String> {
    if let Some(required) = tool.parameters.get("required").and_then(|r| r.as_array()) {
        for field in required {
            if let Some(field_name) = field.as_str() {
                if params.get(field_name).is_none() {
                    return Err(format!("Missing required parameter: {}", field_name));
                }
            }
        }
    }

    if let Some(properties) = tool.parameters.get("properties").and_then(|p| p.as_object()) {
        for (key, prop) in properties {
            if let Some(value) = params.get(key) {
                if let Some(expected_type) = prop.get("type").and_then(|t| t.as_str()) {
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
        let tool = make_tool(vec![], json!({"count": {"type": "integer"}, "name": {"type": "string"}}));
        assert!(validate_tool_params(&tool, &json!({"count": 42, "name": "ok"})).is_ok());
    }

    #[test]
    fn test_extra_fields_allowed() {
        let tool = make_tool(vec![], json!({"a": {"type": "string"}}));
        assert!(validate_tool_params(&tool, &json!({"a": "ok", "b": "extra"})).is_ok());
    }
}
