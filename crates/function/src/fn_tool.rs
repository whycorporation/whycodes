use std::sync::Arc;
use whycode_core::types::ToolResult;

/// A tool defined by a closure/function
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

/// Macro to define a tool inline
#[macro_export]
macro_rules! define_tool {
    ($name:expr, $desc:expr, $params:expr, |$args:ident| $body:expr) => {
        $crate::fn_tool::FnTool::new($name, $desc, $params, move |$args| $body)
    };
}
