#[test]
fn lsp_module_loads() {
    assert!(!module_path!().is_empty());
}
