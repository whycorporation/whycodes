use whycode_core::types::ToolDefinition;

/// Validate tool call parameters against a tool's JSON Schema definition
pub fn validate_tool_params(tool: &ToolDefinition, params: &serde_json::Value) -> Result<(), String> {
    // Basic validation: check that all required fields exist
    if let Some(required) = tool.parameters.get("required").and_then(|r| r.as_array()) {
        for field in required {
            if let Some(field_name) = field.as_str() {
                if params.get(field_name).is_none() {
                    return Err(format!("Missing required parameter: {}", field_name));
                }
            }
        }
    }

    // Type checking
    if let Some(properties) = tool.parameters.get("properties").and_then(|p| p.as_object()) {
        for (key, prop) in properties {
            if let Some(value) = params.get(key) {
                if let Some(expected_type) = prop.get("type").and_then(|t| t.as_str()) {
                    match expected_type {
                        "string" => if !value.is_string() { return Err(format!("{}: expected string", key)); }
                        "integer" | "number" => if !value.is_number() { return Err(format!("{}: expected number", key)); }
                        "boolean" => if !value.is_boolean() { return Err(format!("{}: expected boolean", key)); }
                        "array" => if !value.is_array() { return Err(format!("{}: expected array", key)); }
                        "object" => if !value.is_object() { return Err(format!("{}: expected object", key)); }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
