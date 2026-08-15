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
    let imperative =
        !has_qmark && t.chars().count() < 200 && starts_with_any(&lower, CHANGE_STARTERS);
    if imperative {
        reasons.push("change_starter");
    }

    // Multi-file / architecture sprawl → plan.
    let sprawl = count_path_hints(t) >= 3 || p_score >= 2;
    if sprawl && p_score > 0 {
        reasons.push("scope_sprawl");
    }

    let mut q = q_score as f32
        + if has_qmark { 1.2 } else { 0.0 }
        + if starts_question { 1.0 } else { 0.0 };
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
        (
            UserIntent::Question,
            1.0 + if starts_question { 0.5 } else { 0.0 },
        )
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
        IntentGuidanceMode::Always => !matches!(assessment.intent, UserIntent::Trivial),
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
    for msg in request.messages_mut().iter_mut().rev() {
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

/// Short chrome badge when confidence is high enough to show.
pub fn badge_label(assessment: &IntentAssessment) -> Option<&'static str> {
    if matches!(
        assessment.intent,
        UserIntent::Trivial | UserIntent::Ambiguous
    ) {
        return None;
    }
    if !assessment.is_high() {
        return None;
    }
    Some(match assessment.intent {
        UserIntent::Question => "Q",
        UserIntent::Change => "chg",
        UserIntent::Plan => "plan",
        UserIntent::Trivial | UserIntent::Ambiguous => unreachable!(),
    })
}

/// Toast severity for TUI (`info` / `warning`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentNoticeKind {
    Info,
    Warning,
}

/// User-visible notice for mode / intent mismatch or posture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentNotice {
    pub kind: IntentNoticeKind,
    pub message: String,
}

/// Build a toast-worthy notice (prefer Warning when mode and intent disagree).
pub fn intent_notice(assessment: &IntentAssessment, agent_name: &str) -> Option<IntentNotice> {
    if !assessment.is_high() {
        return None;
    }
    match (assessment.intent, agent_name) {
        (UserIntent::Change, "ask") | (UserIntent::Change, "plan") => Some(IntentNotice {
            kind: IntentNoticeKind::Warning,
            message: format!(
                "Implementation request while in {agent_name} (read-only). \
                 Ctrl+T → build to apply edits."
            ),
        }),
        (UserIntent::Question, "build") => Some(IntentNotice {
            kind: IntentNoticeKind::Info,
            message: "Intent: question — no edits (Ctrl+T → ask for hard read-only)".into(),
        }),
        (UserIntent::Plan, "build") => Some(IntentNotice {
            kind: IntentNoticeKind::Info,
            message: "Intent: plan — outline first (Ctrl+T → plan mode)".into(),
        }),
        (UserIntent::Plan, "ask") => Some(IntentNotice {
            kind: IntentNoticeKind::Info,
            message: "Design-shaped ask — Ctrl+T → plan for a full plan".into(),
        }),
        _ => None,
    }
}

/// Short UI/status line (legacy string form).
pub fn status_hint(assessment: &IntentAssessment, agent_name: &str) -> Option<String> {
    intent_notice(assessment, agent_name).map(|n| n.message)
}

// ── Tool authorization (Claude-style: user message vs agent action) ─────

/// Decision for a tool call given turn intent (not shell blast-radius risk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolAuthDecision {
    Allow,
    /// Ask the user; show `reason` in the permission dialog.
    Confirm {
        reason: String,
    },
    /// Hard block; model must try another path.
    Refuse {
        reason: String,
    },
}

/// Tools that mutate the workspace or shared state.
const MUTATING_TOOLS: &[&str] = &[
    "write",
    "edit",
    "apply_patch",
    "git_commit",
    "bash",
    "shell",
    "code_mode",
    "external_directory",
];

/// Whether this tool can change durable state (files, git, shell side effects).
pub fn is_mutating_tool(name: &str) -> bool {
    MUTATING_TOOLS.contains(&name)
}

/// Classify shell as observational (ls/rg/git status/…) vs side-effecting.
///
/// Conservative: any pipe that clearly mutates, redirect, or unknown head → not read-only.
pub fn is_read_only_shell(command: &str) -> bool {
    let cmd = command.trim();
    if cmd.is_empty() {
        return true;
    }
    // Redirection / background / destructive chain markers.
    if cmd.contains('>')
        || cmd.contains(">>")
        || cmd.contains("<<")
        || cmd.contains('|') && (cmd.contains("xargs") || cmd.contains("tee "))
        || cmd.contains("sudo ")
        || cmd.contains("rm ")
        || cmd.contains("mv ")
        || cmd.contains("cp ")
        || cmd.contains("chmod ")
        || cmd.contains("chown ")
        || cmd.contains("kill ")
        || cmd.contains("dd ")
        || cmd.contains("mkfs")
        || cmd.contains("git push")
        || cmd.contains("git commit")
        || cmd.contains("git reset")
        || cmd.contains("git checkout")
        || cmd.contains("git clean")
        || cmd.contains("npm install")
        || cmd.contains("cargo install")
        || cmd.contains("pip install")
    {
        return false;
    }

    // Split on shell list operators; every segment must look observational.
    for segment in cmd.split(&['&', ';', '\n'][..]) {
        let seg = segment
            .trim()
            .trim_start_matches("&&")
            .trim_start_matches("||")
            .trim();
        if seg.is_empty() {
            continue;
        }
        if !segment_looks_read_only(seg) {
            return false;
        }
    }
    true
}

fn segment_looks_read_only(seg: &str) -> bool {
    let lower = seg.to_ascii_lowercase();
    // Strip env assignments: `FOO=1 ls`
    let token = lower
        .split_whitespace()
        .find(|t| !t.contains('=') || t.starts_with('-'))
        .unwrap_or("");
    let head = token.rsplit('/').next().unwrap_or(token);
    READ_ONLY_SHELL_HEADS.contains(&head)
}

const READ_ONLY_SHELL_HEADS: &[&str] = &[
    "ls",
    "ll",
    "dir",
    "pwd",
    "echo",
    "printf",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "file",
    "stat",
    "which",
    "type",
    "command",
    "true",
    "false",
    "test",
    "[",
    "rg",
    "grep",
    "fgrep",
    "egrep",
    "find",
    "fd",
    "locate",
    "tree",
    "git", // further filtered above for push/commit/reset
    "diff",
    "cmp",
    "md5sum",
    "sha256sum",
    "cargo", // cargo check/test/build are builds — not pure read; see below
    "rustc",
    "python",
    "python3",
    "node",
    "ruby",
    "perl",
    "jq",
    "yq",
    "awk",
    "sed", // sed without -i is usually filter; allow if no -i
    "env",
    "printenv",
    "uname",
    "whoami",
    "id",
    "date",
    "cal",
    "df",
    "du",
    "free",
    "ps",
    "top",
    "htop",
    "curl",
    "wget",
    "http", // network fetch; still non-mutating for local FS
];

/// Shell heads that are usually read-only only with specific subcommands.
fn shell_head_readonly_ok(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    if lower.split_whitespace().any(|t| t == "sed") && lower.contains("-i") {
        return false;
    }
    // cargo build/run/install mutate target/; allow check/test/clippy/doc as "dev" —
    // still side effects in target/. Treat cargo as mutating unless metadata-ish.
    if let Some(rest) = lower.strip_prefix("cargo ") {
        let sub = rest.split_whitespace().next().unwrap_or("");
        return matches!(
            sub,
            "metadata" | "tree" | "search" | "info" | "version" | "--version" | "-V" | "help"
        );
    }
    if lower.starts_with("git ") {
        let mut parts = lower.split_whitespace();
        let _ = parts.next();
        let sub = parts.next().unwrap_or("");
        return match sub {
            "status" | "log" | "diff" | "show" | "branch" | "tag" | "remote" | "rev-parse"
            | "describe" | "ls-files" | "blame" => true,
            "stash" => {
                let third = parts.next().unwrap_or("list");
                matches!(third, "list" | "show")
            }
            "config" => lower.contains("--get") || lower.contains("-l") || lower.contains("--list"),
            _ => false,
        };
    }
    true
}

/// Authorize a tool against the turn's user intent (Claude auto-mode spirit).
///
/// - **ask/plan agents**: mutating tools should already be denied by permission;
///   this is a second line of defence.
/// - **build + Question/Plan (high)**: escalate mutators to Confirm so the model
///   cannot silently implement a question.
/// - **Change / Ambiguous / Off mode**: allow (shell risk gate still applies).
pub fn authorize_tool(
    assessment: &IntentAssessment,
    agent_name: &str,
    tool_name: &str,
    command: Option<&str>,
    guidance: IntentGuidanceMode,
) -> ToolAuthDecision {
    if matches!(guidance, IntentGuidanceMode::Off) {
        return ToolAuthDecision::Allow;
    }
    if !is_mutating_tool(tool_name) {
        return ToolAuthDecision::Allow;
    }

    // Read-only shell never needs intent confirm.
    if matches!(tool_name, "bash" | "shell")
        && let Some(cmd) = command
        && is_read_only_shell(cmd)
        && shell_head_readonly_ok(cmd)
    {
        return ToolAuthDecision::Allow;
    }

    let restricted_agent = matches!(agent_name, "ask" | "plan" | "explore" | "scout");
    if restricted_agent {
        return ToolAuthDecision::Refuse {
            reason: format!(
                "Agent `{agent_name}` is read-only; tool `{tool_name}` is not allowed. \
                 Switch to build (Ctrl+T) to mutate the workspace."
            ),
        };
    }

    // Only escalate when we are confident the user did not authorize mutation.
    let escalate = match assessment.intent {
        UserIntent::Question
            if assessment.is_high() || matches!(guidance, IntentGuidanceMode::Always) =>
        {
            Some("question")
        }
        UserIntent::Plan
            if assessment.is_high() || matches!(guidance, IntentGuidanceMode::Always) =>
        {
            Some("plan")
        }
        UserIntent::Ambiguous
            if matches!(guidance, IntentGuidanceMode::Always) && assessment.confidence >= 0.4 =>
        {
            Some("ambiguous")
        }
        _ => None,
    };

    let Some(kind) = escalate else {
        return ToolAuthDecision::Allow;
    };

    let reason = match kind {
        "question" => format!(
            "User message looks like a **question**, not an edit request \
             (intent={kind}, confidence={:.0}%).\n\
             Tool `{tool_name}` would change the workspace.\n\
             Approve only if you want this mutation now.",
            assessment.confidence * 100.0
        ),
        "plan" => format!(
            "User message looks like a **planning** request \
             (intent={kind}, confidence={:.0}%).\n\
             Tool `{tool_name}` would start implementation.\n\
             Approve only if you want to implement now without a separate build step.",
            assessment.confidence * 100.0
        ),
        _ => format!(
            "User intent is unclear; tool `{tool_name}` would mutate the workspace. Confirm?"
        ),
    };

    ToolAuthDecision::Confirm { reason }
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
    "how ", "what ", "why ", "where ", "when ", "which ", "who ", "is ", "are ", "does ", "do ",
    "can ", "could ", "would ", "should ", "nasıl", "nedir", "neden", "niçin", "hangi", "nerede",
    "ne ", "mi ", "mı ", "mu ", "mü ",
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
            messages: std::sync::Arc::from(vec![whycode_core::types::Message {
                role: Role::User,
                content: MessageContent::Text("How does auth work?".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
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
            messages: std::sync::Arc::from(vec![whycode_core::types::Message {
                role: Role::User,
                content: MessageContent::Text("How does auth work?".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
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
}
