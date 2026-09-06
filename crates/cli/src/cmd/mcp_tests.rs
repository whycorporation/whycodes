use super::*;

#[test]
fn unknown_transport_is_rejected_by_parser_path() {
    assert!(parse_mcp_transport(Some("bogus")).is_err());
}

#[test]
fn mcp_transport_and_header_helpers() {
    assert!(parse_mcp_transport(None).unwrap().is_none());
    assert!(matches!(
        parse_mcp_transport(Some("stdio")).unwrap(),
        Some(whycodes_config::McpTransportKind::Stdio)
    ));
    assert!(matches!(
        parse_mcp_transport(Some("local")).unwrap(),
        Some(whycodes_config::McpTransportKind::Stdio)
    ));
    assert!(matches!(
        parse_mcp_transport(Some("http")).unwrap(),
        Some(whycodes_config::McpTransportKind::Http)
    ));
    assert!(matches!(
        parse_mcp_transport(Some("streamable-http")).unwrap(),
        Some(whycodes_config::McpTransportKind::Http)
    ));
    assert!(matches!(
        parse_mcp_transport(Some("remote")).unwrap(),
        Some(whycodes_config::McpTransportKind::Http)
    ));
    assert!(matches!(
        parse_mcp_transport(Some("sse")).unwrap(),
        Some(whycodes_config::McpTransportKind::Sse)
    ));
    assert!(matches!(
        parse_mcp_transport(Some("auto")).unwrap(),
        Some(whycodes_config::McpTransportKind::Auto)
    ));

    assert!(parse_mcp_headers(&[]).unwrap().is_none());
    let map = parse_mcp_headers(&["Auth: Bearer x".into(), "X-A: 1".into()]).unwrap();
    let map = map.expect("headers");
    assert_eq!(map.get("Auth").map(String::as_str), Some("Bearer x"));
    assert_eq!(map.get("X-A").map(String::as_str), Some("1"));
    assert!(parse_mcp_headers(&["no-colon".into()]).is_err());
}

#[test]
fn mcp_printer_helpers() {
    assert!(mcp_remote_line("fs", "http", "https://x").contains("https://x"));
    assert!(mcp_stdio_line("fs", "npx", &[]).contains("npx"));
    assert!(mcp_stdio_line("fs", "npx", &["-y".into(), "pkg".into()]).contains("pkg"));
    assert!(mcp_saved_remote_line("fs", "https://x").contains("remote"));
    assert!(mcp_saved_stdio_line("fs", "npx", "-y pkg").contains("stdio"));
    assert!(mcp_removed_line("fs").contains("removed"));
    assert!(mcp_not_found_line("fs").contains("not found"));
}
