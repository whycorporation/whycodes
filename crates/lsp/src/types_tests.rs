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
    assert_eq!(p.inner["rootUri"], crate::detect::file_uri("/workspace"));
    assert_eq!(p.inner["rootPath"], "/workspace");
    let with_opts = InitializeParams::with_options(
        "/workspace",
        Some(&serde_json::json!({"cargo": {"buildScripts": true}})),
    );
    assert_eq!(
        with_opts.inner["initializationOptions"]["cargo"]["buildScripts"],
        true
    );
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
    assert!(IncomingMessage::from_line(r#"{"method":"x"}"#).is_err());
    assert!(IncomingMessage::from_line(r#"{"id":1}"#).is_err());
}

#[test]
fn remaining_severity_and_completion_kinds_roundtrip() {
    for sev in [
        DiagnosticSeverity::Error,
        DiagnosticSeverity::Warning,
        DiagnosticSeverity::Information,
        DiagnosticSeverity::Hint,
    ] {
        let v = serde_json::to_value(&sev).unwrap();
        let back: DiagnosticSeverity = serde_json::from_value(v).unwrap();
        assert_eq!(back, sev);
    }
    let kinds = [
        CompletionItemKind::Text,
        CompletionItemKind::Method,
        CompletionItemKind::Function,
        CompletionItemKind::Constructor,
        CompletionItemKind::Field,
        CompletionItemKind::Variable,
        CompletionItemKind::Class,
        CompletionItemKind::Interface,
        CompletionItemKind::Module,
        CompletionItemKind::Property,
        CompletionItemKind::Unit,
        CompletionItemKind::Value,
        CompletionItemKind::Enum,
        CompletionItemKind::Keyword,
        CompletionItemKind::Snippet,
        CompletionItemKind::Color,
        CompletionItemKind::File,
        CompletionItemKind::Reference,
        CompletionItemKind::Folder,
        CompletionItemKind::EnumMember,
        CompletionItemKind::Constant,
        CompletionItemKind::Struct,
        CompletionItemKind::Event,
        CompletionItemKind::Operator,
        CompletionItemKind::TypeParameter,
    ];
    for kind in kinds {
        let item = CompletionItem {
            label: "x".into(),
            detail: Some("d".into()),
            insert_text: Some("i".into()),
            kind: Some(kind),
            documentation: Some("docs".into()),
        };
        let v = serde_json::to_value(&item).unwrap();
        let back: CompletionItem = serde_json::from_value(v).unwrap();
        assert_eq!(back.label, "x");
        assert!(back.kind.is_some());
    }
    let init = InitializeResult {
        capabilities: serde_json::json!({"hoverProvider": true}),
        server_info: Some(ServerInfo {
            name: "fake".into(),
            version: Some("1".into()),
        }),
    };
    let v = serde_json::to_value(&init).unwrap();
    let back: InitializeResult = serde_json::from_value(v).unwrap();
    assert_eq!(back.server_info.unwrap().name, "fake");
    let loc = Location {
        uri: "file:///a.rs".into(),
        range: Range {
            start: Position {
                line: 1,
                character: 2,
            },
            end: Position {
                line: 1,
                character: 3,
            },
        },
    };
    let v = serde_json::to_value(&loc).unwrap();
    let back: Location = serde_json::from_value(v).unwrap();
    assert_eq!(back.uri, "file:///a.rs");
    let hover = HoverResult {
        contents: HoverContents::String("x".into()),
        range: Some(loc.range.clone()),
    };
    let v = serde_json::to_value(&hover).unwrap();
    let back: HoverResult = serde_json::from_value(v).unwrap();
    assert!(back.range.is_some());
    let err = JsonRpcError {
        code: -32601,
        message: "nope".into(),
        data: Some(serde_json::json!({"hint": 1})),
    };
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id: Some(1),
        result: None,
        error: Some(err),
    };
    let v = serde_json::to_value(&resp).unwrap();
    let back: JsonRpcResponse = serde_json::from_value(v).unwrap();
    assert_eq!(back.error.unwrap().code, -32601);
    let params = PublishDiagnosticsParams {
        uri: "file:///a.rs".into(),
        diagnostics: vec![],
    };
    let v = serde_json::to_value(&params).unwrap();
    let back: PublishDiagnosticsParams = serde_json::from_value(v).unwrap();
    assert_eq!(back.uri, "file:///a.rs");
    let ident = TextDocumentIdentifier {
        uri: "file:///a.rs".into(),
    };
    let pos = TextDocumentPositionParams {
        text_document: ident,
        position: Position {
            line: 0,
            character: 0,
        },
    };
    let v = serde_json::to_value(&pos).unwrap();
    let back: TextDocumentPositionParams = serde_json::from_value(v).unwrap();
    assert_eq!(back.text_document.uri, "file:///a.rs");
    let doc = TextDocumentItem {
        uri: "file:///a.rs".into(),
        language_id: "rust".into(),
        version: 1,
        text: "fn main() {}".into(),
    };
    let v = serde_json::to_value(&doc).unwrap();
    let back: TextDocumentItem = serde_json::from_value(v).unwrap();
    assert_eq!(back.language_id, "rust");
    let req = JsonRpcRequest::new(3, "textDocument/hover", serde_json::json!({"uri": "x"}));
    let v = serde_json::to_value(&req).unwrap();
    let back: JsonRpcRequest = serde_json::from_value(v).unwrap();
    assert_eq!(back.method, "textDocument/hover");
}
