//! Interactive questionnaire tool (`question`).
//!
//! Schema is Grok-style: one or more questions, each with labelled options,
//! optional multi-select, and an implicit **Other** free-text path.
//!
//! Execution is UI-backed when the agent installs a [`QuestionPrompter`]-style
//! channel (TUI). This module owns parsing + result formatting + a stdin
//! fallback for plain CLI / tests.

use serde_json::json;
use std::io::{self, Write};

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

#[cfg(test)]
static TEST_STDIN: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// Soft product cap (schema says “max 4 recommended”). Hard-refuse dumps.
const MAX_QUESTIONS: usize = 8;

// ── Public types (shared by agent prompter + TUI) ──────────────────────

/// One selectable option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
    pub preview: Option<String>,
}

/// One question in a questionnaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionSpec {
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    /// When true, `approval_mode=auto` still shows the TUI/SDK prompt.
    /// Routine questions (false) are auto-picked in auto.
    pub important: bool,
}

/// User response for one question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswer {
    /// Labels of chosen predefined options (empty if only free-text).
    pub selected: Vec<String>,
    /// Free-text when Other was used (or sole free-form answer).
    pub free_text: Option<String>,
    /// Set when `AutoAnswerPrompter` filled this — not a user choice.
    pub auto_picked: bool,
}

impl QuestionAnswer {
    pub fn summary(&self) -> String {
        let mut parts = self.selected.clone();
        if let Some(ref t) = self.free_text {
            let t = t.trim();
            if !t.is_empty() {
                parts.push(format!("Other: {t}"));
            }
        }
        if parts.is_empty() {
            "(no selection)".into()
        } else {
            parts.join("; ")
        }
    }
}

/// Reject answers that do not match the questionnaire.
pub fn validate_answers(
    questions: &[QuestionSpec],
    answers: &[QuestionAnswer],
) -> Result<(), String> {
    if answers.len() != questions.len() {
        return Err(format!(
            "expected {} answers, got {}",
            questions.len(),
            answers.len()
        ));
    }
    for (i, (q, a)) in questions.iter().zip(answers.iter()).enumerate() {
        let labels: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
        for sel in &a.selected {
            if !labels.iter().any(|l| *l == sel) {
                return Err(format!(
                    "answers[{i}]: unknown option `{sel}` (not in this question)"
                ));
            }
        }
        if q.multi_select {
            if a.selected.is_empty() && a.free_text.as_ref().is_none_or(|t| t.trim().is_empty()) {
                return Err(format!(
                    "answers[{i}]: select at least one option or Other text"
                ));
            }
        } else if too_many_single_select(&a.selected) {
            return Err(format!(
                "answers[{i}]: single-select question got multiple labels"
            ));
        }
        if q.options.is_empty() && a.free_text.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(format!("answers[{i}]: free-form question requires text"));
        }
        if a.selected.is_empty() && a.free_text.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(format!(
                "answers[{i}]: empty selection (pick an option or provide Other text)"
            ));
        }
    }
    Ok(())
}

fn too_many_single_select(selected: &[String]) -> bool {
    selected.len() > 1
}

/// Parse tool arguments into question specs.
///
/// Accepts:
/// - Grok-style: `{ "questions": [ { "question", "options": [{label,description}], "multi_select" } ] }`
/// - Legacy: `{ "question": "...", "choices": ["a","b"] }`
pub fn parse_questions(args: &serde_json::Value) -> Result<Vec<QuestionSpec>, String> {
    if let Some(arr) = args.get("questions").and_then(|v| v.as_array()) {
        if arr.is_empty() {
            return Err("questions array must not be empty".into());
        }
        if arr.len() > MAX_QUESTIONS {
            return Err(format!(
                "at most {MAX_QUESTIONS} questions (got {})",
                arr.len()
            ));
        }
        let mut out = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            out.push(parse_one_question(item).map_err(|e| format!("questions[{i}]: {e}"))?);
        }
        return Ok(out);
    }

    let prompt = args
        .get("question")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "provide `questions` or a non-empty `question` string".to_string())?;

    let options = parse_choices_or_options(args)?;
    let multi_select = args
        .get("multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let important = args
        .get("important")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    Ok(vec![QuestionSpec {
        prompt: prompt.to_string(),
        options,
        multi_select,
        important,
    }])
}

fn parse_one_question(item: &serde_json::Value) -> Result<QuestionSpec, String> {
    let prompt = item
        .get("question")
        .or_else(|| item.get("prompt"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing question text".to_string())?;

    let options = parse_choices_or_options(item)?;
    let multi_select = item
        .get("multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let important = item
        .get("important")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    Ok(QuestionSpec {
        prompt: prompt.to_string(),
        options,
        multi_select,
        important,
    })
}

fn parse_choices_or_options(item: &serde_json::Value) -> Result<Vec<QuestionOption>, String> {
    if let Some(opts) = item.get("options").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for (i, o) in opts.iter().enumerate() {
            if let Some(s) = o.as_str() {
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                out.push(QuestionOption {
                    label: s.to_string(),
                    description: String::new(),
                    preview: None,
                });
                continue;
            }
            let label = o
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("options[{i}]: missing label"))?
                .to_string();
            let description = o
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let preview = o
                .get("preview")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            out.push(QuestionOption {
                label,
                description,
                preview,
            });
        }
        if out.is_empty() {
            // Models often emit `"options": []` for free-form; treat like omitted.
            return Ok(Vec::new());
        }
        return Ok(out);
    }

    if let Some(choices) = item.get("choices").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for c in choices {
            if let Some(s) = c.as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    out.push(QuestionOption {
                        label: s.to_string(),
                        description: String::new(),
                        preview: None,
                    });
                }
            }
        }
        if out.is_empty() {
            return Err("choices must contain at least one non-empty string".into());
        }
        return Ok(out);
    }

    // Free-form only (no options) — UI will offer Other / free text.
    Ok(Vec::new())
}

/// Format answers for the model (tool result body).
pub fn format_question_result(questions: &[QuestionSpec], answers: &[QuestionAnswer]) -> String {
    let mut out = String::new();
    for (i, (q, a)) in questions.iter().zip(answers.iter()).enumerate() {
        if questions.len() > 1 {
            out.push_str(&format!("### Question {}\n", i + 1));
        }
        out.push_str(&format!("Question: {}\n", q.prompt));
        out.push_str(&format!("Answer: {}", a.summary()));
        if a.auto_picked {
            out.push_str("  (auto-picked; approval_mode=auto — not a user choice)");
        }
        out.push('\n');
        if i + 1 < questions.len() {
            out.push('\n');
        }
    }
    if out.is_empty() {
        "No answers.".into()
    } else {
        out
    }
}

// ── Tool ───────────────────────────────────────────────────────────────

pub struct QuestionTool;

impl Default for QuestionTool {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }

    fn description(&self) -> &str {
        "Ask the user one or more clarifying questions with optional multiple-choice \
         options. Prefer this over guessing when requirements, approach, or risk are \
         ambiguous. Each question may set multi_select. The UI always offers an Other \
         free-text path. Use short labels and helpful descriptions; put the recommended \
         option first."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "One or more questions (preferred). Max 4 recommended.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The question text"
                            },
                            "options": {
                                "type": "array",
                                "description": "Choices (label + description). Other is added automatically.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Short option label (a few words)"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "What choosing this option means"
                                        },
                                        "preview": {
                                            "type": "string",
                                            "description": "Optional extra detail shown when focused"
                                        }
                                    },
                                    "required": ["label"]
                                }
                            },
                            "choices": {
                                "type": "array",
                                "description": "Legacy: plain string options (same as options[].label)",
                                "items": { "type": "string" }
                            },
                            "multi_select": {
                                "type": "boolean",
                                "description": "Allow selecting more than one option (default false)"
                            },
                            "important": {
                                "type": "boolean",
                                "description": "If true, prompt even in approval_mode=auto (destructive / irreversible). Default false."
                            }
                        },
                        "required": ["question"]
                    }
                },
                "question": {
                    "type": "string",
                    "description": "Legacy single-question form"
                },
                "choices": {
                    "type": "array",
                    "description": "Legacy string choices for the single-question form",
                    "items": { "type": "string" }
                },
                "multi_select": {
                    "type": "boolean",
                    "description": "Legacy multi_select for the single-question form"
                },
                "important": {
                    "type": "boolean",
                    "description": "Legacy: if true, prompt even in auto mode"
                }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            // Fallback path when the agent did not intercept (plain CLI / tests).
            let questions = match parse_questions(&args) {
                Ok(q) => q,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Invalid question arguments: {e}"),
                        is_error: true,
                    };
                }
            };

            match stdin_questionnaire(&questions) {
                Ok(answers) => ToolResult {
                    tool_call_id: String::new(),
                    content: format_question_result(&questions, &answers),
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: e,
                    is_error: true,
                },
            }
        })
    }
}

pub fn stdin_questionnaire(questions: &[QuestionSpec]) -> Result<Vec<QuestionAnswer>, String> {
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
            let line = read_line_stdin()?;
            if line.is_empty() {
                return Err("No answer received (empty input).".into());
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
        if q.multi_select {
            eprint!("   Enter numbers separated by comma, or text: ");
        } else {
            eprint!("   Enter number or type your answer: ");
        }
        let _ = io::stderr().flush();
        let line = read_line_stdin()?;
        if line.is_empty() {
            return Err("No answer received (empty input).".into());
        }
        answers.push(resolve_stdin_answer(q, &line, other_n));
    }
    Ok(answers)
}

fn resolve_stdin_answer(q: &QuestionSpec, line: &str, other_n: usize) -> QuestionAnswer {
    if q.multi_select {
        let mut selected = Vec::new();
        let mut free = None;
        for part in line.split([',', ' ']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Ok(n) = part.parse::<usize>() {
                if n >= 1 && n <= q.options.len() {
                    selected.push(q.options[n - 1].label.clone());
                } else if n == other_n {
                    free = Some(String::new());
                }
            } else {
                free = Some(part.to_string());
            }
        }
        if free.as_ref().is_some_and(|s| s.is_empty()) {
            eprint!("   Other text: ");
            let _ = io::stderr().flush();
            free = read_line_stdin().ok().filter(|s| !s.is_empty());
        }
        if selected.is_empty() && free.is_none() {
            free = Some(line.to_string());
        }
        return QuestionAnswer {
            selected,
            free_text: free,
            auto_picked: false,
        };
    }

    if let Ok(n) = line.parse::<usize>() {
        if n >= 1 && n <= q.options.len() {
            return QuestionAnswer {
                selected: vec![q.options[n - 1].label.clone()],
                free_text: None,
                auto_picked: false,
            };
        }
        if n == other_n {
            eprint!("   Other text: ");
            let _ = io::stderr().flush();
            let t = read_line_stdin().unwrap_or_default();
            return QuestionAnswer {
                selected: vec![],
                free_text: if t.is_empty() { None } else { Some(t) },
                auto_picked: false,
            };
        }
    }
    // Typed answer: match label case-insensitively or treat as free text
    for opt in &q.options {
        if opt.label.eq_ignore_ascii_case(line) {
            return QuestionAnswer {
                selected: vec![opt.label.clone()],
                free_text: None,
                auto_picked: false,
            };
        }
    }
    QuestionAnswer {
        selected: vec![],
        free_text: Some(line.to_string()),
        auto_picked: false,
    }
}

fn read_line_stdin() -> Result<String, String> {
    #[cfg(test)]
    if let Some(line) = take_test_stdin() {
        return Ok(line);
    }
    read_line_from(&mut io::stdin().lock())
}

#[cfg(test)]
fn take_test_stdin() -> Option<String> {
    TEST_STDIN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front()
}

fn read_line_from(reader: &mut impl io::BufRead) -> Result<String, String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read input: {e}"))?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "question_tests.rs"]
mod tests;
