//! User-intent signals for coding agents (question / change / plan).
//!
//! ## Design (industry pattern)
//!
//! Competitors (Cursor Ask/Plan/Agent, OpenCode Build/Plan, Claude Code plan mode)
//! primarily rely on **explicit modes** (hard tool gates) + **system-prompt
//! rules**. They do **not** ship a silent ML router that rewrites every turn.
//!
//! Whycode mirrors that, then adds a **zero-cost heuristic** layer:
//!
//! 1. **Hard modes** — `build` / `plan` / `ask` primary agents (tool denylists).
//! 2. **Static prompt protocol** — when to answer, edit, plan, or `question`.
//! 3. **Ephemeral posture** — high-confidence hints appended to the last user
//!    message **only in the LLM request** (not stored in session history), so
//!    the system prompt stays cache-stable.
//!
//! Authorization-style classifiers (Claude Code auto mode) are a separate
//! concern: "is this tool call authorized?", not "what does the user want?".

use whycode_core::types::{LlmRequest, MessageContent, Role};

/// How to treat heuristic intent for build-mode turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntentGuidanceMode {
    /// Inject posture when confidence is high enough (default).
    #[default]
    Auto,
    /// Never inject.
    Off,
    /// Always inject a short posture line (even for medium confidence).
    Always,
}

impl IntentGuidanceMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "0" => Self::Off,
            "always" | "force" | "on" => Self::Always,
            _ => Self::Auto,
        }
    }
}

/// Coarse user intent for a single message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIntent {
    /// Explain / how / why — prefer read-only tools and prose answers.
    Question,
    /// Fix / add / implement — edits and shell are expected.
    Change,
    /// Design / architecture / multi-file ambiguity — plan before mutating.
    Plan,
    /// Casual / empty / greeting.
    Trivial,
    /// Mixed or weak signals.
    Ambiguous,
}

impl UserIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Change => "change",
            Self::Plan => "plan",
            Self::Trivial => "trivial",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// Result of heuristic classification.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentAssessment {
    pub intent: UserIntent,
    /// 0.0–1.0; high means the posture is safe to assert.
    pub confidence: f32,
    pub reasons: Vec<&'static str>,
}

impl IntentAssessment {
    pub fn is_high(&self) -> bool {
        self.confidence >= 0.72
    }
}

/// Classify a user message without an extra LLM call.
///
/// English + Turkish markers; biased toward **Change** when both imperative
/// and interrogative forms appear (coding agents default to doing work).
pub fn classify_user_intent(text: &str) -> IntentAssessment {
    let t = text.trim();
    if t.is_empty() || crate::title::is_trivial_title_seed(t) {
        return IntentAssessment {
            intent: UserIntent::Trivial,
            confidence: 0.95,
            reasons: vec!["trivial_or_empty"],
        };
    }

    let lower = t.to_ascii_lowercase();
    let mut reasons: Vec<&'static str> = Vec::new();

    let q_score = score_markers(&lower, QUESTION_MARKERS, &mut reasons, "question_marker");
    let c_score = score_markers(&lower, CHANGE_MARKERS, &mut reasons, "change_marker");
    let p_score = score_markers(&lower, PLAN_MARKERS, &mut reasons, "plan_marker");

    let has_qmark = t.contains('?') || t.contains('？');
    if has_qmark {
        reasons.push("question_mark");
    }

    // Soft question shape: starts with WH-word / Turkish equivalents.
    let starts_question = starts_with_any(&lower, QUESTION_STARTERS);
    if starts_question {
        reasons.push("question_starter");
    }

    // Imperative / short directive without `?` → change bias.
    let imperative = !has_qmark
        && t.chars().count() < 200
        && starts_with_any(&lower, CHANGE_STARTERS);
    if imperative {
        reasons.push("change_starter");
    }

    // Multi-file / architecture sprawl → plan.
    let sprawl = count_path_hints(t) >= 3 || p_score >= 2;
    if sprawl && p_score > 0 {
        reasons.push("scope_sprawl");
    }

    let mut q = q_score as f32 + if has_qmark { 1.2 } else { 0.0 } + if starts_question { 1.0 } else { 0.0 };
    let c = c_score as f32 + if imperative { 1.4 } else { 0.0 };
    let p = p_score as f32 + if sprawl && p_score > 0 { 0.8 } else { 0.0 };

    // "can we fix" / "shall we" — question form about a change: treat as
    // clarification-first (Claude auto-mode: not a hard directive).
    if has_qmark && c_score > 0 && q_score == 0 {
        q += 0.8;
        reasons.push("question_about_change");
    }

    // Pure explain requests without fix verbs.
    if q > c && q > p && c_score == 0 {
        q += 0.3;
    }

    let (intent, raw) = if p >= q && p >= c && p >= 1.5 {
        (UserIntent::Plan, p)
    } else if q >= c && q >= p && q >= 1.2 {
        (UserIntent::Question, q)
    } else if c >= q && c >= p && c >= 1.0 {
        (UserIntent::Change, c)
    } else if has_qmark && c < 1.0 {
        (UserIntent::Question, 1.0 + if starts_question { 0.5 } else { 0.0 })
    } else if imperative {
        (UserIntent::Change, 1.2)
    } else {
        (UserIntent::Ambiguous, 0.4)
    };

    let confidence = match intent {
        UserIntent::Trivial => 0.95,
        UserIntent::Ambiguous => 0.35,
        _ => (raw / 4.0).clamp(0.4, 0.95),
    };

    IntentAssessment {
        intent,
        confidence,
        reasons,
    }
}

/// Build the ephemeral posture suffix for the model (not persisted).
pub fn posture_suffix(assessment: &IntentAssessment, agent_name: &str) -> Option<String> {
    // Modes that already hard-gate tools do not need soft posture.
    if matches!(agent_name, "plan" | "ask" | "explore" | "scout") {
        return None;
    }

    let label = assessment.intent.as_str();
    let body = match assessment.intent {
        UserIntent::Question => {
            "This turn looks like a **question / explanation** request. \
             Prefer a clear answer; use read-only tools (`read`, `grep`, `glob`, `list`, web) if needed. \
             Do **not** edit files, run mutating shell, or open PRs unless the user clearly asked for a change. \
             A question is not a directive to implement."
        }
        UserIntent::Plan => {
            "This turn looks like a **planning / design** request. \
             Research with read-only tools, outline a structured plan (files, steps, risks), \
             and use `question` if requirements are ambiguous. \
             Do **not** start large edits until the approach is clear or the user asks to implement."
        }
        UserIntent::Change => {
            "This turn looks like an **implementation** request. \
             Make the change carefully: read before edit, keep diffs focused, run relevant checks. \
             If the request is vague or high-risk, ask with `question` first rather than guessing."
        }
        UserIntent::Ambiguous => {
            "Intent is **ambiguous**. If the next step would touch many files or is irreversible, \
             ask a short clarifying `question` or propose a brief plan before editing. \
             Small, obvious fixes may proceed."
        }
        UserIntent::Trivial => return None,
    };

    Some(format!(
        "\n\n<whycode_intent confidence=\"{:.2}\" kind=\"{label}\">\n{body}\n</whycode_intent>",
        assessment.confidence
    ))
}

/// Whether guidance should be injected for this assessment and mode.
pub fn should_inject(mode: IntentGuidanceMode, assessment: &IntentAssessment) -> bool {
    match mode {
        IntentGuidanceMode::Off => false,
        IntentGuidanceMode::Always => {
            !matches!(assessment.intent, UserIntent::Trivial)
        }
        IntentGuidanceMode::Auto => {
            match assessment.intent {
                UserIntent::Trivial => false,
                UserIntent::Ambiguous => assessment.confidence >= 0.5,
                // Soft posture for non-change intents is highest value in build.
                UserIntent::Question | UserIntent::Plan => assessment.is_high(),
                // Change is the build default; only inject when very clear (authorization tone).
                UserIntent::Change => assessment.confidence >= 0.85,
            }
        }
    }
}

/// Append posture to the **last user message in the request only**.
///
/// Returns the assessment when a suffix was applied.
pub fn apply_intent_to_request(
    request: &mut LlmRequest,
    user_text: &str,
    agent_name: &str,
    mode: IntentGuidanceMode,
) -> Option<IntentAssessment> {
    if matches!(mode, IntentGuidanceMode::Off) {
        return None;
    }
    let assessment = classify_user_intent(user_text);
    if !should_inject(mode, &assessment) {
        return None;
    }
    let suffix = posture_suffix(&assessment, agent_name)?;

    // Find last user message and append (clone-safe on owned request).
    for msg in request.messages.iter_mut().rev() {
        if msg.role != Role::User {
            continue;
        }
        match &mut msg.content {
            MessageContent::Text(t) => {
                t.push_str(&suffix);
                return Some(assessment);
            }
            MessageContent::Blocks(blocks) => {
                // Append a trailing text block so multimodal turns keep images.
                blocks.push(whycode_core::types::ContentBlock::Text { text: suffix });
                return Some(assessment);
            }
        }
    }
    None
}

/// Short UI/status line (TUI toast / status bar).
pub fn status_hint(assessment: &IntentAssessment, agent_name: &str) -> Option<String> {
    if !assessment.is_high() {
        return None;
    }
    match (assessment.intent, agent_name) {
        (UserIntent::Question, "build") => Some(
            "Intent: question — answering without edits (Ctrl+T → ask for read-only mode)".into(),
        ),
        (UserIntent::Plan, "build") => {
            Some("Intent: plan — outlining before edits (Ctrl+T → plan mode)".into())
        }
        (UserIntent::Change, "ask") | (UserIntent::Change, "plan") => Some(format!(
            "Intent: change — switch to build (Ctrl+T) to apply edits (current: {agent_name})"
        )),
        _ => None,
    }
}

// ── markers ──────────────────────────────────────────────────────────────

const QUESTION_MARKERS: &[&str] = &[
    "how does",
    "how do ",
    "how is ",
    "what is ",
    "what are ",
    "what does",
    "why does",
    "why is ",
    "why do ",
    "where is ",
    "where are ",
    "when does",
    "explain",
    "describe",
    "walk me through",
    "help me understand",
    "can you explain",
    "could you explain",
    "nasıl çalış",
    "nedir",
    "ne işe yarar",
    "neden ",
    "niçin",
    "anlat",
    "açıkla",
    "acikla",
    "ne fark",
];

const QUESTION_STARTERS: &[&str] = &[
    "how ",
    "what ",
    "why ",
    "where ",
    "when ",
    "which ",
    "who ",
    "is ",
    "are ",
    "does ",
    "do ",
    "can ",
    "could ",
    "would ",
    "should ",
    "nasıl",
    "nedir",
    "neden",
    "niçin",
    "hangi",
    "nerede",
    "ne ",
    "mi ",
    "mı ",
    "mu ",
    "mü ",
];

const CHANGE_MARKERS: &[&str] = &[
    "fix ",
    "fix the",
    "add ",
    "implement",
    "refactor",
    "rename ",
    "delete ",
    "remove ",
    "update ",
    "change ",
    "create ",
    "write ",
    "edit ",
    "patch ",
    "migrate",
    "install ",
    "upgrade ",
    "wire ",
    "hook up",
    "make it",
    "make sure",
    "please fix",
    "please add",
    "please implement",
    "düzelt",
    "duzelt",
    "ekle",
    "uygula",
    "yaz ",
    "sil ",
    "kaldır",
    "kaldir",
    "değiştir",
    "degistir",
    "güncelle",
    "guncelle",
    "oluştur",
    "olustur",
    "refactor et",
    "bug fix",
];

const CHANGE_STARTERS: &[&str] = &[
    "fix ",
    "add ",
    "implement ",
    "refactor ",
    "rename ",
    "delete ",
    "remove ",
    "update ",
    "change ",
    "create ",
    "write ",
    "edit ",
    "patch ",
    "make ",
    "set ",
    "enable ",
    "disable ",
    "düzelt",
    "duzelt",
    "ekle ",
    "uygula ",
    "yaz ",
    "sil ",
    "kaldır",
    "kaldir",
    "değiştir",
    "degistir",
    "güncelle",
    "guncelle",
    "oluştur",
    "olustur",
];

const PLAN_MARKERS: &[&str] = &[
    "plan ",
    "plan the",
    "make a plan",
    "write a plan",
    "design ",
    "architecture",
    "how should we",
    "how would we",
    "what's the best approach",
    "what is the best approach",
    "trade-off",
    "tradeoff",
    "roadmap",
    "break down",
    "step by step plan",
    "before we implement",
    "propose an approach",
    "migration plan",
    "planla",
    "plan çıkar",
    "plan cikar",
    "mimari",
    "nasıl ilerleyelim",
    "nasil ilerleyelim",
    "yaklaşım",
    "yaklasim",
    "strateji",
    "adım adım",
    "adim adim",
];

fn score_markers(
    lower: &str,
    markers: &[&str],
    reasons: &mut Vec<&'static str>,
    reason: &'static str,
) -> u32 {
    let mut n = 0u32;
    for m in markers {
        if lower.contains(m) {
            n += 1;
            if n == 1 {
                reasons.push(reason);
            }
        }
    }
    n
}

fn starts_with_any(lower: &str, heads: &[&str]) -> bool {
    let t = lower.trim_start();
    heads.iter().any(|h| t.starts_with(h))
}

fn count_path_hints(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| {
            w.contains('/')
                || w.ends_with(".rs")
                || w.ends_with(".ts")
                || w.ends_with(".tsx")
                || w.ends_with(".py")
                || w.ends_with(".go")
                || w.ends_with(".js")
        })
        .count()
}

#[cfg(test)]
mod tests {
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
        let a = classify_user_intent(
            "Design the architecture for multi-tenant billing and write a plan",
        );
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
            messages: vec![whycode_core::types::Message {
                role: Role::User,
                content: MessageContent::Text("How does auth work?".into()),
                tool_call_id: None,
                name: None,
            }],
            tools: vec![],
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
        assert!(text.contains("whycode_intent"));
        assert!(text.contains("How does auth work?"));
    }

    #[test]
    fn off_mode_skips() {
        let mut req = LlmRequest {
            system: "sys".into(),
            messages: vec![whycode_core::types::Message {
                role: Role::User,
                content: MessageContent::Text("How does auth work?".into()),
                tool_call_id: None,
                name: None,
            }],
            tools: vec![],
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
}
