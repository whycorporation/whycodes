//! `/goal <condition>`: keep taking turns until a fast evaluator says the
//! condition holds.
//!
//! After each finished turn a no-tools call to `session.model_fast` reads the
//! condition plus the recent transcript and answers met / not met /
//! impossible. Not met queues another turn; met or impossible clears the goal.
//! The evaluator cannot run commands, so it only sees what the agent surfaced.
//! Several turns in a row without a tool call pause the goal (kept, not
//! cleared) until the user prompts again. Pure logic lives here; the run loop
//! owns the spawn, the channel, and persistence.

use std::time::{Duration, Instant};

use whycodes_core::types::{ContentBlock, LlmRequest, Message, MessageContent, Role};

/// Longest condition `/goal` accepts (characters).
pub const MAX_CONDITION_CHARS: usize = 4_000;

/// Consecutive turns without a tool call that pause the goal.
pub const IDLE_TURN_LIMIT: u32 = 3;

/// Transcript characters the evaluator sees, newest first.
const TRANSCRIPT_CHARS: usize = 20_000;

/// Earlier checks' findings the evaluator is shown (newest kept).
const MAX_FINDINGS: usize = 6;

/// `state` table key holding a session's active goal condition.
pub fn state_key(session_id: &str) -> String {
    format!("goal:{session_id}")
}

/// `state` table key holding how many turns the goal has taken, so a
/// turn cap in the condition or `session.goal_max_turns` survives a resume.
pub fn turns_key(session_id: &str) -> String {
    format!("goal_turns:{session_id}")
}

/// Input plus output tokens the session has spent (goal spend baseline).
pub fn session_tokens(session: &whycodes_session::session::Session) -> u64 {
    session.usage.input_tokens + session.usage.output_tokens
}

/// Store (or, with `None`, forget) the session's goal so `--resume` restores
/// it. Best effort: a goal that cannot be saved still runs this session.
pub fn save(db: Option<&whycodes_storage::db::Database>, session_id: &str, goal: Option<&Goal>) {
    let Some(db) = db else {
        return;
    };
    let (key, turns) = (state_key(session_id), turns_key(session_id));
    let result = match goal {
        Some(goal) => db
            .set_state(&key, &goal.condition)
            .and_then(|()| db.set_state(&turns, &goal.turns.to_string())),
        None => db.delete_state(&key).and_then(|()| db.delete_state(&turns)),
    };
    if let Err(e) = result {
        tracing::debug!(error = %e, "goal state not saved");
    }
}

/// The session's saved goal, restored active with its turn count; time and
/// spend start fresh.
pub fn restore(
    db: Option<&whycodes_storage::db::Database>,
    session: &whycodes_session::session::Session,
) -> Option<Goal> {
    let db = db?;
    let condition = db.get_state(&state_key(&session.id)).ok()??;
    let mut goal = Goal::new(condition, session_tokens(session));
    if let Ok(Some(turns)) = db.get_state(&turns_key(&session.id)) {
        goal.turns = turns.parse().unwrap_or(0);
    }
    Some(goal)
}

/// What `/goal …` asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalSlash {
    Show,
    Clear,
    Set(String),
    TooLong(usize),
}

pub fn parse_goal_slash(rest: &str) -> GoalSlash {
    let rest = rest.trim();
    if rest.is_empty() {
        return GoalSlash::Show;
    }
    let lower = rest.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "clear" | "stop" | "off" | "reset" | "none" | "cancel"
    ) {
        return GoalSlash::Clear;
    }
    let chars = rest.chars().count();
    if chars > MAX_CONDITION_CHARS {
        return GoalSlash::TooLong(chars);
    }
    GoalSlash::Set(rest.to_string())
}

/// What the run loop does after a goal turn finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStep {
    /// Ask the evaluator.
    Evaluate,
    /// Too many idle turns; the goal is now paused.
    Pause,
}

/// An active goal for one session.
#[derive(Debug, Clone)]
pub struct Goal {
    pub condition: String,
    pub started: Instant,
    /// Turns evaluated since the goal was set (kept across a resume).
    pub turns: u32,
    /// Turns since the goal was set, restored, or resumed from a pause:
    /// what `session.goal_max_turns` caps.
    pub turns_this_run: u32,
    /// Consecutive finished turns with no tool call.
    pub idle_turns: u32,
    /// Session output tokens when the goal was set, for spend.
    pub tokens_at_start: u64,
    /// The evaluator's latest reason.
    pub last_reason: Option<String>,
    /// Earlier checks' reasons, oldest first, so evidence that scrolled out
    /// of the transcript window (or was compacted away) still counts.
    pub findings: Vec<String>,
    /// Tokens the checks themselves spent.
    pub check_tokens: u64,
    /// Why the goal is paused, when it is.
    pub paused: Option<String>,
    /// An evaluator call is in flight.
    pub evaluating: bool,
}

impl Goal {
    pub fn new(condition: impl Into<String>, tokens_now: u64) -> Self {
        Self {
            condition: condition.into(),
            started: Instant::now(),
            turns: 0,
            turns_this_run: 0,
            idle_turns: 0,
            tokens_at_start: tokens_now,
            last_reason: None,
            findings: Vec::new(),
            check_tokens: 0,
            paused: None,
            evaluating: false,
        }
    }

    /// Record a finished turn. A paused goal resumes here: while paused no
    /// turn is queued, so any turn that finishes was the user's prompt.
    pub fn record_turn(&mut self, used_tools: bool) -> TurnStep {
        if self.paused.take().is_some() {
            self.turns_this_run = 0;
        }
        self.turns += 1;
        self.turns_this_run += 1;
        self.idle_turns = if used_tools { 0 } else { self.idle_turns + 1 };
        if self.idle_turns >= IDLE_TURN_LIMIT {
            self.pause(format!("no tool use for {IDLE_TURN_LIMIT} turns in a row"));
            return TurnStep::Pause;
        }
        self.evaluating = true;
        TurnStep::Evaluate
    }

    /// Keep a check's reason for the next check to read.
    pub fn record_finding(&mut self, reason: &str) {
        self.last_reason = Some(reason.to_string());
        self.findings
            .push(format!("after turn {}: {reason}", self.turns));
        if self.findings.len() > MAX_FINDINGS {
            self.findings.remove(0);
        }
    }

    pub fn pause(&mut self, why: impl Into<String>) {
        self.paused = Some(why.into());
        self.evaluating = false;
    }

    /// `/goal` with no arguments.
    pub fn status(&self, tokens_now: u64) -> String {
        let state = match &self.paused {
            Some(why) => format!("paused ({why}; send a prompt to resume)"),
            None if self.evaluating => "checking".to_string(),
            None => "active".to_string(),
        };
        let mut out = format!(
            "◎ goal: {}\n{state} · {} · {} turn{} evaluated · {} tokens",
            self.condition,
            elapsed_label(self.started.elapsed()),
            self.turns,
            if self.turns == 1 { "" } else { "s" },
            tokens_now.saturating_sub(self.tokens_at_start),
        );
        if self.check_tokens > 0 {
            out.push_str(&format!(" + {} for checks", self.check_tokens));
        }
        if let Some(reason) = &self.last_reason {
            out.push_str(&format!("\nlast check: {reason}"));
        }
        out
    }
}

fn elapsed_label(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// The evaluator's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Met,
    NotMet,
    Impossible,
}

/// Read `VERDICT: …` / `REASON: …`, tolerating the markdown small models
/// wrap them in (`**VERDICT:** met`, `- verdict = met`). A verdict line with
/// an unclear word is not met. `None` when there is no verdict line at all:
/// the caller asks again instead of guessing.
pub fn parse_verdict(text: &str) -> (Option<Verdict>, String) {
    let mut verdict = None;
    let mut reason = String::new();
    for line in text.lines() {
        let line = plain_line(line);
        if let Some(rest) = field(&line, "verdict") {
            let word: String = rest
                .chars()
                .filter(char::is_ascii_alphabetic)
                .collect::<String>()
                .to_ascii_lowercase();
            verdict = Some(match word.as_str() {
                "met" => Verdict::Met,
                "impossible" => Verdict::Impossible,
                _ => Verdict::NotMet,
            });
        } else if let Some(rest) = field(&line, "reason") {
            reason = rest.to_string();
        }
    }
    if reason.is_empty() {
        reason = text
            .lines()
            .map(plain_line)
            .rfind(|l| !l.is_empty())
            .unwrap_or_default();
    }
    (verdict, reason)
}

/// A line without the list, quote and bold markers around it. Stars inside
/// stay: a reason may say `a * b`.
fn plain_line(line: &str) -> String {
    line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '#' | '>' | '*'))
        .trim_end_matches(|c: char| c.is_whitespace() || c == '*')
        .to_string()
}

/// The value of `name:` (or `name =` / `name -`), case-insensitive, with
/// bold stars around the name or the separator skipped.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let head = line.get(..name.len())?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    let bold = |c: char| c.is_whitespace() || c == '*';
    line[name.len()..]
        .trim_start_matches(bold)
        .strip_prefix([':', '=', '-'])
        .map(|rest| rest.trim_start_matches(bold))
}

/// True when the turn that just finished (after the last user prompt) made a
/// tool call.
pub fn turn_used_tools(messages: &[Message]) -> bool {
    for message in messages.iter().rev() {
        let blocks = match &message.content {
            MessageContent::Blocks(blocks) => blocks.as_slice(),
            MessageContent::Text(_) => &[],
        };
        if message.role == Role::Assistant
            && blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
        {
            return true;
        }
        let is_prompt = message.role == Role::User
            && !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
        if is_prompt {
            return false;
        }
    }
    false
}

/// Recent conversation as plain text, newest kept when over the budget.
pub fn transcript(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut used = 0usize;
    for message in messages.iter().rev() {
        let who = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => continue,
        };
        let text = message_text(&message.content);
        if text.trim().is_empty() {
            continue;
        }
        let entry = format!("[{who}]\n{}", text.trim());
        used += entry.len();
        parts.push(entry);
        if used >= TRANSCRIPT_CHARS {
            break;
        }
    }
    parts.reverse();
    let joined = parts.join("\n\n");
    let skip = joined.len().saturating_sub(TRANSCRIPT_CHARS);
    joined[joined.ceil_char_boundary(skip)..].to_string()
}

fn message_text(content: &MessageContent) -> String {
    let blocks = match content {
        MessageContent::Text(text) => return text.clone(),
        MessageContent::Blocks(blocks) => blocks,
    };
    let mut out = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => out.push(text.clone()),
            ContentBlock::ToolUse { name, input, .. } => {
                out.push(format!("(called {name} {input})"))
            }
            ContentBlock::ToolResult { content, .. } => out.push(content.clone()),
            _ => {}
        }
    }
    out.join("\n")
}

// Live runs on small models: a self-contradictory goal was judged not_met
// (while the reason called it impossible) until the agent faked it, and the
// fake was then judged met. Hence the explicit intent and up-front rules.
// Once goal turns asked agents to stop honestly, a small checker judged an
// honest "this cannot be done" as met; the goal-not-instructions line.
const EVALUATOR_SYSTEM: &str = "You check whether a coding agent has met a goal. You cannot run commands or read files: judge only from the transcript.
met: the transcript shows evidence (command output, test results, the finished change) that the goal holds as the user meant it. A workaround that only satisfies the wording (special-casing a check, faking output, redefining what the words mean) is not met.
impossible: the goal contradicts itself or logic, something it needs is gone for good, or the user ruled it out. Judge this from the goal itself: the agent does not have to try first.
not_met: anything else, including missing evidence.
Judge the goal, not whether the agent followed instructions: an agent that rightly says the goal cannot be met has not met it; that is impossible.
Findings from earlier checks still hold unless the transcript shows they changed.
Answer in plain text without markdown, in exactly two lines:
VERDICT: met | not_met | impossible
REASON: one sentence naming what is done and what is still missing";

/// No-tools request for the evaluator model.
pub fn evaluator_request(
    condition: &str,
    turns: u32,
    findings: &[String],
    messages: &[Message],
) -> LlmRequest {
    // The turn count lets a condition like "or stop after 20 turns" be judged;
    // the transcript alone is too short to count them.
    let mut body = format!("Goal:\n{condition}\n\nTurns taken toward this goal: {turns}\n\n");
    if !findings.is_empty() {
        body.push_str("Earlier checks (oldest first):\n");
        for finding in findings {
            body.push_str(&format!("- {finding}\n"));
        }
        body.push('\n');
    }
    body.push_str(&format!(
        "Transcript (most recent last):\n{}",
        transcript(messages)
    ));
    LlmRequest {
        system: EVALUATOR_SYSTEM.into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text(body),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        // Room for a model that thinks before its two lines.
        max_tokens: Some(1024),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

/// Live runs: given a contradictory goal, agents built a value "equal" to
/// both sides and called it done. Every goal turn asks for the intent and
/// an honest stop instead.
const GOAL_RULES: &str = "Meet it as the user means it. If it cannot be met that way \
(it contradicts itself, or only a workaround would satisfy the wording), say so \
plainly and stop instead of working around it. When it is met, show the evidence \
(for example the command output) in your reply.";

/// The prompt that starts a goal.
pub fn goal_prompt(condition: &str) -> String {
    format!("Goal: {condition}\n{GOAL_RULES}")
}

/// The prompt queued when the evaluator says not met.
pub fn continue_prompt(condition: &str, reason: &str) -> String {
    format!(
        "Continue working toward the goal: {condition}\n\
         Goal check: not met yet — {reason}\n{GOAL_RULES}"
    )
}

/// The model that checks a goal: `session.goal_model` (`provider/model`, or
/// a bare id on the session provider), else the session model. Not the
/// small fast model: in live runs a weak checker accepted workarounds.
pub fn checker_model(provider: &str, model: &str, goal_model: Option<&str>) -> (String, String) {
    match goal_model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(raw) => match raw.split_once('/') {
            Some((p, m)) => (p.to_string(), m.to_string()),
            None => (provider.to_string(), raw.to_string()),
        },
        None => (provider.to_string(), model.to_string()),
    }
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod tests;
