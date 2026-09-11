use super::*;
use tokio::io::{AsyncBufRead, AsyncRead};

#[test]
fn known_extensions_resolve_to_a_language_server() {
    assert_eq!(
        language_server_for_extension("rs"),
        Some(("rust-analyzer".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("py"),
        Some(("pyright-langserver".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("ts"),
        Some(("typescript-language-server".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("go"),
        Some(("gopls".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("cs"),
        Some(("omnisharp".into(), vec!["--languageserver".into()]))
    );
}

#[test]
fn unknown_extensions_have_no_language_server() {
    assert_eq!(language_server_for_extension("xyz"), None);
    assert_eq!(language_server_for_extension(""), None);
    assert_eq!(language_server_for_extension("exe"), None);
}

#[test]
fn extension_maps_to_language_id() {
    assert_eq!(language_id_for_extension("rs"), "rust");
    assert_eq!(language_id_for_extension("py"), "python");
    assert_eq!(language_id_for_extension("tsx"), "typescriptreact");
    assert_eq!(language_id_for_extension("h"), "c");
    assert_eq!(language_id_for_extension("hpp"), "cpp");
    assert_eq!(language_id_for_extension("md"), "markdown");
    assert_eq!(language_id_for_extension("sh"), "shellscript");
    assert_eq!(language_id_for_extension("zzz"), "plaintext");
}

#[test]
fn strips_content_length_prefix() {
    assert_eq!(
        line_strip_prefix("Content-Length: ", "Content-Length: 123"),
        Some("123")
    );
    assert_eq!(line_strip_prefix("Content-Length: ", "foo"), None);
}

#[test]
fn more_extensions_have_servers_and_ids() {
    assert_eq!(
        language_server_for_extension("js"),
        Some(("typescript-language-server".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("c"),
        Some(("clangd".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("java"),
        Some(("jdtls".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("lua"),
        Some(("lua-language-server".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("zig"),
        Some(("zls".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("swift"),
        Some(("sourcekit-lsp".into(), vec![]))
    );
    assert_eq!(language_id_for_extension("go"), "go");
    assert_eq!(language_id_for_extension("yaml"), "yaml");
    assert_eq!(language_id_for_extension("toml"), "toml");
    assert_eq!(language_id_for_extension("json"), "json");
    assert_eq!(language_id_for_extension("java"), "java");
    assert_eq!(language_id_for_extension("lua"), "lua");
}

#[test]
fn encode_frame_and_parse_results() {
    let frame = encode_lsp_frame("{}");
    let s = String::from_utf8(frame).unwrap();
    assert!(s.starts_with("Content-Length: 2\r\n\r\n"));
    assert!(s.ends_with("{}"));

    assert!(parse_hover_result(None).unwrap().is_none());
    assert!(
        parse_hover_result(Some(serde_json::Value::Null))
            .unwrap()
            .is_none()
    );
    let hover = parse_hover_result(Some(serde_json::json!({
        "contents": "hello"
    })))
    .unwrap()
    .unwrap();
    assert_eq!(hover.contents_string(), "hello");

    assert!(parse_locations(None).unwrap().is_empty());
    assert!(
        parse_locations(Some(serde_json::Value::Null))
            .unwrap()
            .is_empty()
    );
    let one = parse_locations(Some(serde_json::json!({
        "uri": "file:///a.rs",
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
    })))
    .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].uri, "file:///a.rs");
    let many = parse_locations(Some(serde_json::json!([{
        "uri": "file:///a.rs",
        "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}
    }])))
    .unwrap();
    assert_eq!(many.len(), 1);
    assert!(
        parse_locations(Some(serde_json::json!({"uri": 1})))
            .unwrap_err()
            .to_string()
            .contains("invalid type")
            || parse_locations(Some(serde_json::json!({"uri": 1}))).is_err()
    );
    assert!(
        !parse_hover_result(Some(serde_json::json!([])))
            .unwrap_err()
            .to_string()
            .is_empty()
    );
    assert!(language_server_command(&crate::config::LspServerSpec::default()).is_none());
    assert!(language_server_for_extension("toml").is_none());
    let now = std::time::Instant::now();
    let slot = Mutex::new(now);
    mark_instant(&slot);
    assert!(idle_since(&slot) < std::time::Duration::from_secs(2));
    let held = Mutex::new(std::time::Instant::now());
    let _guard = held.try_lock().unwrap();
    mark_instant(&held);
    assert_eq!(idle_since(&held), std::time::Duration::ZERO);
    assert!(parse_locations(Some(serde_json::json!([{"uri": 1}]))).is_err());
}

fn fake_args(mode: &str) -> Vec<String> {
    vec!["-c".into(), FAKE_LSP_PY.into(), mode.into()]
}

async fn start_fake(mode: &str) -> LspClient {
    start_test_client(mode, false).await.unwrap()
}

fn pos() -> Position {
    Position {
        line: 0,
        character: 0,
    }
}

#[tokio::test]
async fn start_fails_when_command_is_missing() {
    let err = match LspClient::start("whycodes-lsp-missing-bin", &[], "/tmp", "rust").await {
        Err(e) => e,
        Ok(_) => panic!("expected spawn failure"),
    };
    assert!(err.to_string().contains("Failed to spawn"));
}

#[tokio::test]
async fn initialize_hover_definition_and_references() {
    let client = start_fake("ok").await;
    client
        .open_document("file:///tmp/a.rs", None)
        .await
        .unwrap();
    let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
    assert_eq!(hover.unwrap().contents_string(), "hello");
    let types = client
        .type_definition("file:///tmp/a.rs", pos())
        .await
        .unwrap();
    assert_eq!(types[0].uri, "file:///tmp/a.rs");
    let impls = client
        .implementation("file:///tmp/a.rs", pos())
        .await
        .unwrap();
    assert_eq!(impls.len(), 1);
    let defs = client.definition("file:///tmp/a.rs", pos()).await.unwrap();
    assert_eq!(defs[0].uri, "file:///tmp/a.rs");
    let refs = client.references("file:///tmp/a.rs", pos()).await.unwrap();
    assert_eq!(refs.len(), 1);
    let diags = client.get_diagnostics("file:///tmp/a.rs").await.unwrap();
    assert_eq!(diags[0].message, "boom");
    let cached = client.get_diagnostics("file:///tmp/a.rs").await.unwrap();
    assert_eq!(cached.len(), 1);
    client
        .open_document("file:///tmp/a.rs", Some("fn main() {}"))
        .await
        .unwrap();
}

#[tokio::test]
async fn empty_results_and_null_diagnostics() {
    let client = start_fake("empty").await;
    assert!(
        client
            .hover("file:///tmp/a.rs", pos())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        client
            .definition("file:///tmp/a.rs", pos())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        client
            .references("file:///tmp/a.rs", pos())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        client
            .get_diagnostics("file:///tmp/a.rs")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn diagnostic_request_without_result() {
    let client = start_fake("no_result").await;
    assert!(
        client
            .get_diagnostics("file:///tmp/a.rs")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn diagnostic_request_with_unparseable_payload() {
    let client = start_fake("bad_diag").await;
    assert!(
        client
            .get_diagnostics("file:///tmp/a.rs")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn request_skips_unmatched_ids_and_noise() {
    let client = start_fake("skip_id").await;
    let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
    assert_eq!(hover.unwrap().contents_string(), "hello");
}

#[tokio::test]
async fn request_skips_non_framed_lines() {
    let client = start_fake("noise").await;
    let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
    assert_eq!(hover.unwrap().contents_string(), "hello");
}

#[tokio::test]
async fn request_rejects_bad_content_length() {
    let err = match LspClient::start(test_python(), &fake_args("bad_len"), "/tmp", "rust").await {
        Err(e) => e,
        Ok(_) => panic!("expected bad Content-Length"),
    };
    assert!(err.to_string().contains("bad Content-Length"));
    assert!(err.to_string().contains("initialize request failed"));
}

#[tokio::test]
async fn diagnostic_request_fails_when_stdout_closes() {
    let client = start_fake("init_then_eof").await;
    let err = client
        .get_diagnostics("file:///tmp/a.rs")
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("textDocument/diagnostic request failed")
    );
}

#[tokio::test]
async fn request_errors_when_stdout_closes() {
    let client = start_fake("init_then_eof").await;
    let err = client.hover("file:///tmp/a.rs", pos()).await.unwrap_err();
    assert!(err.to_string().contains("closed stdout"));
}

#[tokio::test]
async fn initialized_notification_fails_when_server_exits() {
    let err = match LspClient::start(test_python(), &fake_args("close_stdin"), "/tmp", "rust").await
    {
        Err(e) => e,
        Ok(_) => panic!("expected initialized notification failure"),
    };
    assert!(
        err.to_string().contains("initialized notification failed")
            || err.to_string().contains("Broken pipe")
            || err.to_string().contains("closed")
            || err.to_string().contains("os error")
    );
}

#[tokio::test]
async fn open_document_fails_after_child_exits() {
    let client = start_fake("ok").await;
    client.kill_for_test().await;
    let err = client
        .open_document("file:///tmp/a.rs", Some("fn main() {}"))
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn malformed_response_body_is_an_error() {
    let client = start_fake("bad_resp").await;
    let err = client.hover("file:///tmp/a.rs", pos()).await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn initialize_times_out_when_server_hangs() {
    let args = fake_args("hang_init");
    let fut = LspClient::start(test_python(), &args, "/tmp", "rust");
    let timed = tokio::time::timeout(std::time::Duration::from_millis(200), fut).await;
    assert!(timed.is_err());
}

#[tokio::test]
async fn hover_times_out_when_server_hangs() {
    let client = start_fake("hang").await;
    let timed = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        client.hover("file:///tmp/a.rs", pos()),
    )
    .await;
    assert!(timed.is_err());
}

#[tokio::test]
async fn background_reader_stores_publish_diagnostics() {
    let client = start_fake("diag_notify").await;
    consume_stdout(
        client.stdout.clone(),
        client.diagnostics.clone(),
        "fake".into(),
    )
    .await;
    let diags = client.get_diagnostics("file:///tmp/a.rs").await.unwrap();
    assert_eq!(diags[0].message, "boom");
}

#[tokio::test]
async fn start_with_reader_collects_diagnostics_in_background() {
    let client = start_test_client("diag_notify", true).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let diags = client.diagnostics.lock().await;
    assert_eq!(diags.get("file:///tmp/a.rs").unwrap()[0].message, "boom");
}

#[tokio::test]
async fn background_reader_handles_malformed_and_other_messages() {
    for mode in [
        "bad_json",
        "response_notify",
        "empty_line",
        "other_notify",
        "unparsed_diag",
        "diag_no_params",
        "bg_noise",
        "partial_line",
        "trunc_body",
        "trunc_sep",
        "bg_bad_len",
    ] {
        let client = start_fake(mode).await;
        consume_stdout(
            client.stdout.clone(),
            client.diagnostics.clone(),
            "fake".into(),
        )
        .await;
    }
}

#[tokio::test]
async fn consume_stdout_breaks_on_read_error() {
    struct ErrReader;
    impl tokio::io::AsyncRead for ErrReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::ready!(AsyncBufRead::poll_fill_buf(self, cx))?;
            buf.initialize_unfilled()[0] = 0;
            buf.advance(0);
            std::task::Poll::Ready(Ok(()))
        }
    }
    impl tokio::io::AsyncBufRead for ErrReader {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            std::task::Poll::Ready(Err(std::io::Error::other("boom")))
        }
        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }
    let stdout = Arc::new(Mutex::new(ErrReader));
    consume_stdout(stdout, Arc::new(Mutex::new(HashMap::new())), "err".into()).await;
    let mut reader = ErrReader;
    let mut buf = [0u8; 1];
    let mut read_buf = tokio::io::ReadBuf::new(&mut buf);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let _ = AsyncRead::poll_read(std::pin::Pin::new(&mut reader), &mut cx, &mut read_buf);
}

#[tokio::test]
async fn write_framed_roundtrip() {
    let mut buf = Vec::new();
    write_framed(&mut buf, &json!({"ok": true})).await.unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("Content-Length:"));
    assert!(s.contains("\"ok\":true"));
}

#[tokio::test]
async fn write_framed_fails_when_writer_errors() {
    struct FailWriter;
    impl tokio::io::AsyncWrite for FailWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::other("nope")))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let err = write_framed(&mut FailWriter, &json!({})).await.unwrap_err();
    assert!(err.to_string().contains("nope"));
}

#[tokio::test]
async fn write_framed_fails_when_flush_errors() {
    struct FlushFailWriter;
    impl tokio::io::AsyncWrite for FlushFailWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("flush nope")))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let err = write_framed(&mut FlushFailWriter, &json!({"ok": true}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("flush nope"));
}

#[test]
fn parse_helpers_surface_json_errors() {
    assert!(parse_hover_result(Some(json!(1))).is_err());
    assert!(parse_locations(Some(json!("not a location"))).is_err());
}

#[test]
fn remaining_language_ids_and_servers() {
    assert_eq!(language_id_for_extension("jsx"), "javascriptreact");
    assert_eq!(language_id_for_extension("cc"), "cpp");
    assert_eq!(language_id_for_extension("cxx"), "cpp");
    assert_eq!(language_id_for_extension("hxx"), "cpp");
    assert_eq!(language_id_for_extension("cs"), "csharp");
    assert_eq!(language_id_for_extension("zig"), "zig");
    assert_eq!(language_id_for_extension("swift"), "swift");
    assert_eq!(language_id_for_extension("yml"), "yaml");
    assert_eq!(language_id_for_extension("bash"), "shellscript");
    assert_eq!(language_id_for_extension("ts"), "typescript");
    assert_eq!(language_id_for_extension("js"), "javascript");
    assert_eq!(language_id_for_extension("c"), "c");
    assert_eq!(language_id_for_extension("cpp"), "cpp");
    assert_eq!(
        language_server_for_extension("tsx"),
        Some(("typescript-language-server".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("jsx"),
        Some(("typescript-language-server".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("cpp"),
        Some(("clangd".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("h"),
        Some(("clangd".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("hpp"),
        Some(("clangd".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("cc"),
        Some(("clangd".into(), vec![]))
    );
    let (cmd, args) = language_server_for_extension("whycodes_lsp_fake").unwrap();
    assert_eq!(cmd, test_python());
    assert_eq!(args[0], "-c");
    assert!(language_server_for_extension("whycodes_lsp_missing").is_some());
    assert!(language_server_for_extension("whycodes_lsp_empty").is_some());
    assert!(language_server_for_extension("whycodes_lsp_err").is_some());
    assert!(language_server_for_extension("whycodes_lsp_failopen").is_some());
    assert!(language_server_for_extension("whycodes_lsp_nocmd").is_none());
    assert_eq!(
        language_server_for_extension("sh"),
        Some(("bash-language-server".into(), vec!["start".into()]))
    );
    assert_eq!(
        language_server_for_extension("yaml"),
        Some(("yaml-language-server".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("yml"),
        Some(("yaml-language-server".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("bash"),
        Some(("bash-language-server".into(), vec!["start".into()]))
    );
    assert_eq!(
        language_server_for_extension("pyi"),
        Some(("pyright-langserver".into(), vec!["--stdio".into()]))
    );
    assert_eq!(
        language_server_for_extension("cxx"),
        Some(("clangd".into(), vec![]))
    );
    assert_eq!(
        language_server_for_extension("hxx"),
        Some(("clangd".into(), vec![]))
    );
}

#[test]
fn command_available_finds_sh_and_rejects_missing() {
    assert!(
        command_available("sh")
            || command_available("cmd")
            || command_available("python")
            || command_available("python3")
    );
    assert!(!command_available("whycodes-lsp-bin-that-does-not-exist"));
    assert!(!command_available_with("whycodes-which-missing", "sh"));
    assert!(
        take_child_pipe::<i32>(None, "stdin")
            .unwrap_err()
            .to_string()
            .contains("no stdin")
    );
    assert_eq!(take_child_pipe(Some(7), "stdout").unwrap(), 7);
    assert_eq!(take_stdio_pair(Some(1), Some(2)).unwrap(), (1, 2));
    assert!(
        take_stdio_pair::<i32, i32>(None, Some(2))
            .unwrap_err()
            .to_string()
            .contains("no stdin")
    );
    assert!(
        take_stdio_pair::<i32, i32>(Some(1), None)
            .unwrap_err()
            .to_string()
            .contains("no stdout")
    );
    assert!(LspClient::spawn_background(true));
    assert!(!LspClient::spawn_background(false));
    assert_eq!(
        test_python_from(&["whycodes-no-python"], |_| None),
        "python3"
    );
    assert!(
        command_available_with("where", "cmd")
            || command_available_with("which", "sh")
            || command_available_with("which", "python3")
    );
}

#[tokio::test]
async fn start_public_api_initializes_without_settings() {
    let client = LspClient::start(test_python(), &fake_args("ok"), "/tmp", "rust")
        .await
        .unwrap();
    assert!(client.settings().is_none());
    let hover = client.hover("file:///tmp/a.rs", pos()).await.unwrap();
    assert_eq!(hover.unwrap().contents_string(), "hello");
}

#[tokio::test]
async fn request_errors_when_separator_is_truncated() {
    let client = start_fake("trunc_req_sep").await;
    let err = client.hover("file:///tmp/a.rs", pos()).await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn request_errors_when_body_is_truncated() {
    let client = start_fake("trunc_req_body").await;
    let err = client.hover("file:///tmp/a.rs", pos()).await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn start_configured_sends_init_options_and_settings() {
    let args = fake_args("ok");
    let py = test_python();
    let settings = serde_json::json!({"checkOnSave": false});
    let init = serde_json::json!({"cargo": {"buildScripts": true}});
    let client =
        LspClient::start_configured(py, &args, "/tmp", "rust", Some(&init), Some(&settings))
            .await
            .unwrap();
    assert_eq!(client.settings(), Some(&settings));
    client.mark_used();
    assert!(client.idle_for() < std::time::Duration::from_secs(2));
}

#[tokio::test]
async fn did_change_configuration_fails_when_server_exits() {
    let args = vec![
        "-c".into(),
        FAKE_LSP_PY.into(),
        "die_after_initialized".into(),
    ];
    let py = test_python();
    let settings = serde_json::json!({"checkOnSave": false});
    let err =
        match LspClient::start_configured(py, &args, "/tmp", "rust", None, Some(&settings)).await {
            Err(e) => e,
            Ok(_) => panic!("expected didChangeConfiguration failure"),
        };
    assert!(
        err.to_string().contains("didChangeConfiguration")
            || err.to_string().contains("initialized"),
        "{err}"
    );
}

#[tokio::test]
async fn open_document_reads_missing_uri_as_empty() {
    let client = start_fake("ok").await;
    client.open_document("not-a-file-uri", None).await.unwrap();
    client
        .open_document("file:///whycodes-lsp-missing-file.rs", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn open_document_reads_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn main() {}").unwrap();
    let client = start_fake("ok").await;
    client
        .open_document(&crate::detect::file_uri(&path), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn document_and_workspace_symbols_cover_null_and_values() {
    let client = start_fake("ok").await;
    let doc = client.document_symbols("file:///tmp/a.rs").await.unwrap();
    assert!(doc.to_string().contains("main"));
    let ws = client.workspace_symbols("main").await.unwrap();
    assert!(ws.to_string().contains("main"));
    let empty = start_fake("empty").await;
    assert!(
        empty
            .document_symbols("file:///tmp/a.rs")
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    let none = start_fake("no_result").await;
    assert!(
        none.document_symbols("file:///tmp/a.rs")
            .await
            .unwrap()
            .is_null()
    );
    assert!(none.workspace_symbols("main").await.unwrap().is_null());
}

#[tokio::test]
async fn consume_stdout_breaks_when_separator_read_fails() {
    struct HeaderThenErr {
        n: u8,
    }
    impl tokio::io::AsyncRead for HeaderThenErr {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let available = std::task::ready!(AsyncBufRead::poll_fill_buf(self, cx))?;
            let n = available.len().min(buf.remaining());
            buf.put_slice(&available[..n]);
            std::task::Poll::Ready(Ok(()))
        }
    }
    impl tokio::io::AsyncBufRead for HeaderThenErr {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            let this = self.get_mut();
            if this.n == 0 {
                this.n = 1;
                // Safety: the buffer is stored on the struct for the duration
                // of this poll; AsyncBufRead requires the slice to remain
                // valid until consume. We keep a static header instead.
                std::task::Poll::Ready(Ok(b"Content-Length: 2\n".as_slice()))
            } else {
                let _ = cx;
                std::task::Poll::Ready(Err(std::io::Error::other("sep boom")))
            }
        }
        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }
    let stdout = Arc::new(Mutex::new(HeaderThenErr { n: 0 }));
    consume_stdout(stdout, Arc::new(Mutex::new(HashMap::new())), "sep".into()).await;
    let mut reader = HeaderThenErr { n: 0 };
    let mut buf = [0u8; 32];
    let mut read_buf = tokio::io::ReadBuf::new(&mut buf);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let _ = AsyncRead::poll_read(std::pin::Pin::new(&mut reader), &mut cx, &mut read_buf);
    let _ = AsyncRead::poll_read(std::pin::Pin::new(&mut reader), &mut cx, &mut read_buf);
}

#[tokio::test]
async fn start_without_stdio_fails_on_missing_pipes() {
    let err = match LspClient::start_without_stdio(test_python(), &fake_args("ok")).await {
        Err(e) => e,
        Ok(_) => panic!("expected missing stdio pipes"),
    };
    assert!(
        err.to_string().contains("no stdin") || err.to_string().contains("no stdout"),
        "{err}"
    );
}

#[tokio::test]
async fn start_without_stdout_fails_on_missing_pipe() {
    let err = match LspClient::start_without_stdout(test_python(), &fake_args("ok")).await {
        Err(e) => e,
        Ok(_) => panic!("expected missing stdout pipe"),
    };
    assert!(err.to_string().contains("no stdout"), "{err}");
}

#[tokio::test]
async fn read_rpc_response_fails_when_header_read_errors() {
    struct ErrReader;
    impl tokio::io::AsyncRead for ErrReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::ready!(AsyncBufRead::poll_fill_buf(self, cx))?;
            buf.advance(0);
            std::task::Poll::Ready(Ok(()))
        }
    }
    impl tokio::io::AsyncBufRead for ErrReader {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            std::task::Poll::Ready(Err(std::io::Error::other("header boom")))
        }
        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }
    let err = read_rpc_response(&mut ErrReader, 1, "hover")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("header boom"));
}

#[tokio::test]
async fn read_rpc_response_fails_when_separator_read_errors() {
    struct HeaderThenErr {
        n: u8,
    }
    impl tokio::io::AsyncRead for HeaderThenErr {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let available = std::task::ready!(AsyncBufRead::poll_fill_buf(self, cx))?;
            let n = available.len().min(buf.remaining());
            buf.put_slice(&available[..n]);
            std::task::Poll::Ready(Ok(()))
        }
    }
    impl tokio::io::AsyncBufRead for HeaderThenErr {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            let this = self.get_mut();
            if this.n == 0 {
                this.n = 1;
                std::task::Poll::Ready(Ok(b"Content-Length: 2\n".as_slice()))
            } else {
                let _ = cx;
                std::task::Poll::Ready(Err(std::io::Error::other("sep boom")))
            }
        }
        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }
    let err = read_rpc_response(&mut HeaderThenErr { n: 0 }, 1, "hover")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("sep boom"));
}
