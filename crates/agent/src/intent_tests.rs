use super::*;

#[test]
fn explains_as_question() {
    let a = classify_user_intent("How does session compaction work?");
    assert_eq!(a.intent, UserIntent::Question);
    assert!(a.is_high(), "{a:?}");
}

#[test]
fn turkish_question() {
    let a = classify_user_intent("Compaction nasıl çalışıyor?");
    assert_eq!(a.intent, UserIntent::Question);
}

#[test]
fn fix_is_change() {
    let a = classify_user_intent("Fix the auth bug in session.rs");
    assert_eq!(a.intent, UserIntent::Change);
    assert!(a.confidence >= 0.5, "{a:?}");
}

#[test]
fn turkish_fix() {
    let a = classify_user_intent("Auth bug'ını düzelt");
    assert_eq!(a.intent, UserIntent::Change);
}

#[test]
fn design_is_plan() {
    let a =
        classify_user_intent("Design the architecture for multi-tenant billing and write a plan");
    assert_eq!(a.intent, UserIntent::Plan);
    assert!(a.is_high(), "{a:?}");
}

#[test]
fn can_we_fix_is_not_blind_change() {
    // Clarification-shaped: question mark + change verb → question posture.
    let a = classify_user_intent("Can we fix the flaky test?");
    assert!(
        matches!(a.intent, UserIntent::Question | UserIntent::Ambiguous),
        "expected question-ish, got {a:?}"
    );
}

#[test]
fn create_roadmap_is_plan_not_change() {
    let a = classify_user_intent("Create a roadmap for provider parity");
    assert_eq!(a.intent, UserIntent::Plan, "{a:?}");
}

#[test]
fn best_approach_question_is_plan() {
    let a = classify_user_intent("What's the best approach for session resume?");
    assert_eq!(a.intent, UserIntent::Plan, "{a:?}");
}

#[test]
fn write_migration_plan_is_plan() {
    let a = classify_user_intent("Write a migration plan");
    assert_eq!(a.intent, UserIntent::Plan, "{a:?}");
}

#[test]
fn trivial_greeting() {
    let a = classify_user_intent("selam");
    assert_eq!(a.intent, UserIntent::Trivial);
}

#[test]
fn posture_none_for_ask_agent() {
    let a = classify_user_intent("How does X work?");
    assert!(posture_suffix(&a, "ask").is_none());
    assert!(posture_suffix(&a, "build").is_some());
}

#[test]
fn apply_mutates_request_not_empty() {
    let mut req = LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![whycodes_core::types::Message {
            role: Role::User,
            content: MessageContent::Text("How does auth work?".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    };
    let applied = apply_intent_to_request(
        &mut req,
        "How does auth work?",
        "build",
        IntentGuidanceMode::Auto,
    );
    assert!(applied.is_some());
    let text = req.messages[0].content.as_text().unwrap();
    assert!(text.contains("whycodes_intent"));
    assert!(text.contains("How does auth work?"));
}

#[test]
fn off_mode_skips() {
    let mut req = LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![whycodes_core::types::Message {
            role: Role::User,
            content: MessageContent::Text("How does auth work?".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    };
    assert!(
        apply_intent_to_request(
            &mut req,
            "How does auth work?",
            "build",
            IntentGuidanceMode::Off
        )
        .is_none()
    );
}

#[test]
fn parse_mode() {
    assert_eq!(IntentGuidanceMode::parse("auto"), IntentGuidanceMode::Auto);
    assert_eq!(IntentGuidanceMode::parse("off"), IntentGuidanceMode::Off);
    assert_eq!(
        IntentGuidanceMode::parse("always"),
        IntentGuidanceMode::Always
    );
}

#[test]
fn badge_for_high_question() {
    let a = classify_user_intent("How does session compaction work?");
    assert_eq!(badge_label(&a), Some("Q"));
}

#[test]
fn mismatch_toast_is_warning() {
    let a = classify_user_intent("Fix the auth bug in session.rs");
    let n = intent_notice(&a, "ask").expect("notice");
    assert_eq!(n.kind, IntentNoticeKind::Warning);
    assert!(n.message.contains("build"));
}

#[test]
fn read_only_shell_ls() {
    assert!(is_read_only_shell("ls -la"));
    assert!(is_read_only_shell("git status"));
    assert!(is_read_only_shell("rg foo src"));
    assert!(!is_read_only_shell("rm -rf target"));
    assert!(!is_read_only_shell("git push origin main"));
}

#[test]
fn authorize_blocks_edit_on_question_in_build() {
    let a = classify_user_intent("How does auth work?");
    let d = authorize_tool(&a, "build", "edit", None, IntentGuidanceMode::Auto);
    assert!(matches!(d, ToolAuthDecision::Confirm { .. }), "{d:?}");
}

#[test]
fn authorize_allows_ls_on_question() {
    let a = classify_user_intent("How does auth work?");
    let d = authorize_tool(
        &a,
        "build",
        "bash",
        Some("ls -la src"),
        IntentGuidanceMode::Auto,
    );
    assert_eq!(d, ToolAuthDecision::Allow);
}

#[test]
fn authorize_confirms_rm_on_question() {
    let a = classify_user_intent("How does auth work?");
    let d = authorize_tool(
        &a,
        "build",
        "bash",
        Some("rm -rf /tmp/x"),
        IntentGuidanceMode::Auto,
    );
    assert!(matches!(d, ToolAuthDecision::Confirm { .. }), "{d:?}");
}

#[test]
fn authorize_refuses_write_on_ask_agent() {
    let a = classify_user_intent("Fix the bug");
    let d = authorize_tool(&a, "ask", "write", None, IntentGuidanceMode::Auto);
    assert!(matches!(d, ToolAuthDecision::Refuse { .. }), "{d:?}");
}

#[test]
fn authorize_allows_change_intent_edits() {
    let a = classify_user_intent("Fix the auth bug in session.rs");
    let d = authorize_tool(&a, "build", "edit", None, IntentGuidanceMode::Auto);
    assert_eq!(d, ToolAuthDecision::Allow);
}

#[test]
fn authorize_off_skips() {
    let a = classify_user_intent("How does auth work?");
    let d = authorize_tool(&a, "build", "edit", None, IntentGuidanceMode::Off);
    assert_eq!(d, ToolAuthDecision::Allow);
}

#[test]
fn authorize_confirms_plan_write_without_high_confidence() {
    let a = IntentAssessment {
        intent: UserIntent::Plan,
        confidence: 0.5,
        reasons: vec!["plan_marker"],
    };
    let d = authorize_tool(&a, "build", "write", None, IntentGuidanceMode::Auto);
    assert!(matches!(d, ToolAuthDecision::Confirm { .. }), "{d:?}");
}

#[test]
fn authorize_always_confirms_ambiguous_write() {
    let a = IntentAssessment {
        intent: UserIntent::Ambiguous,
        confidence: 0.35,
        reasons: vec![],
    };
    let d = authorize_tool(&a, "build", "write", None, IntentGuidanceMode::Always);
    assert!(matches!(d, ToolAuthDecision::Confirm { .. }), "{d:?}");
}

fn req_with(content: MessageContent) -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![whycodes_core::types::Message {
            role: Role::User,
            content,
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    }
}

#[test]
fn always_injects_change_and_plan_posture() {
    let mut req = req_with(MessageContent::Text(
        "Fix the auth bug in session.rs".into(),
    ));
    let applied = apply_intent_to_request(
        &mut req,
        "Fix the auth bug in session.rs",
        "build",
        IntentGuidanceMode::Always,
    )
    .expect("change");
    assert_eq!(applied.intent, UserIntent::Change);
    assert!(
        req.messages[0]
            .content
            .as_text()
            .unwrap()
            .contains("implementation")
    );

    let mut req = req_with(MessageContent::Text(
        "Design the architecture for multi-tenant billing and write a plan".into(),
    ));
    let applied = apply_intent_to_request(
        &mut req,
        "Design the architecture for multi-tenant billing and write a plan",
        "build",
        IntentGuidanceMode::Always,
    )
    .expect("plan");
    assert_eq!(applied.intent, UserIntent::Plan);
    assert!(
        req.messages[0]
            .content
            .as_text()
            .unwrap()
            .contains("planning")
    );
}

#[test]
fn apply_appends_text_block_on_multimodal_user() {
    let mut req = req_with(MessageContent::Blocks(vec![
        whycodes_core::types::ContentBlock::Text {
            text: "How does auth work?".into(),
        },
    ]));
    let applied = apply_intent_to_request(
        &mut req,
        "How does auth work?",
        "build",
        IntentGuidanceMode::Auto,
    );
    assert!(applied.is_some());
    let MessageContent::Blocks(blocks) = &req.messages[0].content else {
        panic!("expected blocks");
    };
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            whycodes_core::types::ContentBlock::Text { text } if text.contains("whycodes_intent")
        )),
        "{blocks:?}"
    );
}

#[test]
fn qmark_without_change_and_bare_imperative() {
    let q = classify_user_intent("huh?");
    assert_eq!(q.intent, UserIntent::Question);
    let c = classify_user_intent("please");
    assert!(
        matches!(c.intent, UserIntent::Change | UserIntent::Ambiguous),
        "{c:?}"
    );
    let t = classify_user_intent("hi");
    assert_eq!(t.intent, UserIntent::Trivial);
    assert_eq!(t.confidence, 0.95);
}

#[test]
fn apply_skips_non_user_last_message() {
    let mut req = LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![whycodes_core::types::Message {
            role: Role::Assistant,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    };
    assert!(
        apply_intent_to_request(
            &mut req,
            "How does auth work?",
            "build",
            IntentGuidanceMode::Always
        )
        .is_none()
    );
}

#[test]
fn git_stash_list_is_read_only() {
    assert!(is_read_only_shell("git stash list"));
    assert!(is_read_only_shell("git stash show"));
    let a = classify_user_intent("How does auth work?");
    let d = authorize_tool(
        &a,
        "build",
        "bash",
        Some("git stash list"),
        IntentGuidanceMode::Auto,
    );
    assert_eq!(d, ToolAuthDecision::Allow);
    assert!(shell_head_readonly_ok("git stash list"));
    assert!(shell_head_readonly_ok("cargo metadata"));
    assert!(!shell_head_readonly_ok("cargo build"));
    assert!(!shell_head_readonly_ok("sed -i s/a/b/ file"));
    let plan =
        classify_user_intent("Design the architecture for multi-tenant billing and write a plan");
    assert_eq!(badge_label(&plan), Some("plan"));
    assert!(intent_notice(&plan, "build").is_some());
    assert!(intent_notice(&plan, "ask").is_some());
    let q = classify_user_intent("How does session compaction work?");
    assert!(intent_notice(&q, "build").is_some());
    assert_eq!(
        status_hint(&q, "build"),
        intent_notice(&q, "build").map(|n| n.message)
    );
    assert_eq!(UserIntent::Change.as_str(), "change");
    assert_eq!(UserIntent::Ambiguous.as_str(), "ambiguous");
    assert_eq!(UserIntent::Trivial.as_str(), "trivial");
    assert!(!should_inject(
        IntentGuidanceMode::Always,
        &classify_user_intent("selam")
    ));
    let change = classify_user_intent("Fix the auth bug in session.rs");
    assert_eq!(badge_label(&change), Some("chg"));
    assert!(
        posture_suffix(&change, "build")
            .unwrap()
            .contains("implementation")
    );
    let amb = IntentAssessment {
        intent: UserIntent::Ambiguous,
        confidence: 0.6,
        reasons: vec![],
    };
    assert!(posture_suffix(&amb, "build").unwrap().contains("ambiguous"));
    assert!(should_inject(IntentGuidanceMode::Always, &amb));
    assert!(should_inject(IntentGuidanceMode::Auto, &amb));
}

#[test]
fn qmark_without_thresholds_is_question_and_imperative_is_change() {
    let q = classify_user_intent("Ready?");
    assert_eq!(q.intent, UserIntent::Question, "{q:?}");
    let change = classify_user_intent("Ship the patch");
    assert!(
        matches!(change.intent, UserIntent::Change | UserIntent::Ambiguous),
        "{change:?}"
    );
    let impl_change = classify_user_intent("implement retries");
    assert_eq!(impl_change.intent, UserIntent::Change, "{impl_change:?}");
    assert!(should_inject(IntentGuidanceMode::Always, &impl_change));
    assert!(!should_inject(
        IntentGuidanceMode::Auto,
        &classify_user_intent("selam")
    ));
    assert!(!should_inject(
        IntentGuidanceMode::Auto,
        &IntentAssessment {
            intent: UserIntent::Ambiguous,
            confidence: 0.2,
            reasons: vec![],
        }
    ));
}

#[test]
fn read_only_shell_empty_segments_and_git_config() {
    assert!(is_read_only_shell(""));
    assert!(is_read_only_shell("ls ; ; pwd"));
    assert!(is_read_only_shell("git config --get user.name"));
    assert!(is_read_only_shell("git config -l"));
    assert!(is_read_only_shell("git config --list"));
    assert!(is_read_only_shell("git foo"));
    assert!(!shell_head_readonly_ok("git foo"));
}

#[test]
fn off_mode_and_trivial_posture_skip_inject() {
    let change = classify_user_intent("Fix the auth bug in session.rs");
    assert!(!should_inject(IntentGuidanceMode::Off, &change));
    let mut req = req_with(MessageContent::Text(
        "Fix the auth bug in session.rs".into(),
    ));
    assert!(
        apply_intent_to_request(
            &mut req,
            "Fix the auth bug in session.rs",
            "build",
            IntentGuidanceMode::Off
        )
        .is_none()
    );
    let trivial = IntentAssessment {
        intent: UserIntent::Trivial,
        confidence: 0.95,
        reasons: vec!["trivial_or_empty"],
    };
    assert!(posture_suffix(&trivial, "build").is_none());
    assert!(!should_inject(IntentGuidanceMode::Always, &trivial));
    assert!(!should_inject(IntentGuidanceMode::Auto, &trivial));
    let mut req = req_with(MessageContent::Text("selam".into()));
    assert!(
        apply_intent_to_request(&mut req, "selam", "build", IntentGuidanceMode::Always).is_none()
    );
}
