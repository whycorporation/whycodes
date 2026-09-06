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
        serde_json::from_value::<ListResourcesResult>(serde_json::to_value(&resources).unwrap())
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
