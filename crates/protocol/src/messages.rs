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
    TextDelta { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { id: String, content: String, is_error: bool },
    Thinking { text: String },
    Usage { input_tokens: u64, output_tokens: u64 },
    Done,
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_chat_json() {
        let msg = ClientMessage::Chat { content: "hello".to_string() };
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
        let msg = ServerMessage::Error { message: "oops".to_string() };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("oops"));
    }

    #[test]
    fn test_deserialize_text_delta() {
        let json = r#"{"type":"text_delta","text":"hi"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        if let ServerMessage::TextDelta { text } = msg {
            assert_eq!(text, "hi");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn test_deserialize_done() {
        let json = r#"{"type":"done"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ServerMessage::Done));
    }
}
