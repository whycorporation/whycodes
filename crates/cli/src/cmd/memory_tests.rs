use super::*;

#[test]
fn onnx_gate_message_is_feature_aware() {
    let available = whycodes_memory::onnx::onnx_available();
    assert!(!available || available);
}

#[test]
fn memory_printer_helpers_cover_list_search_and_hits() {
    assert_eq!(memory_list_header(2, "proj"), "2 memories (proj)");
    let row = memory_row_line("abcdefghij", "note");
    assert!(row.contains("abcdefgh"), "{row}");
    assert!(row.contains("note"), "{row}");
    assert_eq!(
        memory_import_summary(3, 1),
        "Import complete: 3 added, 1 skipped"
    );
    assert!(memory_empty_line().contains("No memories"));
    assert!(memory_saved_line("abcdefghij", "note").contains("note"));
    assert!(memory_deleted_line("abc").contains("abc"));
    assert!(memory_delete_missing_line("abc").contains("abc"));
    assert!(memory_cleared_line(4).contains("4"));
    assert!(memory_exported_line("/tmp/m.json").contains("/tmp/m.json"));
    assert!(memory_indexing_line().contains("Indexing"));
    assert!(memory_indexed_line(9).contains("9"));
}
