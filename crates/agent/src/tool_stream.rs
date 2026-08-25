//! Assemble tool calls from streaming LLM events.
//!
//! OpenAI-compatible APIs send tool calls as:
//! 1. First chunk: `id` + `name` + empty/partial `arguments`
//! 2. Later chunks: `index` + argument JSON fragments
//!
//! Anthropic sends `ToolUse` then `ToolUseDelta` with empty id.
//! This assembler merges both styles into final `ToolCall` values with
//! parsed JSON object arguments.

use std::collections::HashMap;

use serde_json::Value;
use whycodes_core::types::ToolCall;
use whycodes_llm::openai_compat::parse_tool_arguments;

/// Builds [`ToolCall`]s from interleaved `ToolUse` / `ToolUseDelta` events.
#[derive(Debug, Default)]
pub struct ToolCallAssembler {
    calls: Vec<ToolCall>,
    arg_bufs: Vec<String>,
    /// Real call id or OpenAI index string → index into `calls`.
    keys: HashMap<String, usize>,
    active: Option<usize>,
}

impl ToolCallAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Handle a tool-call start (or a full non-streamed tool use).
    pub fn on_tool_use(&mut self, id: String, name: String, input: Value) {
        let idx = self.calls.len();
        let mut buf = String::new();
        let mut pre_parsed: Option<Value> = None;

        match &input {
            Value::String(s) => buf.push_str(s),
            Value::Object(m) if !m.is_empty() => {
                // Already a complete object (Anthropic sometimes / complete API).
                pre_parsed = Some(input);
            }
            Value::Null | Value::Object(_) => {}
            other => buf.push_str(&other.to_string()),
        }

        self.calls.push(ToolCall {
            id: id.clone(),
            name,
            arguments: pre_parsed.unwrap_or(Value::Null),
        });
        self.arg_bufs.push(buf);
        self.active = Some(idx);
        if !id.is_empty() {
            self.keys.insert(id, idx);
        }
        // Sequential OpenAI index: first call → "0", second → "1", …
        self.keys.insert(idx.to_string(), idx);
    }

    /// Append a JSON fragment to the matching tool call.
    ///
    /// Matching order:
    /// 1. Exact id or index key in `keys`
    /// 2. Numeric id as `calls` index
    /// 3. Last active tool (Anthropic deltas use empty id)
    pub fn on_tool_use_delta(&mut self, id: &str, fragment: &str) {
        if fragment.is_empty() {
            return;
        }

        let target = if !id.is_empty() {
            self.keys
                .get(id)
                .copied()
                .or_else(|| id.parse::<usize>().ok().filter(|&i| i < self.calls.len()))
        } else {
            None
        }
        .or(self.active);

        if let Some(i) = target {
            self.arg_bufs[i].push_str(fragment);
            self.active = Some(i);
            if !id.is_empty() {
                self.keys.entry(id.to_string()).or_insert(i);
            }
        }
    }

    /// Finalize: parse argument buffers into JSON objects.
    pub fn finish(self) -> Vec<ToolCall> {
        let mut calls = self.calls;
        for (i, tc) in calls.iter_mut().enumerate() {
            let buf = &self.arg_bufs[i];
            if !buf.is_empty() {
                tc.arguments = parse_tool_arguments(&Value::String(buf.clone()));
            } else if tc.arguments.is_null() {
                tc.arguments = Value::Object(Default::default());
            }
            // else keep pre-parsed object
        }
        calls
    }

    /// Snapshot of (id, name, raw arg buffer) for speculative early tool I/O.
    pub fn pending_snapshots(&self) -> Vec<(String, String, String)> {
        self.calls
            .iter()
            .enumerate()
            .map(|(i, tc)| {
                let buf = if !self.arg_bufs[i].is_empty() {
                    self.arg_bufs[i].clone()
                } else if !tc.arguments.is_null() {
                    // Complete object already present (Anthropic non-streamed).
                    serde_json::to_string(&tc.arguments).unwrap_or_default()
                } else {
                    String::new()
                };
                (tc.id.clone(), tc.name.clone(), buf)
            })
            .collect()
    }

    /// After a delta, return the updated (id, name, buf) for the active call.
    pub fn last_updated(&self) -> Option<(String, String, String)> {
        let i = self.active?;
        let tc = self.calls.get(i)?;
        let buf = if !self.arg_bufs[i].is_empty() {
            self.arg_bufs[i].clone()
        } else if !tc.arguments.is_null() {
            serde_json::to_string(&tc.arguments).unwrap_or_default()
        } else {
            String::new()
        };
        Some((tc.id.clone(), tc.name.clone(), buf))
    }
}

#[cfg(test)]
mod tests {
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
}
