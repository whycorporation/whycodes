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
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

/// The LSP wire format is an integer (1–4); snake_case names are accepted too.
/// An out-of-range number maps to `Hint` so one odd entry cannot drop the
/// whole `publishDiagnostics` batch.
impl<'de> Deserialize<'de> for DiagnosticSeverity {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(i64),
            Name(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(1) => Ok(Self::Error),
            Raw::Num(2) => Ok(Self::Warning),
            Raw::Num(3) => Ok(Self::Information),
            Raw::Num(_) => Ok(Self::Hint),
            Raw::Name(name) => match name.as_str() {
                "error" => Ok(Self::Error),
                "warning" => Ok(Self::Warning),
                "information" => Ok(Self::Information),
                "hint" => Ok(Self::Hint),
                other => Err(serde::de::Error::unknown_variant(
                    other,
                    &["error", "warning", "information", "hint"],
                )),
            },
        }
    }
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
        Self::with_options(workspace_root, None)
    }

    pub fn with_options(workspace_root: &str, init_options: Option<&serde_json::Value>) -> Self {
        let uri = crate::detect::file_uri(workspace_root);
        let mut params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": uri,
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
                    "diagnostics": { "refreshSupport": true },
                    "configuration": true
                }
            },
            "workspaceFolders": [{
                "uri": uri,
                "name": "workspace"
            }]
        });
        if let Some(opts) = init_options {
            params["initializationOptions"] = opts.clone();
        }
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
    /// Server → client request (`id` and `method`). The id may be a string.
    Request(ServerRequest),
}

/// A request the server sends us (e.g. `workspace/configuration`).
#[derive(Debug, Clone, Deserialize)]
pub struct ServerRequest {
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

impl IncomingMessage {
    pub fn from_line(line: &str) -> serde_json::Result<Self> {
        let val: serde_json::Value = serde_json::from_str(line)?;
        match (val.get("method").is_some(), val.get("id").is_some()) {
            (true, false) => Ok(IncomingMessage::Notification(serde_json::from_value(val)?)),
            (true, true) => Ok(IncomingMessage::Request(serde_json::from_value(val)?)),
            _ => Ok(IncomingMessage::Response(serde_json::from_value(val)?)),
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
#[path = "types_tests.rs"]
mod tests;
