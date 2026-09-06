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
    let search = memory_search_line(0.5, "abcdefghij", "hit");
    assert!(search.contains("[0.50]"), "{search}");
    assert!(search.contains("abcdefgh"), "{search}");
    assert!(search.contains("hit"), "{search}");
    assert_eq!(
        memory_session_hit_line(0.25, "sessionid", 3),
        "  [0.25] sessioni turn 3"
    );
    assert_eq!(
        memory_code_hit_line(0.9, "src/lib.rs", 1, 4),
        "  [0.90] src/lib.rs:1-4"
    );
    assert_eq!(
        memory_import_summary(3, 1),
        "Import complete: 3 added, 1 skipped"
    );
}
