use std::sync::Arc;
use whycodes_core::types::ToolResult;

/// A tool defined by a closure/function.
pub struct FnTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub handler: Arc<dyn Fn(serde_json::Value) -> ToolResult + Send + Sync>,
}

impl FnTool {
    pub fn new<N, D, F>(name: N, description: D, parameters: serde_json::Value, handler: F) -> Self
    where
        N: Into<String>,
        D: Into<String>,
        F: Fn(serde_json::Value) -> ToolResult + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler: Arc::new(handler),
        }
    }

    pub fn execute(&self, args: serde_json::Value) -> ToolResult {
        (self.handler)(args)
    }
}

/// Macro to define a tool inline.
#[macro_export]
macro_rules! define_tool {
    ($name:expr, $desc:expr, $params:expr, |$args:ident| $body:expr) => {
        $crate::FnTool::new($name, $desc, $params, move |$args| $body)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use whycodes_core::types::ToolResult;

    fn ok(text: &str) -> ToolResult {
        ToolResult {
            tool_call_id: "fn".into(),
            content: text.into(),
            is_error: false,
        }
    }

    #[test]
    fn new_stores_name_description_and_parameters() {
        let params = json!({"type": "object"});
        let tool = FnTool::new("echo", "echoes args", params.clone(), |_| ok("x"));
        assert_eq!(tool.name, "echo");
        assert_eq!(tool.description, "echoes args");
        assert_eq!(tool.parameters, params);
        assert_eq!(tool.execute(json!({})).content, "x");
    }

    #[test]
    fn execute_forwards_args_to_handler() {
        let tool = FnTool::new("echo", "echo", json!({}), |args| {
            ok(args["msg"].as_str().unwrap_or(""))
        });
        let result = tool.execute(json!({"msg": "hi"}));
        assert!(!result.is_error);
        assert_eq!(result.content, "hi");
    }

    #[test]
    fn execute_propagates_error_result() {
        let tool = FnTool::new("fail", "fails", json!({}), |_| ToolResult {
            tool_call_id: "fn".into(),
            content: "nope".into(),
            is_error: true,
        });
        let result = tool.execute(json!({}));
        assert!(result.is_error);
        assert_eq!(result.content, "nope");
    }

    #[test]
    fn define_tool_macro_builds_and_runs() {
        let tool = define_tool!("sum", "add", json!({"type": "object"}), |args| {
            let a = args["a"].as_i64().unwrap_or(0);
            let b = args["b"].as_i64().unwrap_or(0);
            ok(&(a + b).to_string())
        });
        assert_eq!(tool.name, "sum");
        assert_eq!(tool.description, "add");
        let result = tool.execute(json!({"a": 2, "b": 3}));
        assert_eq!(result.content, "5");
    }

    #[test]
    fn handler_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let tool = FnTool::new("n", "d", json!({}), |_| ok("ok"));
        assert_send_sync(&tool);
        assert_eq!(tool.execute(json!({})).content, "ok");
    }
}
