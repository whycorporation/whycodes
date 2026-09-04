use serde::{Deserialize, Serialize};

// ── JSON-RPC 2.0 types ──

/// JSON-RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC response (success)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ── MCP request/response payloads ──

/// Parameters for the `initialize` request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub tools: Option<ClientCapabilityTools>,
    #[serde(default)]
    pub prompts: Option<ClientCapabilityPrompts>,
    #[serde(default)]
    pub resources: Option<ClientCapabilityResources>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilityTools {
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilityPrompts {
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilityResources {
    #[serde(default)]
    pub subscribe: Option<bool>,
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Result of the `initialize` request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<ServerCapabilityTools>,
    #[serde(default)]
    pub prompts: Option<ServerCapabilityPrompts>,
    #[serde(default)]
    pub resources: Option<ServerCapabilityResources>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilityTools {
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilityPrompts {
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilityResources {
    #[serde(default)]
    pub subscribe: Option<bool>,
    #[serde(default)]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

// ── MCP domain types ──

/// An MCP tool definition returned by `tools/list`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Result of `tools/list`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// Arguments to `tools/call`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Result of `tools/call`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
}

impl CallToolResult {
    /// Extract text content from all text blocks, joined with newlines.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.as_text())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        resource: McpResourceContent,
    },
}

impl ToolContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

/// An MCP resource definition (from `resources/list`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// Inline resource content embedded in a tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

/// An MCP prompt definition (from `prompts/list`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<McpPromptArgument>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Result of `resources/list`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<McpResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// Result of `prompts/list`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsResult {
    pub prompts: Vec<McpPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// Notification that the client sends after initialize to signal readiness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializedNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl InitializedNotification {
    pub fn new() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
        }
    }
}

impl Default for InitializedNotification {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rpc_request_new_and_roundtrip() {
        let req = JsonRpcRequest::new(7, "tools/list", Some(serde_json::json!({"cursor": "a"})));
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, 7);
        assert_eq!(req.method, "tools/list");
        let encoded = serde_json::to_string(&req).unwrap();
        let decoded: JsonRpcRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.params, req.params);
    }

    #[test]
    fn call_tool_result_text_skips_non_text_blocks() {
        let result = CallToolResult {
            content: vec![
                ToolContent::Text {
                    text: "hello".into(),
                },
                ToolContent::Image {
                    data: "abc".into(),
                    mime_type: "image/png".into(),
                },
                ToolContent::Resource {
                    resource: McpResourceContent {
                        uri: "file:///x".into(),
                        mime_type: Some("text/plain".into()),
                        text: Some("ignored".into()),
                        blob: None,
                    },
                },
                ToolContent::Text {
                    text: "world".into(),
                },
            ],
            is_error: Some(false),
        };
        assert_eq!(result.text(), "hello\nworld");
        assert_eq!(
            ToolContent::Image {
                data: "x".into(),
                mime_type: "image/png".into()
            }
            .as_text(),
            None
        );
    }

    #[test]
    fn initialized_notification_default_matches_new() {
        let n = InitializedNotification::default();
        assert_eq!(n.jsonrpc, "2.0");
        assert_eq!(n.method, "notifications/initialized");
        assert!(n.params.is_none());
        let again = InitializedNotification::new();
        assert_eq!(again.method, n.method);
    }

    #[test]
    fn capability_and_list_payloads_roundtrip() {
        let init = InitializeParams {
            protocol_version: "2025-03-26".into(),
            capabilities: ClientCapabilities {
                tools: Some(ClientCapabilityTools {
                    list_changed: Some(true),
                }),
                prompts: Some(ClientCapabilityPrompts {
                    list_changed: Some(false),
                }),
                resources: Some(ClientCapabilityResources {
                    subscribe: Some(true),
                    list_changed: Some(false),
                }),
            },
            client_info: ClientInfo {
                name: "whycodes".into(),
                version: "0".into(),
            },
        };
        let value = serde_json::to_value(&init).unwrap();
        let back: InitializeParams = serde_json::from_value(value).unwrap();
        assert_eq!(back.client_info.name, "whycodes");

        let tools = ListToolsResult {
            tools: vec![McpTool {
                name: "echo".into(),
                description: Some("d".into()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            next_cursor: Some("n".into()),
        };
        assert_eq!(
            serde_json::from_value::<ListToolsResult>(serde_json::to_value(&tools).unwrap())
                .unwrap()
                .next_cursor
                .as_deref(),
            Some("n")
        );

        let resources = ListResourcesResult {
            resources: vec![McpResource {
                uri: "file:///a".into(),
                name: "a".into(),
                description: None,
                mime_type: Some("text/plain".into()),
            }],
            next_cursor: None,
        };
        assert_eq!(
            serde_json::from_value::<ListResourcesResult>(
                serde_json::to_value(&resources).unwrap()
            )
            .unwrap()
            .resources[0]
                .name,
            "a"
        );

        let prompts = ListPromptsResult {
            prompts: vec![McpPrompt {
                name: "review".into(),
                description: Some("d".into()),
                arguments: Some(vec![McpPromptArgument {
                    name: "path".into(),
                    description: None,
                    required: Some(true),
                }]),
            }],
            next_cursor: None,
        };
        assert_eq!(
            serde_json::from_value::<ListPromptsResult>(serde_json::to_value(&prompts).unwrap())
                .unwrap()
                .prompts[0]
                .name,
            "review"
        );

        let err = JsonRpcError {
            code: -32601,
            message: "nope".into(),
            data: Some(serde_json::json!({"hint": 1})),
        };
        let rpc = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: None,
            error: Some(err),
        };
        assert_eq!(
            serde_json::from_value::<JsonRpcResponse>(serde_json::to_value(&rpc).unwrap())
                .unwrap()
                .error
                .unwrap()
                .code,
            -32601
        );
    }

    #[test]
    fn server_capabilities_and_optional_fields_roundtrip() {
        let caps = ServerCapabilities {
            tools: Some(ServerCapabilityTools {
                list_changed: Some(true),
            }),
            prompts: Some(ServerCapabilityPrompts {
                list_changed: Some(false),
            }),
            resources: Some(ServerCapabilityResources {
                subscribe: Some(true),
                list_changed: Some(false),
            }),
        };
        let value = serde_json::to_value(&caps).unwrap();
        let back: ServerCapabilities = serde_json::from_value(value).unwrap();
        assert_eq!(back.prompts.unwrap().list_changed, Some(false));
        assert_eq!(back.resources.unwrap().subscribe, Some(true));
        let info = ServerInfo {
            name: "whycodes".into(),
            version: "0".into(),
        };
        let value = serde_json::to_value(&info).unwrap();
        let back: ServerInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back.name, "whycodes");
        let init = InitializeResult {
            protocol_version: "2024-11-05".into(),
            capabilities: caps,
            server_info: info,
        };
        let value = serde_json::to_value(&init).unwrap();
        let back: InitializeResult = serde_json::from_value(value).unwrap();
        assert_eq!(back.protocol_version, "2024-11-05");
        let params = CallToolParams {
            name: "echo".into(),
            arguments: Some(serde_json::json!({"a": 1})),
        };
        let value = serde_json::to_value(&params).unwrap();
        let back: CallToolParams = serde_json::from_value(value).unwrap();
        assert_eq!(back.name, "echo");
        let resource = McpResourceContent {
            uri: "file:///x".into(),
            mime_type: None,
            text: None,
            blob: Some("abc".into()),
        };
        let value = serde_json::to_value(&resource).unwrap();
        let back: McpResourceContent = serde_json::from_value(value).unwrap();
        assert_eq!(back.blob.as_deref(), Some("abc"));
        let req = JsonRpcRequest::new(1, "ping", None);
        let encoded = serde_json::to_string(&req).unwrap();
        assert!(!encoded.contains("params"));
        let rpc = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 2,
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        let encoded = serde_json::to_string(&rpc).unwrap();
        assert!(encoded.contains("result"));
        let err = JsonRpcError {
            code: -32700,
            message: "parse".into(),
            data: None,
        };
        let encoded = serde_json::to_string(&err).unwrap();
        assert!(!encoded.contains("data"));
        let prompt = McpPrompt {
            name: "p".into(),
            description: None,
            arguments: None,
        };
        let encoded = serde_json::to_string(&prompt).unwrap();
        assert!(!encoded.contains("arguments"));
        let arg = McpPromptArgument {
            name: "path".into(),
            description: Some("d".into()),
            required: None,
        };
        let encoded = serde_json::to_string(&arg).unwrap();
        assert!(encoded.contains("description"));
        let tool = McpTool {
            name: "t".into(),
            description: None,
            input_schema: serde_json::json!({}),
        };
        let encoded = serde_json::to_string(&tool).unwrap();
        assert!(!encoded.contains("description"));
        let list = ListToolsResult {
            tools: vec![tool],
            next_cursor: None,
        };
        let encoded = serde_json::to_string(&list).unwrap();
        assert!(!encoded.contains("nextCursor"));
        let notif = InitializedNotification {
            jsonrpc: "2.0".into(),
            method: "notifications/initialized".into(),
            params: Some(serde_json::json!({})),
        };
        let encoded = serde_json::to_string(&notif).unwrap();
        assert!(encoded.contains("params"));
        let call = CallToolResult {
            content: vec![],
            is_error: None,
        };
        assert!(call.text().is_empty());
        let encoded = serde_json::to_string(&call).unwrap();
        assert!(!encoded.contains("isError"));
    }
}
