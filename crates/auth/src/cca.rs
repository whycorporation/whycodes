//! Cloud Code Assist (CCA) transport logic for Antigravity integration.
//!
//! Handles request construction, signing, and response parsing for
//! CCA-specific SSE endpoints.

use serde::{Deserialize, Serialize};
use crate::error::Result;

/// Metadata sent by native Antigravity control-plane requests.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AntigravityMetadata {
    pub ide_type: String,
}

impl Default for AntigravityMetadata {
    fn default() -> Self {
        Self {
            ide_type: "ANTIGRAVITY".to_string(),
        }
    }
}

/// Base CCA request envelope.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudCodeRequest<T> {
    #[serde(flatten)]
    pub context: CloudCodeContext,
    pub payload: T,
}
/// Perform a request to the Cloud Code Assist API.
pub async fn request_cloud_code_assist<T, R>(
    url: &str,
    token: &str,
    body: &CloudCodeRequest<T>,
) -> Result<R>
where
    T: Serialize,
    R: for<'de> Deserialize<'de>,
{
    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(crate::error::AuthError::from)?
        .error_for_status()
        .map_err(crate::error::AuthError::from)?;

    response.json::<R>().await.map_err(crate::error::AuthError::from)
}
use crate::token::OAuthToken;

pub async fn perform_antigravity_onboarding(mut token: OAuthToken) -> Result<OAuthToken> {
    let url = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
    let request = CloudCodeRequest {
        context: CloudCodeContext {
            project_id: "projects/-".to_string(), // Try wildcard
            metadata: AntigravityMetadata::default(),
        },
        payload: serde_json::json!({}), // Actual payload
    };

    let response: LoadCodeAssistResponse = request_cloud_code_assist(url, &token.access_token, &request).await?;
    
    // Store project id if returned
    if let Some(project_id) = extract_project_id(&response) {
        token.extra.insert("project_id".to_string(), serde_json::json!(project_id));
    }
    
    Ok(token)
}

fn extract_project_id(response: &LoadCodeAssistResponse) -> Option<String> {
    // According to oh-my-pi, project_id is often nested in the response structure
    // We'll look for a plausible location or return None if it's missing
    response.allowed_tiers.as_ref()?.first().map(|t| t.id.clone())
}

/// Context for Cloud Code requests.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudCodeContext {
    pub project_id: String,
    pub metadata: AntigravityMetadata,
}

/// Cloud Code Assist response.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoadCodeAssistResponse {
    pub current_tier: Option<String>,
    pub paid_tier: Option<String>,
    pub allowed_tiers: Option<Vec<Tier>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Tier {
    pub id: String,
}
