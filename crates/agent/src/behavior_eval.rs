//! Deterministic semantic-policy regression corpus.
//!
//! This is deliberately not a live-model benchmark: each scenario checks an
//! objective intent and authorization contract without network or prose
//! snapshots. Live providers can reuse the same scenario metadata later.

use crate::intent::{
    IntentGuidanceMode, ToolAuthDecision, UserIntent, authorize_tool, classify_user_intent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedToolDecision {
    Allow,
    Confirm,
    Refuse,
}

#[derive(Debug, Clone, Copy)]
struct BehaviorScenario {
    name: &'static str,
    group: &'static str,
    user_message: &'static str,
    expected_intent: UserIntent,
    agent_name: &'static str,
    tool_name: &'static str,
    command: Option<&'static str>,
    guidance: IntentGuidanceMode,
    expected_tool_decision: ExpectedToolDecision,
}

fn evaluate_scenario(scenario: &BehaviorScenario) -> (UserIntent, ExpectedToolDecision) {
    let assessment = classify_user_intent(scenario.user_message);
    let decision = authorize_tool(
        &assessment,
        scenario.agent_name,
        scenario.tool_name,
        scenario.command,
        scenario.guidance,
    );
    let decision = match decision {
        ToolAuthDecision::Allow => ExpectedToolDecision::Allow,
        ToolAuthDecision::Confirm { .. } => ExpectedToolDecision::Confirm,
        ToolAuthDecision::Refuse { .. } => ExpectedToolDecision::Refuse,
    };
    (assessment.intent, decision)
}

macro_rules! case {
    ($name:literal, $group:literal, $message:literal, $intent:ident, $agent:literal, $tool:literal, $command:expr, $decision:ident) => {
        BehaviorScenario {
            name: $name,
            group: $group,
            user_message: $message,
            expected_intent: UserIntent::$intent,
            agent_name: $agent,
            tool_name: $tool,
            command: $command,
            guidance: IntentGuidanceMode::Auto,
            expected_tool_decision: ExpectedToolDecision::$decision,
        }
    };
    ($name:literal, $group:literal, $message:literal, $intent:ident, $agent:literal, $tool:literal, $command:expr, $guidance:ident, $decision:ident) => {
        BehaviorScenario {
            name: $name,
            group: $group,
            user_message: $message,
            expected_intent: UserIntent::$intent,
            agent_name: $agent,
            tool_name: $tool,
            command: $command,
            guidance: IntentGuidanceMode::$guidance,
            expected_tool_decision: ExpectedToolDecision::$decision,
        }
    };
}

const SCENARIOS: &[BehaviorScenario] = &[
    // Questions: explain without silently mutating.
    case!(
        "question_en_how",
        "intent_question",
        "How does session compaction work?",
        Question,
        "build",
        "read",
        None,
        Allow
    ),
    case!(
        "question_en_why",
        "intent_question",
        "Why does the auth refresh fail?",
        Question,
        "build",
        "edit",
        None,
        Confirm
    ),
    case!(
        "question_en_explain",
        "intent_question",
        "Explain the provider fallback chain",
        Question,
        "build",
        "grep",
        None,
        Allow
    ),
    case!(
        "question_en_where",
        "intent_question",
        "Where is prompt caching configured?",
        Question,
        "build",
        "write",
        None,
        Confirm
    ),
    case!(
        "question_tr_how",
        "intent_question",
        "Compaction nasıl çalışıyor?",
        Question,
        "build",
        "read",
        None,
        Allow
    ),
    case!(
        "question_tr_explain",
        "intent_question",
        "Kimlik doğrulamayı açıkla",
        Question,
        "build",
        "bash",
        Some("git status"),
        Allow
    ),
    case!(
        "question_change_shape",
        "intent_question",
        "Can we fix the flaky test?",
        Question,
        "build",
        "edit",
        None,
        Confirm
    ),
    case!(
        "question_shell_mutation",
        "intent_question",
        "How does cleanup work?",
        Question,
        "build",
        "bash",
        Some("rm -rf target/tmp"),
        Confirm
    ),
    // Changes: explicit implementation requests authorize normal mutations.
    case!(
        "change_en_fix",
        "intent_change",
        "Fix the auth bug in session.rs",
        Change,
        "build",
        "edit",
        None,
        Allow
    ),
    case!(
        "change_en_add",
        "intent_change",
        "Add a timeout to the provider request",
        Change,
        "build",
        "write",
        None,
        Allow
    ),
    case!(
        "change_en_refactor",
        "intent_change",
        "Refactor the retry loop",
        Change,
        "build",
        "bash",
        Some("cargo test -p whycodes-agent"),
        Allow
    ),
    case!(
        "change_en_update",
        "intent_change",
        "Update the README example",
        Change,
        "build",
        "edit",
        None,
        Allow
    ),
    case!(
        "change_tr_fix",
        "intent_change",
        "Auth bug'ını düzelt",
        Change,
        "build",
        "edit",
        None,
        Allow
    ),
    case!(
        "change_tr_add",
        "intent_change",
        "Provider testini ekle",
        Change,
        "build",
        "write",
        None,
        Allow
    ),
    case!(
        "change_tr_update",
        "intent_change",
        "Bağımlılıkları güncelle",
        Change,
        "build",
        "bash",
        Some("cargo metadata --no-deps"),
        Allow
    ),
    case!(
        "change_read",
        "intent_change",
        "Implement request tracing",
        Change,
        "build",
        "read",
        None,
        Allow
    ),
    // Plans: research is allowed; implementation needs confirmation in build.
    case!(
        "plan_en_architecture",
        "intent_plan",
        "Design the architecture and write a plan",
        Plan,
        "build",
        "read",
        None,
        Allow
    ),
    case!(
        "plan_en_roadmap",
        "intent_plan",
        "Create a roadmap for provider parity",
        Plan,
        "build",
        "edit",
        None,
        Confirm
    ),
    case!(
        "plan_en_approach",
        "intent_plan",
        "What's the best approach for session resume?",
        Plan,
        "build",
        "grep",
        None,
        Allow
    ),
    case!(
        "plan_en_before",
        "intent_plan",
        "Before we implement, propose an approach",
        Plan,
        "build",
        "write",
        None,
        Confirm
    ),
    case!(
        "plan_tr",
        "intent_plan",
        "Mimariyi planla ve bir yol haritası çıkar",
        Plan,
        "build",
        "edit",
        None,
        Confirm
    ),
    case!(
        "plan_tr_strategy",
        "intent_plan",
        "Provider geçişi için strateji planla",
        Plan,
        "build",
        "read",
        None,
        Allow
    ),
    case!(
        "plan_readonly_agent",
        "intent_plan",
        "Write a migration plan",
        Plan,
        "plan",
        "write",
        None,
        Refuse
    ),
    case!(
        "plan_shell_read",
        "intent_plan",
        "Plan the dependency cleanup",
        Plan,
        "build",
        "bash",
        Some("git diff --stat"),
        Allow
    ),
    // Mode and guidance contracts.
    case!(
        "ask_refuses_change",
        "authorization",
        "Fix the login bug",
        Change,
        "ask",
        "edit",
        None,
        Refuse
    ),
    case!(
        "explore_refuses_change",
        "authorization",
        "Add request logging",
        Change,
        "explore",
        "write",
        None,
        Refuse
    ),
    case!(
        "plan_refuses_change",
        "authorization",
        "Implement token refresh",
        Change,
        "plan",
        "bash",
        Some("cargo test"),
        Refuse
    ),
    case!(
        "readonly_tool_any_mode",
        "authorization",
        "Fix the parser",
        Change,
        "ask",
        "read",
        None,
        Allow
    ),
    case!(
        "guidance_off_question",
        "authorization",
        "How does auth work?",
        Question,
        "build",
        "edit",
        None,
        Off,
        Allow
    ),
    case!(
        "guidance_always_ambiguous",
        "authorization",
        "auth maybe",
        Ambiguous,
        "build",
        "write",
        None,
        Always,
        Confirm
    ),
    case!(
        "ambiguous_read",
        "authorization",
        "auth maybe",
        Ambiguous,
        "build",
        "read",
        None,
        Allow
    ),
    case!(
        "trivial_read",
        "authorization",
        "selam",
        Trivial,
        "build",
        "read",
        None,
        Allow
    ),
];

#[derive(Debug, Default, PartialEq, Eq)]
struct EvalMetrics {
    total: usize,
    intent_matches: usize,
    authorization_matches: usize,
    passed: usize,
}

struct EvalReport {
    metrics: EvalMetrics,
    failures: Vec<String>,
    groups: std::collections::BTreeSet<&'static str>,
}

fn evaluate_corpus(scenarios: &[BehaviorScenario]) -> EvalReport {
    let mut metrics = EvalMetrics::default();
    let mut failures = Vec::new();
    let mut groups = std::collections::BTreeSet::new();

    for scenario in scenarios {
        metrics.total += 1;
        groups.insert(scenario.group);
        let (actual_intent, actual_decision) = evaluate_scenario(scenario);
        let intent_ok = actual_intent == scenario.expected_intent;
        let authorization_ok = actual_decision == scenario.expected_tool_decision;
        metrics.intent_matches += usize::from(intent_ok);
        metrics.authorization_matches += usize::from(authorization_ok);
        metrics.passed += usize::from(intent_ok && authorization_ok);

        if !intent_ok || !authorization_ok {
            failures.push(format!(
                "{} [{}]: intent={actual_intent:?} expected={:?}; authorization={actual_decision:?} expected={:?}",
                scenario.name,
                scenario.group,
                scenario.expected_intent,
                scenario.expected_tool_decision,
            ));
        }
    }

    EvalReport {
        metrics,
        failures,
        groups,
    }
}

#[test]
fn semantic_policy_corpus_is_valid() {
    assert!(SCENARIOS.len() >= 30, "semantic corpus unexpectedly shrank");

    let mut names = std::collections::BTreeSet::new();
    for scenario in SCENARIOS {
        assert!(
            names.insert(scenario.name),
            "duplicate scenario name: {}",
            scenario.name
        );
    }

    let report = evaluate_corpus(SCENARIOS);
    assert!(
        report.groups.len() >= 4,
        "semantic corpus needs multiple independent policy groups"
    );
}

#[test]
fn semantic_policy_baseline_is_explicit() {
    let report = evaluate_corpus(SCENARIOS);

    // Clarification policy: plan/question turns do not silently mutate, and
    // plan-shaped asks (roadmap, best approach, write a plan) classify as Plan.
    assert_eq!(
        report.metrics,
        EvalMetrics {
            total: 32,
            intent_matches: 32,
            authorization_matches: 32,
            passed: 32,
        },
        "semantic baseline changed; review every changed scenario:\n{}",
        report.failures.join("\n")
    );
    assert!(report.failures.is_empty(), "{}", report.failures.join("\n"));
}

#[test]
fn semantic_policy_corpus_meets_desired_contract() {
    let report = evaluate_corpus(SCENARIOS);
    assert_eq!(
        report.metrics.passed,
        report.metrics.total,
        "scenario pass rate {}/{}; failures:\n{}",
        report.metrics.passed,
        report.metrics.total,
        report.failures.join("\n")
    );
}
