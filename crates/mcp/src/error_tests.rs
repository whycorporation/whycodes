use super::*;

#[test]
fn message_io_json_display() {
    assert_eq!(McpError::msg("no stdin").to_string(), "no stdin");
    let io = McpError::from(std::io::Error::other("pipe"));
    assert!(io.to_string().contains("pipe"));
    let json = McpError::from(serde_json::from_str::<u8>("x").unwrap_err());
    assert!(!json.to_string().is_empty());
}

#[tokio::test]
async fn http_error_conversion() {
    let err = reqwest::Client::new()
        .get("http://127.0.0.1:1/")
        .send()
        .await
        .unwrap_err();
    let wrapped = McpError::from(err);
    assert!(!wrapped.to_string().is_empty());
}
