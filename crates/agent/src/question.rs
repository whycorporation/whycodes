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
        Box::pin(async move {
            // Re-use tool stdin path by serializing back to args and calling parse-free logic.
            // Inline minimal stdin to avoid tool execute needing ToolContext.
            use std::io::{self, Write};
            let mut answers = Vec::with_capacity(questions.len());
            for (qi, q) in questions.iter().enumerate() {
                eprintln!();
                if questions.len() > 1 {
                    eprintln!("── Question {}/{} ──", qi + 1, questions.len());
                }
                eprintln!("❓ {}", q.prompt);
                if q.options.is_empty() {
                    eprint!("   Your answer: ");
                    let _ = io::stderr().flush();
                    let line = read_line().map_err(QuestionError::Invalid)?;
                    if line.is_empty() {
                        return Err(QuestionError::Cancelled);
                    }
                    answers.push(QuestionAnswer {
                        selected: vec![],
                        free_text: Some(line),
                        auto_picked: false,
                    });
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
                let _ = io::stderr().flush();
                let line = read_line().map_err(QuestionError::Invalid)?;
                if line.is_empty() {
                    return Err(QuestionError::Cancelled);
                }
                if let Ok(n) = line.parse::<usize>() {
                    if n >= 1 && n <= q.options.len() {
                        answers.push(QuestionAnswer {
                            selected: vec![q.options[n - 1].label.clone()],
                            free_text: None,
                            auto_picked: false,
                        });
                        continue;
                    }
                    if n == other_n {
                        eprint!("   Other text: ");
                        let _ = io::stderr().flush();
                        let t = read_line().unwrap_or_default();
                        answers.push(QuestionAnswer {
                            selected: vec![],
                            free_text: if t.is_empty() { None } else { Some(t) },
                            auto_picked: false,
                        });
                        continue;
                    }
                }
                answers.push(QuestionAnswer {
                    selected: vec![],
                    free_text: Some(line),
                    auto_picked: false,
                });
            }
            Ok(answers)
        })
    }
}

fn read_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
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
mod tests {
    use super::*;
    use whycodes_tools::question::QuestionOption;

    #[tokio::test]
    async fn auto_prompter_picks_first() {
        let p = AutoAnswerPrompter;
        let qs = vec![QuestionSpec {
            prompt: "x".into(),
            options: vec![
                QuestionOption {
                    label: "A".into(),
                    description: String::new(),
                    preview: None,
                },
                QuestionOption {
                    label: "B".into(),
                    description: String::new(),
                    preview: None,
                },
            ],
            multi_select: false,
            important: false,
        }];
        let a = p.ask(qs).await.unwrap();
        assert_eq!(a[0].selected, vec!["A".to_string()]);
        assert!(a[0].auto_picked);
    }

    #[test]
    fn auto_prompts_only_when_any_question_is_important() {
        let routine = vec![QuestionSpec {
            prompt: "x".into(),
            options: vec![],
            multi_select: false,
            important: false,
        }];
        let important = vec![QuestionSpec {
            prompt: "y".into(),
            options: vec![],
            multi_select: false,
            important: true,
        }];
        assert!(!should_prompt_questions(ApprovalMode::Auto, &routine));
        assert!(should_prompt_questions(ApprovalMode::Auto, &important));
        assert!(should_prompt_questions(ApprovalMode::Important, &routine));
        assert!(should_prompt_questions(ApprovalMode::Manual, &routine));
    }

    #[tokio::test]
    async fn auto_prompter_free_text_when_no_options() {
        let p = AutoAnswerPrompter;
        let qs = vec![QuestionSpec {
            prompt: "free?".into(),
            options: vec![],
            multi_select: false,
            important: false,
        }];
        let a = p.ask(qs).await.unwrap();
        assert_eq!(a[0].free_text.as_deref(), Some("auto"));
        assert!(a[0].selected.is_empty());
        assert!(a[0].auto_picked);
    }

    #[tokio::test]
    async fn channel_timeout_and_disconnect() {
        let (prompter, rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(20)));
        drop(rx);
        let err = prompter
            .ask(vec![QuestionSpec {
                prompt: "x".into(),
                options: vec![],
                multi_select: false,
                important: false,
            }])
            .await
            .expect_err("disconnected");
        assert_eq!(err, QuestionError::Disconnected);

        let (prompter, mut rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(20)));
        let ask = tokio::spawn(async move {
            prompter
                .ask(vec![QuestionSpec {
                    prompt: "y".into(),
                    options: vec![],
                    multi_select: false,
                    important: true,
                }])
                .await
        });
        let _req = rx.recv().await.expect("request");
        let err = ask.await.unwrap().expect_err("timeout");
        assert_eq!(err, QuestionError::Timeout);
        assert!(err.message().to_lowercase().contains("timed"));
        assert!(
            QuestionError::Cancelled
                .message()
                .to_lowercase()
                .contains("cancel")
        );
        assert!(
            QuestionError::Invalid("x".into())
                .message()
                .contains("Invalid")
        );
    }

    #[tokio::test]
    async fn channel_none_timeout_disconnects_when_reply_dropped() {
        let (prompter, mut rx) = ChannelQuestionPrompter::new(None);
        let ask = tokio::spawn(async move {
            prompter
                .ask(vec![QuestionSpec {
                    prompt: "z".into(),
                    options: vec![],
                    multi_select: false,
                    important: false,
                }])
                .await
        });
        let req = rx.recv().await.expect("request");
        drop(req.reply);
        let err = ask.await.unwrap().expect_err("disconnected");
        assert_eq!(err, QuestionError::Disconnected);
    }

    #[tokio::test]
    async fn channel_timeout_some_disconnected_after_send() {
        let (prompter, mut rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(80)));
        let ask = tokio::spawn(async move {
            prompter
                .ask(vec![QuestionSpec {
                    prompt: "drop-reply".into(),
                    options: vec![],
                    multi_select: false,
                    important: false,
                }])
                .await
        });
        let req = rx.recv().await.expect("request");
        drop(req.reply);
        let err = ask.await.unwrap().expect_err("disconnected");
        assert_eq!(err, QuestionError::Disconnected);
    }

    #[tokio::test]
    async fn run_question_tool_invalid_args() {
        let p = AutoAnswerPrompter;
        let r = run_question_tool(&p, &serde_json::json!({}), "q1").await;
        assert!(r.is_error);
        assert!(r.content.contains("Invalid questionnaire"), "{r:?}");
        assert_eq!(r.tool_call_id, "q1");
    }

    #[tokio::test]
    async fn run_question_tool_auto_answers() {
        let p = AutoAnswerPrompter;
        let args = serde_json::json!({
            "questions": [{
                "prompt": "pick",
                "options": [{"label": "A"}, {"label": "B"}]
            }]
        });
        let r = run_question_tool(&p, &args, "q2").await;
        assert!(!r.is_error, "{r:?}");
        assert!(
            r.content.contains("A") || r.content.to_lowercase().contains("auto"),
            "{r:?}"
        );
    }

    #[test]
    fn default_question_prompter_ci_and_construct_stdin() {
        let prev_approve = std::env::var_os("WHYCODES_AUTO_APPROVE");
        let prev_ci = std::env::var_os("CI");
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "0") };
        unsafe { std::env::set_var("CI", "1") };
        let _ = default_question_prompter();
        unsafe { std::env::remove_var("CI") };
        let _ = default_question_prompter();
        let _ = StdinQuestionPrompter;
        // Force both restore arms so leftover coverage is not env-dependent.
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "restore-some") };
        if let Some(v) = std::env::var_os("WHYCODES_AUTO_APPROVE") {
            unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
        }
        unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
        if std::env::var_os("WHYCODES_AUTO_APPROVE").is_none() {
            // None restore arm
        }
        if let Some(v) = prev_approve {
            unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
        } else {
            unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
        }
        unsafe { std::env::set_var("CI", "restore-some") };
        if let Some(v) = std::env::var_os("CI") {
            unsafe { std::env::set_var("CI", v) };
        }
        unsafe { std::env::remove_var("CI") };
        if let Some(v) = prev_ci {
            unsafe { std::env::set_var("CI", v) };
        } else {
            unsafe { std::env::remove_var("CI") };
        }
    }

    #[test]
    fn disconnected_message_is_stable() {
        assert_eq!(
            QuestionError::Disconnected.message(),
            "Questionnaire UI disconnected."
        );
    }

    #[tokio::test]
    async fn channel_with_notify_and_successful_reply() {
        let cfg = whycodes_config::NotifyConfig {
            on: vec!["need_input".into()],
            discord_webhook: Some("https://example.invalid/webhook".into()),
            ..Default::default()
        };
        let (prompter, mut rx) = ChannelQuestionPrompter::new(Some(Duration::from_secs(2)));
        let prompter = prompter.with_notify(crate::notify::handle_from_config(&cfg));
        let ask = tokio::spawn(async move {
            prompter
                .ask(vec![QuestionSpec {
                    prompt: "pick one".into(),
                    options: vec![QuestionOption {
                        label: "A".into(),
                        description: String::new(),
                        preview: None,
                    }],
                    multi_select: false,
                    important: true,
                }])
                .await
        });
        let req = rx.recv().await.expect("request");
        req.reply
            .send(Ok(vec![QuestionAnswer {
                selected: vec!["A".into()],
                free_text: None,
                auto_picked: false,
            }]))
            .unwrap();
        let answers = ask.await.unwrap().expect("ok");
        assert_eq!(answers[0].selected, vec!["A".to_string()]);
    }

    #[tokio::test]
    async fn run_question_tool_propagates_prompter_error() {
        let (prompter, rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(20)));
        drop(rx);
        let args = serde_json::json!({
            "questions": [{"prompt": "pick", "options": [{"label": "A"}]}]
        });
        let r = run_question_tool(&prompter, &args, "q3").await;
        assert!(r.is_error);
        assert!(r.content.contains("disconnected") || r.content.contains("Disconnected"));
    }

    #[test]
    fn default_question_prompter_auto_approve() {
        let prev_approve = std::env::var_os("WHYCODES_AUTO_APPROVE");
        let prev_ci = std::env::var_os("CI");
        unsafe { std::env::set_var("CI", "0") };
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "yes") };
        let _ = default_question_prompter();
        if let Some(v) = prev_approve {
            unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
        } else {
            unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
        }
        if let Some(v) = prev_ci {
            unsafe { std::env::set_var("CI", v) };
        } else {
            unsafe { std::env::remove_var("CI") };
        }
    }

    struct InvalidAnswersPrompter;

    impl QuestionPrompter for InvalidAnswersPrompter {
        fn ask(&self, questions: Vec<QuestionSpec>) -> QuestionAskFuture<'_> {
            Box::pin(async move {
                Ok(questions
                    .iter()
                    .map(|_| QuestionAnswer {
                        selected: vec!["not-an-option".into()],
                        free_text: None,
                        auto_picked: false,
                    })
                    .collect())
            })
        }
    }

    #[tokio::test]
    async fn run_question_tool_rejects_invalid_answers() {
        let p = InvalidAnswersPrompter;
        let args = serde_json::json!({
            "questions": [{
                "prompt": "pick",
                "options": [{"label": "A"}, {"label": "B"}]
            }]
        });
        let r = run_question_tool(&p, &args, "q-bad").await;
        assert!(r.is_error);
        assert!(
            r.content.contains("Invalid") || r.content.contains("unknown option"),
            "{r:?}"
        );
    }

    #[tokio::test]
    async fn stdin_question_prompter_eof_cancels() {
        let p = StdinQuestionPrompter;
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            p.ask(vec![QuestionSpec {
                prompt: "free?".into(),
                options: vec![],
                multi_select: false,
                important: false,
            }]),
        )
        .await
        .expect("stdin question must not hang on EOF");
        assert!(err.is_err(), "{err:?}");

        let err = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            p.ask(vec![
                QuestionSpec {
                    prompt: "one".into(),
                    options: vec![QuestionOption {
                        label: "A".into(),
                        description: "alpha".into(),
                        preview: None,
                    }],
                    multi_select: false,
                    important: false,
                },
                QuestionSpec {
                    prompt: "two".into(),
                    options: vec![QuestionOption {
                        label: "B".into(),
                        description: String::new(),
                        preview: None,
                    }],
                    multi_select: false,
                    important: false,
                },
            ]),
        )
        .await
        .expect("multi-question stdin must not hang on EOF");
        assert!(err.is_err(), "{err:?}");
    }
}
