pub mod client;
pub mod error;
pub mod http;
pub mod oauth;
pub mod server;
pub mod sse;
pub mod types;

pub use client::McpClient;
pub use error::{McpError, Result};
pub use server::run_stdio_server;
pub use types::{
    CallToolParams, CallToolResult, GetPromptResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse,
    McpPrompt, McpResource, McpResourceContent, McpTool, PromptMessage, ToolContent,
};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
