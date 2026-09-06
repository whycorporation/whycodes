use super::*;

#[test]
fn sdk_error_constructors() {
    let e = SdkError::new(ErrorCode::Internal, "boom");
    assert_eq!(e.code, ErrorCode::Internal);
    assert_eq!(e.message, "boom");
    assert!(e.source.is_none());
    assert!(e.to_string().contains("internal"));

    let e = SdkError::with_source(ErrorCode::Disconnected, "nope", std::io::Error::other("io"));
    assert_eq!(e.code, ErrorCode::Disconnected);
    assert!(e.source.is_some());
}

#[tokio::test]
async fn reqwest_error_timeout_maps_to_timeout_code() {
    let err = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap()
        .get("http://127.0.0.1:1/")
        .send()
        .await
        .unwrap_err();
    let wrapped = SdkError::from(err);
    assert!(
        matches!(wrapped.code, ErrorCode::Timeout | ErrorCode::Disconnected),
        "{wrapped:?}"
    );
}
