//! Coverage-friendly JSON object builders.
//!
//! `serde_json::json!` expands into many branches llvm-cov never marks as hit
//! even when the resulting value is used. These helpers keep the same `Value`
//! shape without the macro expansion cost.

use serde_json::{Map, Value};

pub(crate) fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(map_from(pairs))
}

pub(crate) fn str(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

pub(crate) fn bool(v: bool) -> Value {
    Value::Bool(v)
}

pub(crate) fn arr(items: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(items.into_iter().collect())
}

pub(crate) fn insert(target: &mut Value, key: &str, value: Value) {
    if let Value::Object(map) = target {
        map.insert(key.to_string(), value);
    }
}

pub(crate) fn map_from<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[cfg(test)]
#[path = "json_value_tests.rs"]
mod tests;
