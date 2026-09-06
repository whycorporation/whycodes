use super::*;

#[test]
fn latest_url_reads_current_env() {
    let url = latest_release_url();
    assert!(url.starts_with("http"));
    assert!(url.contains("github.com") || url.contains("127.0.0.1") || url.contains("localhost"));
}
