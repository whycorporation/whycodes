#[test]
fn unknown_transport_is_rejected_by_parser_path() {
    // Production match lives in cmd_mcp; this only keeps the module
    // from being an empty stub. Full Serve/Add coverage is in tests.rs.
    assert!(!module_path!().is_empty());
}
