use super::*;

fn on() -> MagicKeywordsConfig {
    MagicKeywordsConfig::default()
}

#[test]
fn hits_standalone_lowercase_words() {
    let hit = scan("please ultrathink about this", &on());
    assert!(hit.ultrathink);
    assert!(!hit.orchestrate);
    assert!(hit.any());

    let hit = scan("orchestrate the migration, then stop", &on());
    assert!(hit.orchestrate);
    assert!(!hit.ultrathink);
}

#[test]
fn both_keywords_in_one_prompt() {
    let hit = scan("ultrathink then orchestrate the rollout", &on());
    assert!(hit.ultrathink && hit.orchestrate);
    let notice = hit.notice();
    assert!(notice.contains("ultrathink"));
    assert!(notice.contains("orchestrate"));
}

#[test]
fn ignores_identifiers_paths_and_calls() {
    for sample in [
        "Ultrathink about this",
        "orchestrated the change",
        "see orchestrate.ts",
        "foo::orchestrate",
        "call orchestrate()",
        "path/ultrathink/file",
        "ultrathink-mode",
    ] {
        let hit = scan(sample, &on());
        assert!(!hit.any(), "should not match: {sample}");
    }
}

#[test]
fn ignores_code_spans_and_fences() {
    assert!(!scan("use `ultrathink` here", &on()).any());
    assert!(!scan("```\nultrathink\n```\nok", &on()).ultrathink);
    assert!(!scan("~~~\norchestrate\n~~~\n", &on()).orchestrate);
    assert!(scan("```\ncode\n```\nultrathink", &on()).ultrathink);
    assert!(!scan("<note>ultrathink</note>", &on()).ultrathink);
}

#[test]
fn punctuation_may_touch_the_word() {
    assert!(scan("ultrathink.", &on()).ultrathink);
    assert!(scan("\"orchestrate\"", &on()).orchestrate);
    assert!(scan("(ultrathink)", &on()).ultrathink);
}

#[test]
fn config_switches_disable_notices() {
    let mut cfg = on();
    cfg.enabled = false;
    assert!(!scan("ultrathink please", &cfg).any());

    cfg = on();
    cfg.ultrathink = false;
    let hit = scan("ultrathink and orchestrate", &cfg);
    assert!(!hit.ultrathink);
    assert!(hit.orchestrate);
}

#[test]
fn empty_notice_when_nothing_matched() {
    assert!(MagicHit::default().notice().is_empty());
    assert!(!MagicHit::default().any());
}

#[test]
fn unclosed_html_and_short_fence_are_safe() {
    assert!(!scan("<note>ultrathink", &on()).ultrathink);
    // Two backticks is an inline span, not a fence (`fence_open` needs 3).
    assert!(!scan("`ultrathink`", &on()).ultrathink);
    assert!(scan("please ultrathink\n~~x", &on()).ultrathink);
    // Unclosed 3-tick fence still masks the keyword until EOF.
    assert!(!scan("```\nultrathink", &on()).ultrathink);
    // 4-tick open vs 3-tick close: `fence_close` needs `i + run <= len`.
    assert!(!scan("````\nultrathink\n```", &on()).ultrathink);
    assert!(!scan("please <foo ultrathink", &on()).ultrathink);
    assert!(scan("please ultrathink <foo", &on()).ultrathink);
}
