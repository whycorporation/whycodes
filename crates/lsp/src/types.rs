use serde::{Deserialize, Serialize};

// ── LSP basic types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<DiagnosticSeverity>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<DiagnosticCode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiagnosticCode {
    String(String),
    Number(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoverResult {
    pub contents: HoverContents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HoverContents {
    MarkupContent(MarkupContent),
    String(String),
    Array(Vec<MarkupContent>),
}

impl HoverResult {
    /// Flatten hover contents to a single string.
    pub fn contents_string(&self) -> String {
        match &self.contents {
            HoverContents::MarkupContent(mc) => mc.value.clone(),
            HoverContents::String(s) => s.clone(),
            HoverContents::Array(arr) => arr
                .iter()
                .map(|mc| mc.value.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkupContent {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CompletionItemKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionItemKind {
    Text = 1,
    Method = 2,
    Function = 3,
    Constructor = 4,
    Field = 5,
    Variable = 6,
    Class = 7,
    Interface = 8,
    Module = 9,
    Property = 10,
    Unit = 11,
    Value = 12,
    Enum = 13,
    Keyword = 14,
    Snippet = 15,
    Color = 16,
    File = 17,
    Reference = 18,
    Folder = 19,
    EnumMember = 20,
    Constant = 21,
    Struct = 22,
    Event = 23,
    Operator = 24,
    TypeParameter = 25,
}

// ── Initialize ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(flatten)]
    pub inner: serde_json::Value,
}

impl InitializeParams {
    pub fn minimal(workspace_root: &str) -> Self {
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root),
            "rootPath": workspace_root,
            "capabilities": {
                "textDocument": {
                    "hover": { "dynamicRegistration": true },
                    "definition": { "dynamicRegistration": true },
                    "references": { "dynamicRegistration": true },
                    "completion": {
                        "dynamicRegistration": true,
                        "completionItem": { "snippetSupport": false }
                    }
                },
                "workspace": {
                    "diagnostics": { "refreshSupport": true }
                }
            },
            "workspaceFolders": [{
                "uri": format!("file://{}", workspace_root),
                "name": "workspace"
            }]
        });
        Self { inner: params }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub capabilities: serde_json::Value,
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentPositionParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

// ── JSON-RPC ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: i64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: i64, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params: Some(params),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Result of parsing an incoming line — could be a response (`id` present but
/// no `method`) or a notification (`method` present but no `id`).
#[derive(Debug, Clone)]
pub enum IncomingMessage {
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

impl IncomingMessage {
    pub fn from_line(line: &str) -> serde_json::Result<Self> {
        let val: serde_json::Value = serde_json::from_str(line)?;
        if val.get("method").is_some() && val.get("id").is_none() {
            Ok(IncomingMessage::Notification(serde_json::from_str(line)?))
        } else {
            Ok(IncomingMessage::Response(serde_json::from_str(line)?))
        }
    }
}

// ── PublishDiagnostics (notification from server) ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_roundtrip() {
        let p = Position {
            line: 3,
            character: 7,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["line"], 3);
        assert_eq!(v["character"], 7);
        let back: Position = serde_json::from_value(v).unwrap();
        assert_eq!(back.line, 3);
        assert_eq!(back.character, 7);
    }

    #[test]
    fn diagnostic_roundtrip_with_optional_fields() {
        let d = Diagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 2,
                },
            },
            severity: Some(DiagnosticSeverity::Warning),
            message: "careful".into(),
            source: Some("rustc".into()),
            code: Some(DiagnosticCode::String("E0308".into())),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["code"], "E0308");
        let back: Diagnostic = serde_json::from_value(v).unwrap();
        assert!(matches!(back.severity, Some(DiagnosticSeverity::Warning)));
        assert!(matches!(
            back.code,
            Some(DiagnosticCode::String(ref s)) if s == "E0308"
        ));
    }

    #[test]
    fn diagnostic_omits_unset_optionals() {
        let d = Diagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 1,
                },
            },
            severity: None,
            message: "x".into(),
            source: None,
            code: None,
        };
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("severity").is_none());
        assert!(v.get("source").is_none());
        assert!(v.get("code").is_none());
    }

    #[test]
    fn numeric_diagnostic_code_roundtrips() {
        let d = DiagnosticCode::Number(42);
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v, 42);
        let back: DiagnosticCode = serde_json::from_value(v).unwrap();
        assert!(matches!(back, DiagnosticCode::Number(42)));
    }

    #[test]
    fn severity_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_value(DiagnosticSeverity::Error).unwrap(),
            "error"
        );
        assert_eq!(
            serde_json::to_value(DiagnosticSeverity::Information).unwrap(),
            "information"
        );
        let back: DiagnosticSeverity = serde_json::from_str(r#""hint""#).unwrap();
        assert_eq!(back, DiagnosticSeverity::Hint);
    }

    #[test]
    fn hover_contents_string_flattens_every_variant() {
        let markup = HoverResult {
            contents: HoverContents::MarkupContent(MarkupContent {
                kind: "markdown".into(),
                value: "**hi**".into(),
            }),
            range: None,
        };
        assert_eq!(markup.contents_string(), "**hi**");

        let plain = HoverResult {
            contents: HoverContents::String("plain".into()),
            range: None,
        };
        assert_eq!(plain.contents_string(), "plain");

        let arr = HoverResult {
            contents: HoverContents::Array(vec![
                MarkupContent {
                    kind: "markdown".into(),
                    value: "one".into(),
                },
                MarkupContent {
                    kind: "markdown".into(),
                    value: "two".into(),
                },
            ]),
            range: None,
        };
        assert_eq!(arr.contents_string(), "one\ntwo");
    }

    #[test]
    fn minimal_initialize_params_carries_the_workspace() {
        let p = InitializeParams::minimal("/workspace");
        assert_eq!(p.inner["rootUri"], "file:///workspace");
        assert_eq!(p.inner["rootPath"], "/workspace");
        assert_eq!(p.inner["workspaceFolders"][0]["name"], "workspace");
        assert!(p.inner.get("processId").is_some());
        assert!(p.inner["capabilities"]["textDocument"]["completion"].is_object());
    }

    #[test]
    fn json_rpc_request_and_notification_shapes() {
        let req = JsonRpcRequest::new(1, "initialize", serde_json::json!({}));
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, 1);
        assert_eq!(req.method, "initialize");
        let notif = JsonRpcNotification::new("initialized", serde_json::json!({}));
        assert_eq!(notif.jsonrpc, "2.0");
        assert_eq!(notif.method, "initialized");
        assert!(notif.params.is_some());
        let v = serde_json::to_value(&notif).unwrap();
        assert!(v.get("id").is_none(), "notifications have no id");
    }

    #[test]
    fn incoming_message_detects_notifications_and_responses() {
        let notif = IncomingMessage::from_line(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{}}"#,
        )
        .unwrap();
        assert!(matches!(notif, IncomingMessage::Notification(_)));

        let resp = IncomingMessage::from_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap();
        assert!(matches!(resp, IncomingMessage::Response(_)));

        // An id wins even if a method sneaks in — the server is answering us.
        let weird = IncomingMessage::from_line(r#"{"jsonrpc":"2.0","id":2,"method":"x"}"#).unwrap();
        assert!(matches!(weird, IncomingMessage::Response(_)));

        assert!(IncomingMessage::from_line("not json").is_err());
    }
}
