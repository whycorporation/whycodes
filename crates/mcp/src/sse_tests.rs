use super::*;

#[test]
fn parse_simple_message() {
    let events = parse_sse_body("event: message\ndata: {\"ok\":true}\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("message"));
    assert_eq!(events[0].data, r#"{"ok":true}"#);
}

#[test]
fn parse_multiline_data() {
    let events = parse_sse_body("data: line1\ndata: line2\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "line1\nline2");
}

#[test]
fn parse_endpoint_event() {
    let events = parse_sse_body("event: endpoint\ndata: /messages?sessionId=abc\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("endpoint"));
    assert_eq!(events[0].data, "/messages?sessionId=abc");
}

#[test]
fn incremental_chunks() {
    let mut p = SseParser::new();
    p.push("event: mes");
    assert!(p.take_events().is_empty());
    p.push("sage\ndata: {\"a\":1}");
    assert!(p.take_events().is_empty());
    p.push("\n\n");
    let ev = p.take_events();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].data, r#"{"a":1}"#);
}

#[test]
fn ignores_comments() {
    let events = parse_sse_body(": keep-alive\n\ndata: hi\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "hi");
}

#[test]
fn field_value_space_stripping() {
    let events = parse_sse_body("data:  spaced\n\n");
    assert_eq!(events[0].data, " spaced");
}

#[test]
fn crlf_id_retry_unknown_and_colonless_fields() {
    let events = parse_sse_body(concat!(
        "id: 7\r\n",
        "retry: 2000\r\n",
        "foo: ignored\r\n",
        "data\r\n",
        "\r\n",
        "id:\r\n",
        "data: after-clear\r\n",
        "\r\n",
    ));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id.as_deref(), Some("7"));
    assert_eq!(events[0].data, "");
    assert_eq!(events[1].id, None);
    assert_eq!(events[1].data, "after-clear");
}

#[test]
fn trailing_event_without_blank_line_is_flushed() {
    let events = parse_sse_body("event: message\ndata: {\"ok\":true}\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("message"));
    assert_eq!(events[0].data, r#"{"ok":true}"#);
}

#[test]
fn id_only_blank_line_still_dispatches() {
    let events = parse_sse_body("id: abc\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id.as_deref(), Some("abc"));
    assert!(events[0].data.is_empty());
}

#[test]
fn empty_blank_line_does_not_dispatch_and_colonless_field() {
    let events = parse_sse_body("\n\nretry: 1\nunknown\ndata: x\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "x");
    let events = parse_sse_body("event: only\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("only"));
    assert!(events[0].data.is_empty());
    let mut p = SseParser::new();
    p.push("data: a\n");
    assert!(p.take_events().is_empty());
    p.push("\n");
    assert_eq!(p.take_events()[0].data, "a");
}
