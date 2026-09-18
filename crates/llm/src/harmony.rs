//! Harmony-dialect capability: which provider/model pairs need leak guards.
//!
//! Scanner + escape live in `whycodes_core::harmony`. This module only decides
//! when the agent loop and request encoder should arm them.

/// Whether this provider/model pair speaks Harmony (or Codex Responses).
///
/// Keyed by family, not a frozen three-id table: `gpt-5*`, `*-codex`,
/// provider `openai-codex`, and the ChatGPT Codex Responses path.
pub fn uses_harmony_dialect(provider: &str, model: &str) -> bool {
    let p = provider.to_ascii_lowercase();
    let m = model.to_ascii_lowercase();
    if p == "openai-codex" || p.contains("codex") {
        return true;
    }
    if p == "openai" || p == "openrouter" || p == "azure" || p == "custom" {
        return m.contains("gpt-5") || m.contains("codex");
    }
    m.contains("gpt-5") || m.contains("-codex") || m.contains("codex-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt5_family_and_codex_are_harmony() {
        assert!(uses_harmony_dialect("openai", "gpt-5"));
        assert!(uses_harmony_dialect("openai", "gpt-5.3-codex"));
        assert!(uses_harmony_dialect("openai", "o4-mini-codex"));
        assert!(uses_harmony_dialect("openai-codex", "anything"));
        assert!(uses_harmony_dialect("openrouter", "openai/gpt-5-mini"));
    }

    #[test]
    fn claude_and_grok_are_not() {
        assert!(!uses_harmony_dialect("anthropic", "claude-sonnet-4"));
        assert!(!uses_harmony_dialect("xai", "grok-4"));
        assert!(!uses_harmony_dialect("openai", "gpt-4o"));
    }

    #[test]
    fn family_matches_azure_custom_and_codex_suffix() {
        assert!(uses_harmony_dialect("azure", "gpt-5-mini"));
        assert!(uses_harmony_dialect("custom", "my-codex"));
        assert!(uses_harmony_dialect("together", "foo-codex"));
        assert!(uses_harmony_dialect("xai", "codex-mini"));
        assert!(uses_harmony_dialect("acme-codex", "any"));
        assert!(!uses_harmony_dialect("together", "llama-3"));
    }
}
