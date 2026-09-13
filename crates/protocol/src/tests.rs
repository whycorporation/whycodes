//! Unit tests. Sibling file so llvm-cov `--ignore-filename-regex tests.rs$`
//! cannot sink the crate's 100% production floor.

use super::ci::{CiEvent, OutputFormat, ResultMeta};
use std::io::Write;
use whycodes_core::types::Usage;

#[test]
fn output_format_parse() {
    assert_eq!(OutputFormat::parse("text"), Some(OutputFormat::Text));
    assert_eq!(OutputFormat::parse("plain"), Some(OutputFormat::Text));
    assert_eq!(OutputFormat::parse("JSON"), Some(OutputFormat::Json));
    assert_eq!(
        OutputFormat::parse("stream-json"),
        Some(OutputFormat::StreamJson)
    );
    assert_eq!(
        OutputFormat::parse("stream_json"),
        Some(OutputFormat::StreamJson)
    );
    assert_eq!(
        OutputFormat::parse("ndjson"),
        Some(OutputFormat::StreamJson)
    );
    assert_eq!(OutputFormat::parse("jsonl"), Some(OutputFormat::StreamJson));
    assert_eq!(OutputFormat::parse("  Text  "), Some(OutputFormat::Text));
    assert_eq!(OutputFormat::parse("nope"), None);
}

#[test]
fn output_format_as_str_display_from_str_and_structured() {
    assert_eq!(OutputFormat::default(), OutputFormat::Text);
    assert_eq!(OutputFormat::Text.as_str(), "text");
    assert_eq!(OutputFormat::Json.as_str(), "json");
    assert_eq!(OutputFormat::StreamJson.as_str(), "stream-json");
    assert_eq!(OutputFormat::Text.to_string(), "text");
    assert_eq!(OutputFormat::Json.to_string(), "json");
    assert_eq!(OutputFormat::StreamJson.to_string(), "stream-json");
    assert!(!OutputFormat::Text.is_structured());
    assert!(OutputFormat::Json.is_structured());
    assert!(OutputFormat::StreamJson.is_structured());
    assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
    let err = "nope".parse::<OutputFormat>().unwrap_err();
    assert!(err.contains("invalid output format 'nope'"));
}

#[test]
fn ci_event_init_roundtrip() {
    let e = CiEvent::Init {
        session_id: "s1".into(),
        provider: "openai".into(),
        model: "gpt-4o".into(),
        agent: "build".into(),
        cwd: "/tmp".into(),
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains(r#""type":"init""#));
    let back: CiEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, e);
}

#[test]
fn ci_event_result_json_shape() {
    let e = CiEvent::Result {
        result: "hello".into(),
        session_id: "s".into(),
        provider: "x".into(),
        model: "m".into(),
        agent: "build".into(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
        duration_ms: 42,
        is_error: false,
        error: None,
        slop: None,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains(r#""type":"result""#));
    assert!(json.contains(r#""result":"hello""#));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("error").is_none()); // skip_serializing_if None
    assert_eq!(v["is_error"], false);
    assert_eq!(v["usage"]["input_tokens"], 10);
    assert!(v.get("slop").is_none());
}

#[test]
fn result_includes_optional_slop() {
    let e = CiEvent::Result {
        result: "ok".into(),
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Usage::default(),
        duration_ms: 1,
        is_error: false,
        error: None,
        slop: Some(serde_json::json!({"verdict":"ok","delta_loc":0})),
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains(r#""slop""#));
    let back: CiEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, e);
    let with_none = ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Usage::default(),
        duration_ms: 1,
        slop: None,
    }
    .err("x");
    let json = serde_json::to_string(&with_none).unwrap();
    assert!(!json.contains(r#""slop""#));
    let back: CiEvent = serde_json::from_str(&json).unwrap();
    match back {
        CiEvent::Result { slop: None, .. } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn write_line_is_ndjson() {
    let e = CiEvent::Status {
        message: "go".into(),
    };
    let mut buf = Vec::new();
    e.write_line(&mut buf).unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.ends_with('\n'));
    assert_eq!(s.lines().count(), 1);
    let parsed: CiEvent = serde_json::from_str(s.trim()).unwrap();
    assert_eq!(parsed, e);
}

#[test]
fn result_meta_err_sets_flag() {
    let meta = ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Usage::default(),
        duration_ms: 1,
        slop: None,
    };
    let ev = meta.err("boom");
    assert!(matches!(
        ev,
        CiEvent::Result {
            is_error: true,
            ref error,
            ref result,
            ..
        } if error.as_deref() == Some("boom") && result.is_empty()
    ));
}

#[test]
fn result_meta_ok_builds_success() {
    let meta = ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Usage::default(),
        duration_ms: 7,
        slop: None,
    };
    let ev = meta.ok("done");
    assert!(matches!(
        ev,
        CiEvent::Result {
            is_error: false,
            ref result,
            error: None,
            duration_ms: 7,
            slop: None,
            ..
        } if result == "done"
    ));
    let with = ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Usage::default(),
        duration_ms: 2,
        slop: Some(serde_json::json!({"verdict":"review"})),
    };
    let ev = with.ok("x");
    match ev {
        CiEvent::Result { slop: Some(v), .. } => {
            assert_eq!(v["verdict"], "review");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn emit_stdout_and_write_line_errors() {
    CiEvent::Cancelled.emit_stdout().unwrap();

    struct FailWriter;
    impl std::io::Write for FailWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("nope"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut fail = FailWriter;
    fail.flush().unwrap();
    let err = CiEvent::Cancelled.write_line(&mut fail).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Other);

    struct FailAfterJson;
    impl std::io::Write for FailAfterJson {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf == b"\n" {
                Err(std::io::Error::other("nl"))
            } else {
                Ok(buf.len())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut after = FailAfterJson;
    after.flush().unwrap();
    assert!(CiEvent::Cancelled.write_line(&mut after).is_err());
}
