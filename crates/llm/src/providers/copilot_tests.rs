use super::*;
use crate::provider::LlmProvider;

#[test]
fn from_base_blank_keeps_cloud_and_override_normalizes() {
    let cloud = CopilotProvider::from_base(Some("   "));
    assert!(cloud.default_base_url().contains("githubcopilot.com"));
    let local = CopilotProvider::from_base(Some("http://127.0.0.1:9/v1"));
    let local_url = local.default_base_url();
    assert!(local_url.ends_with("/chat/completions"), "{local_url}");
    assert_eq!(CopilotProvider::default().name(), "github-copilot");
}
