pub mod client;
pub mod error;
pub mod http;
pub mod server;
pub mod sse;
pub mod types;

pub use client::McpClient;
pub use error::{McpError, Result};
pub use server::run_stdio_server;
pub use types::{
    CallToolParams, CallToolResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpPrompt,
    McpResource, McpTool, ToolContent,
};

#[cfg(test)]
mod tests {
    #[test]
    fn lib_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
