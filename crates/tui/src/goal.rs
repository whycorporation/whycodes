//! `/goal <condition>`: keep taking turns until a fast evaluator says the
//! condition holds (Claude Code's `/goal`).
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
const TRANSCRIPT_CHARS: usize = 12_000;

/// `state` table key holding a session's active goal condition.
pub fn state_key(session_id: &str) -> String {
    format!("goal:{session_id}")
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
    let key = state_key(session_id);
    let result = match goal {
        Some(goal) => db.set_state(&key, &goal.condition),
        None => db.delete_state(&key),
    };
    if let Err(e) = result {
        tracing::debug!(error = %e, "goal state not saved");
    }
}

/// The session's saved goal, restored active with fresh counters.
pub fn restore(
    db: Option<&whycodes_storage::db::Database>,
    session: &whycodes_session::session::Session,
) -> Option<Goal> {
    let condition = db?.get_state(&state_key(&session.id)).ok()??;
    Some(Goal::new(condition, session_tokens(session)))
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
    /// Turns evaluated since the goal was set (or restored).
    pub turns: u32,
    /// Consecutive finished turns with no tool call.
    pub idle_turns: u32,
    /// Session output tokens when the goal was set, for spend.
    pub tokens_at_start: u64,
    /// The evaluator's latest reason.
    pub last_reason: Option<String>,
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
            idle_turns: 0,
            tokens_at_start: tokens_now,
            last_reason: None,
            paused: None,
            evaluating: false,
        }
    }

    /// Record a finished turn. A paused goal resumes here: while paused no
    /// turn is queued, so any turn that finishes was the user's prompt.
    pub fn record_turn(&mut self, used_tools: bool) -> TurnStep {
        self.paused = None;
        self.turns += 1;
        self.idle_turns = if used_tools { 0 } else { self.idle_turns + 1 };
        if self.idle_turns >= IDLE_TURN_LIMIT {
            self.pause(format!("no tool use for {IDLE_TURN_LIMIT} turns in a row"));
            return TurnStep::Pause;
        }
        self.evaluating = true;
        TurnStep::Evaluate
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

/// Read `VERDICT: …` / `REASON: …`. Anything unclear is not met, so a vague
/// evaluator keeps the agent working instead of declaring success.
pub fn parse_verdict(text: &str) -> (Verdict, String) {
    let mut verdict = None;
    let mut reason = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("verdict:") {
            let word = rest
                .trim()
                .trim_matches(|c: char| !c.is_ascii_alphabetic() && c != '_');
            verdict = Some(match word {
                "met" => Verdict::Met,
                "impossible" => Verdict::Impossible,
                _ => Verdict::NotMet,
            });
        } else if lower.starts_with("reason:") {
            reason = trimmed["reason:".len()..].trim().to_string();
        }
    }
    if reason.is_empty() {
        reason = text.trim().lines().last().unwrap_or("").trim().to_string();
    }
    (verdict.unwrap_or(Verdict::NotMet), reason)
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

const EVALUATOR_SYSTEM: &str = "You check whether a coding agent has met a goal. \
You cannot run commands or read files: judge only from the transcript. \
Say met only when the transcript shows evidence (command output, test results, \
the finished change). Say impossible only when the goal cannot be reached. \
Answer in exactly two lines:\nVERDICT: met | not_met | impossible\nREASON: one sentence";

/// No-tools request for the evaluator model.
pub fn evaluator_request(condition: &str, turns: u32, messages: &[Message]) -> LlmRequest {
    // The turn count lets a condition like "or stop after 20 turns" be judged;
    // the transcript alone is too short to count them.
    let body = format!(
        "Goal:\n{condition}\n\nTurns taken toward this goal: {turns}\n\n\
         Transcript (most recent last):\n{}",
        transcript(messages)
    );
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
        max_tokens: Some(200),
        temperature: Some(0.0),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

/// The prompt queued when the evaluator says not met.
pub fn continue_prompt(condition: &str, reason: &str) -> String {
    format!(
        "Continue working toward the goal: {condition}\n\
         Goal check: not met yet — {reason}\n\
         When it is met, show the evidence (for example the command output) in your reply."
    )
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod tests;
