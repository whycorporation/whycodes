use super::*;

#[test]
fn test_gpt4o() {
    let caps = detect_capabilities("openai", "gpt-4o");
    assert!(caps.tools);
    assert!(caps.vision);
    assert!(!caps.thinking);
    assert_eq!(caps.context_window, 128_000);
}

#[test]
fn test_claude_sonnet_4() {
    let caps = detect_capabilities("anthropic", "claude-sonnet-4-20250514");
    assert!(caps.tools);
    assert!(caps.vision);
    assert!(caps.thinking);
    assert!(caps.caching);
    assert_eq!(caps.context_window, 200_000);
}

#[test]
fn test_deepseek_r1() {
    let caps = detect_capabilities("deepseek", "deepseek-r1");
    assert!(!caps.tools);
    assert!(caps.thinking);
}

#[test]
fn test_unknown_model() {
    let caps = detect_capabilities("openai", "some-new-model");
    assert!(caps.tools); // falls back to provider heuristic
    assert_eq!(caps.context_window, 128_000);
}

#[test]
fn test_grok_catalog() {
    let caps = detect_capabilities("xai", "grok-4");
    assert_eq!(caps.context_window, 256_000);
    assert!(caps.thinking);
    let caps = detect_capabilities("xai", "grok-3");
    assert_eq!(caps.context_window, 131_072);
}

#[test]
fn gpt5_and_claude_4_think() {
    assert!(detect_capabilities("openai", "gpt-5").thinking);
    assert!(detect_capabilities("openai", "gpt-5-mini").thinking);
    assert!(detect_capabilities("anthropic", "claude-opus-4-1").thinking);
    assert!(detect_capabilities("anthropic", "claude-4-sonnet").thinking);
}

#[test]
fn resolve_prefers_configured_over_catalog() {
    let n = resolve_context_window(
        "anthropic",
        "claude-sonnet-4",
        Some(50_000),
        Some(1_050_000),
        200_000,
    );
    assert_eq!(n, 50_000);
}

#[test]
fn resolve_prefers_api_over_builtin() {
    // Built-in would guess ~128k for unknown; API says 1.05M.
    let n = resolve_context_window("custom", "auto/best-coding", None, Some(1_050_000), 200_000);
    assert_eq!(n, 1_050_000);
}

#[test]
fn resolve_uses_catalog_when_unconfigured() {
    let n = resolve_context_window("openai", "gpt-4o", None, None, 200_000);
    assert_eq!(n, 128_000);
    let n = resolve_context_window("google", "gemini-2.5-pro", None, None, 200_000);
    assert_eq!(n, 1_000_000);
}

#[test]
fn resolve_zero_configured_falls_through() {
    let n = resolve_context_window("openai", "gpt-4o", Some(0), None, 200_000);
    assert_eq!(n, 128_000);
}

#[test]
fn resolve_unknown_model_uses_heuristic_default() {
    assert_eq!(
        resolve_context_window("vendor", "mystery", None, None, 4096),
        128_000
    );
}

#[test]
fn openai_family_catalog_entries() {
    let g45 = detect_capabilities("openai", "GPT-4.5-Preview");
    assert!(g45.thinking && g45.vision && g45.context_window == 128_000);

    let turbo = detect_capabilities("openai", "gpt-4-turbo-2024");
    assert!(turbo.tools && !turbo.thinking && turbo.context_window == 128_000);

    let k32 = detect_capabilities("openai", "gpt-4-32k");
    assert_eq!(k32.context_window, 32_000);

    let legacy = detect_capabilities("openai", "gpt-3.5-turbo");
    assert!(legacy.tools && !legacy.vision && legacy.context_window == 16_384);

    let o1 = detect_capabilities("openai", "o1-mini");
    assert!(o1.thinking && !o1.vision && o1.context_window == 200_000);

    let o4 = detect_capabilities("openai", "o4-mini");
    assert!(o4.thinking && o4.vision && o4.context_window == 200_000);
}

#[test]
fn anthropic_family_catalog_entries() {
    let haiku4 = detect_capabilities("anthropic", "claude-haiku-4-5");
    assert!(haiku4.tools && haiku4.caching && !haiku4.thinking);

    let c37 = detect_capabilities("anthropic", "claude-3-7-sonnet-latest");
    assert!(c37.thinking && c37.caching);

    let c35 = detect_capabilities("anthropic", "claude-3.5-haiku-latest");
    assert!(!c35.thinking && c35.caching);

    let opus3 = detect_capabilities("anthropic", "claude-3-opus-20240229");
    assert!(opus3.vision && !opus3.caching && !opus3.thinking);
    assert_eq!(opus3.context_window, 200_000);
}

#[test]
fn grok_family_catalog_entries() {
    let g4 = detect_capabilities("xai", "grok-4-fast");
    assert!(g4.thinking && g4.vision && g4.context_window == 256_000);

    let g3 = detect_capabilities("xai", "grok-3-beta");
    assert!(!g3.thinking && g3.context_window == 131_072);

    let g3mini = detect_capabilities("xai", "grok-3-mini-reasoner");
    assert!(g3mini.thinking);

    let g2v = detect_capabilities("xai", "grok-2-vision");
    assert!(g2v.vision && !g2v.thinking);

    let bare = detect_capabilities("xai", "grok-beta");
    assert_eq!(bare.context_window, 131_072);
}

#[test]
fn deepseek_and_gemini_catalog_entries() {
    let v3 = detect_capabilities("deepseek", "deepseek-v3");
    assert!(v3.tools && !v3.thinking);

    let chat = detect_capabilities("deepseek", "deepseek-chat");
    assert!(chat.vision);

    let p25 = detect_capabilities("google", "gemini-2.5-pro");
    assert!(p25.thinking && p25.context_window == 1_000_000);

    let f20 = detect_capabilities("google", "gemini-2.0-flash");
    assert!(!f20.thinking && f20.context_window == 1_000_000);

    let p15 = detect_capabilities("google", "gemini-1.5-pro");
    assert_eq!(p15.context_window, 2_000_000);

    let f15 = detect_capabilities("google", "gemini-1.5-flash");
    assert_eq!(f15.context_window, 1_000_000);
}

#[test]
fn mistral_family_catalog_entries() {
    let large = detect_capabilities("mistral", "mistral-large-latest");
    assert!(large.tools && large.context_window == 128_000);

    let pix = detect_capabilities("mistral", "pixtral-large");
    assert!(pix.vision);

    let code = detect_capabilities("mistral", "codestral-latest");
    assert_eq!(code.context_window, 32_000);
}

#[test]
fn heuristics_cover_models_missing_from_the_catalog() {
    let cases = [
        ("gateway", "custom-gpt-4-32k-endpoint", 32_000),
        ("google", "gemini-flash-experimental", 128_000),
        ("google-antigravity", "gemini-3.1-pro", 128_000),
        ("anthropic", "claude-future", 200_000),
        ("deepseek", "deepseek-coder-v2", 64_000),
        ("mistral", "mistral-medium-latest", 32_000),
        ("xai", "unknown-xai-model", 131_072),
        ("vendor", "totally-mystery", 128_000),
    ];
    for (provider, model, want) in cases {
        assert_eq!(
            detect_capabilities(provider, model).context_window,
            want,
            "{model}"
        );
    }
    assert!(detect_capabilities("vendor", "llava-vision").vision);
    assert!(detect_capabilities("ollama", "plain-model").tools);
    assert!(!detect_capabilities("vendor", "plain-model").tools);
    assert_eq!(
        detect_capabilities("google", "gemini-2.5-experimental").context_window,
        1_000_000
    );
    assert_eq!(ModelCapabilities::default().context_window, 128_000);
    assert_eq!(
        heuristic_context_window_for_tests("grok-4-custom", "xai"),
        256_000
    );
    assert_eq!(
        heuristic_context_window_for_tests("grok-custom", "xai"),
        131_072
    );
    assert_eq!(
        heuristic_context_window_for_tests("deepseek-r1-distill", "deepseek"),
        128_000
    );
    assert_eq!(
        heuristic_context_window_for_tests("gpt-4o-mini-custom", "openai"),
        128_000
    );
    assert_eq!(
        heuristic_context_window_for_tests("my-gpt-4-endpoint", "openai"),
        8_192
    );
    assert_eq!(
        heuristic_context_window_for_tests("gemini-2.0-custom", "google"),
        1_000_000
    );
    assert_eq!(
        resolve_context_window("vendor", "mystery", None, None, 0),
        128_000
    );
    assert_eq!(
        resolve_context_window("vendor", "__force_zero_window__", None, None, 0),
        1,
        "fallback.max(1) when heuristic window is 0"
    );
    assert_eq!(
        resolve_context_window("vendor", "__force_zero_window__", None, None, 8),
        8
    );
}
