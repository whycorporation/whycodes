use super::*;

#[test]
fn message_io_json_display() {
    assert_eq!(LspError::msg("no stdin").to_string(), "no stdin");
    let io = LspError::from(std::io::Error::other("pipe"));
    assert!(io.to_string().contains("pipe"));
    let json = LspError::from(serde_json::from_str::<u8>("x").unwrap_err());
    assert!(!json.to_string().is_empty());
}
