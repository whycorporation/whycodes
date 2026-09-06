//! Interactive questionnaire prompting (Grok-style `ask_user_question`).
//!
//! The agent intercepts the `question` tool and blocks until the UI (or stdin)
//! returns structured answers. Mirrors [`crate::permission`] channel pattern.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use crate::notify::{NotifyHandle, spawn_need_input_wait};
use whycodes_core::types::ApprovalMode;
use whycodes_tools::question::{
    QuestionAnswer, QuestionSpec, format_question_result, parse_questions, validate_answers,
};

/// `auto` auto-picks routine questions; `important: true` still prompts.
pub fn should_prompt_questions(mode: ApprovalMode, questions: &[QuestionSpec]) -> bool {
    match mode {
        ApprovalMode::Auto => questions.iter().any(|q| q.important),
        ApprovalMode::Important | ApprovalMode::Manual => true,
    }
}

/// Failure modes for a questionnaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionError {
    Cancelled,
    Timeout,
    Invalid(String),
    /// Channel closed (TUI exited).
    Disconnected,
}

impl QuestionError {
    pub fn message(&self) -> String {
        match self {
            Self::Cancelled => "User cancelled the questionnaire.".into(),
            Self::Timeout => "Timed out waiting for user answers.".into(),
            Self::Invalid(s) => format!("Invalid questionnaire: {s}"),
            Self::Disconnected => "Questionnaire UI disconnected.".into(),
        }
    }
}

/// Boxed, sendable future returned by [`QuestionPrompter::ask`].
pub type QuestionAskFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<QuestionAnswer>, QuestionError>> + Send + 'a>>;

/// Asked when the model calls the `question` tool.
pub trait QuestionPrompter: Send + Sync {
    fn ask(&self, questions: Vec<QuestionSpec>) -> QuestionAskFuture<'_>;
}

/// Pending request for the TUI (or other UI) to fulfill.
pub struct QuestionRequest {
    pub questions: Vec<QuestionSpec>,
    pub reply: oneshot::Sender<Result<Vec<QuestionAnswer>, QuestionError>>,
}

/// Channel-based prompter: blocks the agent until the UI replies.
pub struct ChannelQuestionPrompter {
    tx: mpsc::UnboundedSender<QuestionRequest>,
    timeout: Option<Duration>,
    notify: Option<NotifyHandle>,
}

impl ChannelQuestionPrompter {
    pub fn new(timeout: Option<Duration>) -> (Self, mpsc::UnboundedReceiver<QuestionRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                tx,
                timeout,
                notify: None,
            },
            rx,
        )
    }

    pub fn with_notify(mut self, notify: NotifyHandle) -> Self {
        self.notify = Some(notify);
        self
    }
}

impl QuestionPrompter for ChannelQuestionPrompter {
    fn ask(&self, questions: Vec<QuestionSpec>) -> QuestionAskFuture<'_> {
        Box::pin(async move {
            if let Some(cfg) = self.notify.as_deref() {
                let summary = questions
                    .first()
                    .map(|q| q.prompt.as_str())
                    .unwrap_or("questionnaire");
                spawn_need_input_wait(cfg, "Question", summary);
            }
            let (reply_tx, reply_rx) = oneshot::channel();
            if self
                .tx
                .send(QuestionRequest {
                    questions,
                    reply: reply_tx,
                })
                .is_err()
            {
                return Err(QuestionError::Disconnected);
            }
            match self.timeout {
                Some(dur) => match tokio::time::timeout(dur, reply_rx).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(_disconnected)) => Err(QuestionError::Disconnected),
                    Err(_timeout) => Err(QuestionError::Timeout),
                },
                None => reply_rx.await.unwrap_or(Err(QuestionError::Disconnected)),
            }
        })
    }
}

/// Auto-pick first option (or empty free-text) — CI / non-interactive.
pub struct AutoAnswerPrompter;

impl QuestionPrompter for AutoAnswerPrompter {
    fn ask(&self, questions: Vec<QuestionSpec>) -> QuestionAskFuture<'_> {
        Box::pin(async move {
            Ok(questions
                .iter()
                .map(|q| {
                    if let Some(opt) = q.options.first() {
                        QuestionAnswer {
                            selected: vec![opt.label.clone()],
                            free_text: None,
                            auto_picked: true,
                        }
                    } else {
                        QuestionAnswer {
                            selected: vec![],
                            free_text: Some("auto".into()),
                            auto_picked: true,
                        }
                    }
                })
                .collect())
        })
    }
}

/// Stdin fallback for plain CLI (delegates to tool module helpers via execute path).
pub struct StdinQuestionPrompter;

impl QuestionPrompter for StdinQuestionPrompter {
    fn ask(&self, questions: Vec<QuestionSpec>) -> QuestionAskFuture<'_> {
        Box::pin(async move { ask_stdin_questions(questions, read_line) })
    }
}

fn ask_stdin_questions(
    questions: Vec<QuestionSpec>,
    mut read_line: impl FnMut() -> Result<String, String>,
) -> Result<Vec<QuestionAnswer>, QuestionError> {
    use std::io::Write;
    let mut answers = Vec::with_capacity(questions.len());
    for (qi, q) in questions.iter().enumerate() {
        eprintln!();
        if questions.len() > 1 {
            eprintln!("── Question {}/{} ──", qi + 1, questions.len());
        }
        eprintln!("❓ {}", q.prompt);
        if q.options.is_empty() {
            eprint!("   Your answer: ");
            let _ = std::io::stderr().flush();
            let line = read_line().map_err(QuestionError::Invalid)?;
            answers.push(apply_free_stdin_parse(
                parse_stdin_question_line(q, &line),
                &line,
            )?);
            continue;
        }
        for (i, opt) in q.options.iter().enumerate() {
            if opt.description.is_empty() {
                eprintln!("  {}. {}", i + 1, opt.label);
            } else {
                eprintln!("  {}. {} — {}", i + 1, opt.label, opt.description);
            }
        }
        let other_n = q.options.len() + 1;
        eprintln!("  {other_n}. Other (type your own)");
        eprint!("   Choice: ");
        let _ = std::io::stderr().flush();
        let line = read_line().map_err(QuestionError::Invalid)?;
        let parsed = parse_stdin_question_line(q, &line);
        let other_text = if matches!(parsed, StdinQuestionParse::Other) {
            eprint!("   Other text: ");
            let _ = std::io::stderr().flush();
            Some(read_line().unwrap_or_default())
        } else {
            None
        };
        answers.push(apply_choice_stdin_parse(parsed, other_text.as_deref())?);
    }
    Ok(answers)
}

fn read_line() -> Result<String, String> {
    let mut line = String::new();
    finish_read_line(std::io::stdin().read_line(&mut line), line)
}

fn finish_read_line(result: Result<usize, std::io::Error>, line: String) -> Result<String, String> {
    match result {
        Ok(_) => Ok(trim_line(line)),
        Err(e) => Err(io_err_string(e)),
    }
}

fn trim_line(line: String) -> String {
    line.trim().to_string()
}

fn io_err_string(err: std::io::Error) -> String {
    err.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StdinQuestionParse {
    Cancelled,
    Selected(String),
    Other,
    FreeText(String),
}

fn parse_stdin_question_line(q: &QuestionSpec, line: &str) -> StdinQuestionParse {
    if line.is_empty() {
        return StdinQuestionParse::Cancelled;
    }
    if q.options.is_empty() {
        return StdinQuestionParse::FreeText(line.to_string());
    }
    if let Ok(n) = line.parse::<usize>() {
        if n >= 1 && n <= q.options.len() {
            return StdinQuestionParse::Selected(q.options[n - 1].label.clone());
        }
        if n == q.options.len() + 1 {
            return StdinQuestionParse::Other;
        }
    }
    StdinQuestionParse::FreeText(line.to_string())
}

fn apply_free_stdin_parse(
    parsed: StdinQuestionParse,
    line: &str,
) -> Result<QuestionAnswer, QuestionError> {
    match parsed {
        StdinQuestionParse::Cancelled => Err(QuestionError::Cancelled),
        StdinQuestionParse::FreeText(text) | StdinQuestionParse::Selected(text) => {
            Ok(QuestionAnswer {
                selected: vec![],
                free_text: Some(text),
                auto_picked: false,
            })
        }
        StdinQuestionParse::Other => Ok(QuestionAnswer {
            selected: vec![],
            free_text: Some(line.to_string()),
            auto_picked: false,
        }),
    }
}

fn apply_choice_stdin_parse(
    parsed: StdinQuestionParse,
    other_text: Option<&str>,
) -> Result<QuestionAnswer, QuestionError> {
    match parsed {
        StdinQuestionParse::Cancelled => Err(QuestionError::Cancelled),
        StdinQuestionParse::Selected(label) => Ok(QuestionAnswer {
            selected: vec![label],
            free_text: None,
            auto_picked: false,
        }),
        StdinQuestionParse::Other => {
            let t = other_text.unwrap_or("").trim();
            Ok(QuestionAnswer {
                selected: vec![],
                free_text: if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                },
                auto_picked: false,
            })
        }
        StdinQuestionParse::FreeText(text) => Ok(QuestionAnswer {
            selected: vec![],
            free_text: Some(text),
            auto_picked: false,
        }),
    }
}

/// Build default prompter for non-TUI:
/// - `WHYCODES_AUTO_APPROVE=1` → auto first option
/// - CI / non-interactive → auto
/// - else stdin
pub fn default_question_prompter() -> Arc<dyn QuestionPrompter> {
    if std::env::var("WHYCODES_AUTO_APPROVE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
        || std::env::var_os("CI").is_some()
    {
        return Arc::new(AutoAnswerPrompter);
    }
    Arc::new(StdinQuestionPrompter)
}

/// Parse tool call args and run the prompter; return a ToolResult body.
pub async fn run_question_tool(
    prompter: &dyn QuestionPrompter,
    arguments: &serde_json::Value,
    tool_call_id: &str,
) -> whycodes_core::types::ToolResult {
    let questions = match parse_questions(arguments) {
        Ok(q) => q,
        Err(e) => {
            return whycodes_core::types::ToolResult {
                tool_call_id: tool_call_id.to_string(),
                content: QuestionError::Invalid(e).message(),
                is_error: true,
            };
        }
    };

    match prompter.ask(questions.clone()).await {
        Ok(answers) => match validate_answers(&questions, &answers) {
            Ok(()) => whycodes_core::types::ToolResult {
                tool_call_id: tool_call_id.to_string(),
                content: format_question_result(&questions, &answers),
                is_error: false,
            },
            Err(e) => whycodes_core::types::ToolResult {
                tool_call_id: tool_call_id.to_string(),
                content: QuestionError::Invalid(e).message(),
                is_error: true,
            },
        },
        Err(e) => whycodes_core::types::ToolResult {
            tool_call_id: tool_call_id.to_string(),
            content: e.message(),
            is_error: true,
        },
    }
}

#[cfg(test)]
#[path = "question_tests.rs"]
mod tests;
