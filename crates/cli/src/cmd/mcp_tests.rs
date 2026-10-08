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
    assert!(missing_mcp_endpoint(None, None));
    assert!(!missing_mcp_endpoint(Some("https://x"), None));
    assert!(!missing_mcp_endpoint(None, Some("npx")));
    let empty = mcp_empty_lines();
    assert!(empty.iter().any(|l| l.contains("No MCP servers")));
    assert!(
        empty
            .iter()
            .any(|l| l.contains("streamable") || l.contains("--url"))
    );
    assert!(mcp_configured_header().contains("Configured"));
}

#[tokio::test]
async fn cmd_mcp_list_add_remove_roundtrip() {
    let _home = crate::cmd::helpers::IsolatedHome::new();
    cmd_mcp(&McpCmd::List).await.unwrap();
    let err = cmd_mcp(&McpCmd::Add {
        name: "fs".into(),
        command: None,
        args: None,
        url: None,
        transport: None,
        headers: vec![],
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("command") || err.to_string().contains("url"));
    let err = cmd_mcp(&McpCmd::Add {
        name: "fs".into(),
        command: Some("npx".into()),
        args: None,
        url: Some("https://example.test/mcp".into()),
        transport: None,
        headers: vec![],
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not both"));

    cmd_mcp(&McpCmd::Add {
        name: "fs".into(),
        command: Some("npx".into()),
        args: Some("-y pkg".into()),
        url: None,
        transport: Some("stdio".into()),
        headers: vec![],
    })
    .await
    .unwrap();
    cmd_mcp(&McpCmd::Add {
        name: "remote".into(),
        command: None,
        args: None,
        url: Some("https://example.test/mcp".into()),
        transport: Some("http".into()),
        headers: vec!["Authorization: Bearer x".into()],
    })
    .await
    .unwrap();
    cmd_mcp(&McpCmd::List).await.unwrap();
    cmd_mcp(&McpCmd::Remove {
        name: "missing".into(),
    })
    .await
    .unwrap();
    cmd_mcp(&McpCmd::Remove { name: "fs".into() })
        .await
        .unwrap();
    cmd_mcp(&McpCmd::Remove {
        name: "remote".into(),
    })
    .await
    .unwrap();
}

/// Linux lets a process delete its own cwd. `mcp serve` then falls back to
/// `.` instead of panicking. Windows refuses to remove the cwd, so this arm
/// is host-only (the coverage job is Linux).
#[cfg(unix)]
#[test]
fn mcp_serve_cwd_uses_dot_when_cwd_is_gone() {
    let prev = std::env::current_dir().expect("cwd");
    let dir = tempfile::tempdir().expect("tempdir");
    let gone = dir.path().join("missing-cwd");
    std::fs::create_dir(&gone).unwrap();
    std::env::set_current_dir(&gone).unwrap();
    std::fs::remove_dir(&gone).unwrap();
    let cwd = mcp_serve_cwd();
    if let Err(err) = std::env::set_current_dir(&prev) {
        panic!("restore cwd: {err}");
    }
    assert_eq!(cwd, ".");
}
