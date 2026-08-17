use serde::{Deserialize, Serialize};

/// Client-to-server message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Chat { content: String },
    CreateSession { project: Option<String> },
    ListSessions,
    GetTools,
    GetModels,
}

/// Server-to-client message types
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    TextDelta {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    Thinking {
        text: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Done,
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_chat_json() {
        let msg = ClientMessage::Chat {
            content: "hello".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("chat"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn test_server_message_done() {
        let json = serde_json::to_string(&ServerMessage::Done).unwrap();
        assert!(json.contains("done"));
    }

    #[test]
    fn test_server_message_error() {
        let msg = ServerMessage::Error {
            message: "oops".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("oops"));
    }

    #[test]
    fn test_deserialize_text_delta() {
        let json = r#"{"type":"text_delta","text":"hi"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ServerMessage::TextDelta { text } if text == "hi"));
    }

    #[test]
    fn client_and_server_variants_roundtrip() {
        let clients = [
            ClientMessage::Chat {
                content: "hi".into(),
            },
            ClientMessage::CreateSession {
                project: Some("p".into()),
            },
            ClientMessage::ListSessions,
            ClientMessage::GetTools,
            ClientMessage::GetModels,
        ];
        for msg in clients {
            let json = serde_json::to_string(&msg).unwrap();
            let _: ClientMessage = serde_json::from_str(&json).unwrap();
        }

        let servers = [
            ServerMessage::TextDelta { text: "t".into() },
            ServerMessage::ToolUse {
                id: "1".into(),
                name: "n".into(),
                input: serde_json::json!({}),
            },
            ServerMessage::ToolResult {
                id: "1".into(),
                content: "c".into(),
                is_error: false,
            },
            ServerMessage::Thinking { text: "th".into() },
            ServerMessage::Usage {
                input_tokens: 1,
                output_tokens: 2,
            },
            ServerMessage::Done,
            ServerMessage::Error {
                message: "e".into(),
            },
        ];
        for msg in servers {
            let json = serde_json::to_string(&msg).unwrap();
            let _: ServerMessage = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn test_deserialize_done() {
        let json = r#"{"type":"done"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ServerMessage::Done));
    }
}
