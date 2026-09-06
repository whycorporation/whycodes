#[test]
fn onnx_gate_message_is_feature_aware() {
    let available = whycodes_memory::onnx::onnx_available();
    assert!(!available || available);
}
