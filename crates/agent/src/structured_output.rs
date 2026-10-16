//! Typed subagent results.
//!
//! A `task` call's `output_schema` (or a custom agent's `output` key) asks the
//! worker to end with one fenced `json` block. This module prepares the
//! schema, writes the system-prompt instruction, pulls the last block out of
//! the reply, and checks it against a JSON Schema subset: `type` (object,
//! array, string, number, integer, boolean, null; a string or a list),
//! `properties`, `required`, `items` and `enum`. Other keywords are ignored.

use serde_json::{Value, json};

/// Field every structured result carries: what the worker did not verify.
pub const UNCHECKED_FIELD: &str = "what_i_did_not_check";

/// A schema ready for a worker: always an object schema that requires
/// [`UNCHECKED_FIELD`]. Errors when `schema` is not a JSON object or
/// describes something other than an object.
pub fn prepare(schema: &Value) -> Result<Value, String> {
    let Some(map) = schema.as_object() else {
        return Err("output schema must be a JSON object".into());
    };
    let mut map = map.clone();
    match map.get("type") {
        None => {
            map.insert("type".into(), json!("object"));
        }
        Some(t) if t == "object" => {}
        Some(t) => return Err(format!("output schema must have type object, not {t}")),
    }
    let props = map
        .entry("properties")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("output schema `properties` must be an object")?;
    props
        .entry(UNCHECKED_FIELD)
        .or_insert_with(|| json!({"type": "array", "items": {"type": "string"}}));
    let required = map
        .entry("required")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("output schema `required` must be an array")?;
    if !required.iter().any(|r| r == UNCHECKED_FIELD) {
        required.push(json!(UNCHECKED_FIELD));
    }
    Ok(Value::Object(map))
}

/// The `task` tool's `output_schema` argument, prepared. A JSON string is
/// parsed first; absent or null means no schema.
pub fn from_arg(arg: Option<&Value>) -> Result<Option<Value>, String> {
    let parsed: Value;
    let schema = match arg {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(raw)) => {
            parsed = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
            &parsed
        }
        Some(other) => other,
    };
    prepare(schema).map(Some)
}

/// System-prompt section asking for the final `json` block.
pub fn instruction(schema: &Value) -> String {
    format!(
        "\n\n# Structured result\n\nEnd your final message with exactly one fenced ```json block \
         whose value matches the JSON Schema below. Put any prose before the block. \
         `{UNCHECKED_FIELD}` lists what you did not verify (an empty array when nothing).\n\n\
         ```json\n{}\n```\n",
        pretty(schema)
    )
}

/// User message for the one retry after an invalid or missing block.
pub fn retry_message(errors: &[String]) -> String {
    format!(
        "Your structured result is not valid:\n- {}\n\nReply with only the corrected fenced \
         ```json block that matches the schema.",
        errors.join("\n- ")
    )
}

/// Byte range of the last fenced `json` block's body in `text`.
fn last_block(text: &str) -> Option<(usize, usize, usize)> {
    let open = text.rfind("```json")?;
    let body = open + text[open..].find('\n')? + 1;
    let close = body + text[body..].find("```")?;
    Some((open, body, close))
}

/// Body of the last fenced `json` block, trimmed.
pub fn extract(text: &str) -> Option<&str> {
    last_block(text).map(|(_, body, close)| text[body..close].trim())
}

/// `text` without its last fenced `json` block, trimmed.
pub fn strip_block(text: &str) -> String {
    match last_block(text) {
        Some((open, _, close)) => format!("{}{}", &text[..open], &text[close + 3..])
            .trim()
            .to_string(),
        None => text.trim().to_string(),
    }
}

/// The validated value of the last `json` block in `text`, or the reasons
/// it is missing or invalid.
pub fn check(schema: &Value, text: &str) -> Result<Value, Vec<String>> {
    let Some(raw) = extract(text) else {
        return Err(vec!["no fenced ```json block found".into()]);
    };
    let value: Value = serde_json::from_str(raw).map_err(|e| vec![format!("invalid JSON: {e}")])?;
    let errors = validate(schema, &value);
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(errors)
    }
}

/// Every way `value` breaks `schema`, each prefixed with its JSON pointer.
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    walk(schema, value, "", &mut errors);
    errors
}

fn walk(schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
    let at = if path.is_empty() { "/" } else { path };
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        if !types.iter().any(|ty| has_type(ty, value)) {
            errors.push(format!("{at}: expected {t}, got {}", kind(value)));
            return;
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(value)
    {
        errors.push(format!(
            "{at}: must be one of {}",
            Value::Array(options.clone())
        ));
    }
    if let Value::Object(map) = value {
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !map.contains_key(name) {
                errors.push(format!("{at}: missing required field `{name}`"));
            }
        }
        if let Some(Value::Object(props)) = schema.get("properties") {
            for (name, sub) in props {
                if let Some(v) = map.get(name) {
                    walk(sub, v, &format!("{path}/{}", escape(name)), errors);
                }
            }
        }
    }
    if let (Value::Array(items), Some(sub)) = (value, schema.get("items")) {
        for (i, v) in items.iter().enumerate() {
            walk(sub, v, &format!("{path}/{i}"), errors);
        }
    }
}

fn has_type(ty: &str, value: &Value) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// JSON pointer escaping for one segment.
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// What a worker's structured result came to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Structured {
    /// The validated value; `None` when it stayed invalid after the retry.
    pub value: Option<Value>,
    /// Validation errors of the last attempt (empty when valid).
    pub errors: Vec<String>,
}

impl Structured {
    /// Text appended to a worker report for the parent.
    pub fn note(&self, id: &str) -> String {
        match &self.value {
            Some(v) => format!("\n\nStructured result (`read agent://{id}/<json-pointer>`):\n{v}"),
            None => format!(
                "\n\nstructured: null\nStructured result errors:\n- {}",
                self.errors.join("\n- ")
            ),
        }
    }
}

#[cfg(test)]
#[path = "structured_output_tests.rs"]
mod tests;
