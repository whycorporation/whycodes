//! Built-in LLM provider implementations.

pub mod anthropic;
pub mod antigravity;
pub mod codeassist;
pub mod codex;
pub mod copilot;
pub mod custom;
pub mod deepseek;
pub mod google;
pub mod groq;
pub mod mistral;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub mod together;
pub mod xai;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;
    use whycodes_core::types::{LlmRequest, Message, MessageContent, Role, ToolDefinition};

    fn req_with_tools() -> LlmRequest {
        LlmRequest {
            system: "sys".into(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: vec![ToolDefinition {
                name: "read".into(),
                description: "read a file".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            max_tokens: Some(32),
            temperature: Some(0.2),
            top_p: Some(0.9),
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    fn assert_openai_compat_body(body: &serde_json::Value, model: &str) {
        assert_eq!(body["model"], model);
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 32);
        assert!(body["tools"].is_array(), "{body}");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
        assert!(body["temperature"].is_number(), "{body}");
        assert!(body["top_p"].is_number(), "{body}");
        assert!(body["messages"].is_array(), "{body}");
    }

    #[test]
    fn openai_compat_wrappers_build_body_and_name_their_host() {
        let req = req_with_tools();
        let groq = groq::GroqProvider::new();
        let mistral = mistral::MistralProvider::new();
        let together = together::TogetherProvider::new();
        let xai = xai::XaiProvider::new();
        let deepseek = deepseek::DeepSeekProvider::new();
        let cases = [
            (
                groq.name(),
                groq.default_base_url(),
                groq.build_body(&req, "llama"),
                "groq",
                "https://api.groq.com/openai/v1/chat/completions",
                "llama",
            ),
            (
                mistral.name(),
                mistral.default_base_url(),
                mistral.build_body(&req, "mistral-small"),
                "mistral",
                "https://api.mistral.ai/v1/chat/completions",
                "mistral-small",
            ),
            (
                together.name(),
                together.default_base_url(),
                together.build_body(&req, "together-model"),
                "together",
                "https://api.together.xyz/v1/chat/completions",
                "together-model",
            ),
            (
                xai.name(),
                xai.default_base_url(),
                xai.build_body(&req, "grok"),
                "xai",
                "https://api.x.ai/v1/chat/completions",
                "grok",
            ),
            (
                deepseek.name(),
                deepseek.default_base_url(),
                deepseek.build_body(&req, "deepseek-chat"),
                "deepseek",
                "https://api.deepseek.com/v1/chat/completions",
                "deepseek-chat",
            ),
        ];
        for (name, url, body, exp_name, exp_url, model) in cases {
            assert_eq!(name, exp_name);
            assert_eq!(url, exp_url);
            assert_openai_compat_body(&body, model);
        }
    }

    #[test]
    fn ollama_body_uses_options_and_system_message() {
        let p = ollama::OllamaProvider::new();
        assert_eq!(p.name(), "ollama");
        let body = p.build_body(&req_with_tools(), "llama3");
        assert_eq!(body["model"], "llama3");
        assert_eq!(body["options"]["num_predict"], 32);
        assert!(body["options"]["temperature"].is_number());
        assert!(body["options"]["top_p"].is_number());
        assert!(body["tools"].is_array());
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["role"].as_str())
            .collect();
        assert!(roles.contains(&"system"), "{roles:?}");
        assert!(roles.contains(&"user"), "{roles:?}");
        assert!(
            p.default_base_url().ends_with("/api/chat"),
            "{}",
            p.default_base_url()
        );
    }

    #[test]
    fn google_body_and_urls() {
        let p = google::GoogleProvider::new();
        assert_eq!(p.name(), "google");
        let req = req_with_tools();
        let body = p.build_body(&req);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 32);
        assert!(body["tools"].is_array());

        let stream = p.build_url("gemini-2.0-flash", "k");
        assert!(stream.contains("streamGenerateContent"), "{stream}");
        assert!(stream.contains("key=k"), "{stream}");
        let complete = p.build_complete_url("gemini-2.0-flash", "k");
        assert!(complete.contains("generateContent"), "{complete}");
        assert!(!complete.contains("streamGenerateContent"), "{complete}");
    }

    #[test]
    fn copilot_identity() {
        let p = copilot::CopilotProvider::new();
        assert_eq!(p.name(), "github-copilot");
        assert_eq!(
            p.default_base_url(),
            "https://api.githubcopilot.com/chat/completions"
        );
    }

    #[test]
    fn antigravity_identity() {
        let p = antigravity::AntigravityProvider::new();
        assert_eq!(p.name(), "google-antigravity");
        assert_eq!(
            p.default_base_url(),
            "https://daily-cloudcode-pa.googleapis.com/v1internal"
        );
    }
}
