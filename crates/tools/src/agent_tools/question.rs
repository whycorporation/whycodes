//! Interactive questionnaire tool (`question`).
//!
//! Schema is Grok-style: one or more questions, each with labelled options,
//! optional multi-select, and an implicit **Other** free-text path.
//!
//! Execution is UI-backed when the agent installs a [`QuestionPrompter`]-style
//! channel (TUI). This module owns parsing + result formatting + a stdin
//! fallback for plain CLI / tests.

use async_trait::async_trait;
use serde_json::json;
use std::io::{self, Write};

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

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
}

/// User response for one question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswer {
    /// Labels of chosen predefined options (empty if only free-text).
    pub selected: Vec<String>,
    /// Free-text when Other was used (or sole free-form answer).
    pub free_text: Option<String>,
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

    Ok(vec![QuestionSpec {
        prompt: prompt.to_string(),
        options,
        multi_select,
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

    Ok(QuestionSpec {
        prompt: prompt.to_string(),
        options,
        multi_select,
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
            return Err("options must contain at least one entry".into());
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
        out.push_str(&format!("Answer: {}\n", a.summary()));
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

#[async_trait]
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
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
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
    }
}

fn stdin_questionnaire(questions: &[QuestionSpec]) -> Result<Vec<QuestionAnswer>, String> {
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
        };
    }

    if let Ok(n) = line.parse::<usize>() {
        if n >= 1 && n <= q.options.len() {
            return QuestionAnswer {
                selected: vec![q.options[n - 1].label.clone()],
                free_text: None,
            };
        }
        if n == other_n {
            eprint!("   Other text: ");
            let _ = io::stderr().flush();
            let t = read_line_stdin().unwrap_or_default();
            return QuestionAnswer {
                selected: vec![],
                free_text: if t.is_empty() { None } else { Some(t) },
            };
        }
    }
    // Typed answer: match label case-insensitively or treat as free text
    for opt in &q.options {
        if opt.label.eq_ignore_ascii_case(line) {
            return QuestionAnswer {
                selected: vec![opt.label.clone()],
                free_text: None,
            };
        }
    }
    QuestionAnswer {
        selected: vec![],
        free_text: Some(line.to_string()),
    }
}

fn read_line_stdin() -> Result<String, String> {
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read input: {e}"))?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WAVE4_QUESTIONNAIRE: &str = r#"
    {
      "questions": [
        {
          "question": "  Which storage?  ",
          "multi_select": false,
          "options": [
            {
              "label": "  SQLite  ",
              "description": "  Simple local storage  ",
              "preview": "One file"
            },
            " Postgres "
          ]
        },
        {
          "prompt": "Select features",
          "multi_select": true,
          "choices": ["Search", "  Export  ", ""]
        },
        {
          "question": "Anything else?"
        }
      ]
    }
    "#;

    fn fixture(source: &str) -> serde_json::Value {
        serde_json::from_str(source).expect("test fixture must be valid JSON")
    }

    #[test]
    fn parse_legacy_choices() {
        let q = parse_questions(&json!({
            "question": "Pick one",
            "choices": ["A", "B"]
        }))
        .unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].prompt, "Pick one");
        assert_eq!(q[0].options.len(), 2);
        assert_eq!(q[0].options[0].label, "A");
    }

    #[test]
    fn parse_grok_style_questions() {
        let questions = parse_questions(&fixture(WAVE4_QUESTIONNAIRE)).unwrap();

        assert_eq!(questions.len(), 3);
        assert_eq!(questions[0].prompt, "Which storage?");
        assert_eq!(questions[0].options[0].label, "SQLite");
        assert_eq!(questions[0].options[0].description, "Simple local storage");
        assert_eq!(questions[0].options[0].preview.as_deref(), Some("One file"));
        assert_eq!(questions[0].options[1].label, "Postgres");
        assert!(questions[0].options[1].description.is_empty());
        assert!(!questions[0].multi_select);
        assert_eq!(
            questions[1]
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            ["Search", "Export"]
        );
        assert!(questions[1].multi_select);
        assert!(questions[2].options.is_empty());
    }

    #[test]
    fn parameters_describe_preferred_and_legacy_schemas() {
        let schema = QuestionTool::new().parameters();

        assert_eq!(schema["type"], "object");
        let properties = schema["properties"].as_object().unwrap();
        for name in ["questions", "question", "choices", "multi_select"] {
            assert!(properties.contains_key(name), "missing {name} schema");
        }
        let question = &schema["properties"]["questions"]["items"];
        assert_eq!(question["required"], json!(["question"]));
        assert_eq!(question["properties"]["options"]["type"], "array");
        assert_eq!(
            question["properties"]["options"]["items"]["required"],
            json!(["label"])
        );
        assert_eq!(question["properties"]["multi_select"]["type"], "boolean");
        assert_eq!(schema["properties"]["choices"]["items"]["type"], "string");
    }

    #[test]
    fn validation_rejects_malformed_question_fixtures() {
        let cases = [
            (
                r#"{}"#,
                "provide `questions` or a non-empty `question` string",
            ),
            (
                r#"{"question":"   "}"#,
                "provide `questions` or a non-empty `question` string",
            ),
            (r#"{"questions":[]}"#, "questions array must not be empty"),
            (
                r#"{"questions":[{"options":["A"]}]}"#,
                "questions[0]: missing question text",
            ),
            (
                r#"{"questions":[{"question":"Pick","options":[]}] }"#,
                "questions[0]: options must contain at least one entry",
            ),
            (
                r#"{"questions":[{"question":"Pick","options":[{"description":"none"}]}]}"#,
                "questions[0]: options[0]: missing label",
            ),
            (
                r#"{"question":"Pick","choices":["",7]}"#,
                "choices must contain at least one non-empty string",
            ),
        ];

        for (source, expected) in cases {
            let error = parse_questions(&fixture(source)).unwrap_err();
            assert_eq!(error, expected, "fixture: {source}");
        }
    }

    #[test]
    fn answer_summary_and_result_render_all_states() {
        let questions = parse_questions(&fixture(WAVE4_QUESTIONNAIRE)).unwrap();
        let answers = [
            QuestionAnswer {
                selected: vec!["SQLite".into()],
                free_text: None,
            },
            QuestionAnswer {
                selected: vec!["Search".into(), "Export".into()],
                free_text: Some("  Audit log  ".into()),
            },
            QuestionAnswer {
                selected: vec![],
                free_text: Some("  ".into()),
            },
        ];

        assert_eq!(answers[0].summary(), "SQLite");
        assert_eq!(answers[1].summary(), "Search; Export; Other: Audit log");
        assert_eq!(answers[2].summary(), "(no selection)");
        assert_eq!(
            format_question_result(&questions, &answers),
            "### Question 1\nQuestion: Which storage?\nAnswer: SQLite\n\n\
             ### Question 2\nQuestion: Select features\nAnswer: Search; Export; Other: Audit log\n\n\
             ### Question 3\nQuestion: Anything else?\nAnswer: (no selection)\n"
        );
        assert_eq!(format_question_result(&[], &[]), "No answers.");
    }

    #[test]
    fn resolve_answer_tracks_single_multi_and_free_text_state() {
        let questions = parse_questions(&fixture(WAVE4_QUESTIONNAIRE)).unwrap();

        assert_eq!(
            resolve_stdin_answer(&questions[0], "2", 3),
            QuestionAnswer {
                selected: vec!["Postgres".into()],
                free_text: None,
            }
        );
        assert_eq!(
            resolve_stdin_answer(&questions[0], "sqlite", 3),
            QuestionAnswer {
                selected: vec!["SQLite".into()],
                free_text: None,
            }
        );
        assert_eq!(
            resolve_stdin_answer(&questions[0], "custom", 3),
            QuestionAnswer {
                selected: vec![],
                free_text: Some("custom".into()),
            }
        );
        assert_eq!(
            resolve_stdin_answer(&questions[1], "1, 2 extra", 3),
            QuestionAnswer {
                selected: vec!["Search".into(), "Export".into()],
                free_text: Some("extra".into()),
            }
        );
        assert_eq!(
            resolve_stdin_answer(&questions[1], "99", 3),
            QuestionAnswer {
                selected: vec![],
                free_text: Some("99".into()),
            }
        );
    }

    #[tokio::test]
    async fn execute_reports_invalid_state_without_reading_stdin() {
        let result = QuestionTool::new()
            .execute(
                fixture(r#"{"questions":[{"question":"Pick","options":[]}]}"#),
                &ToolContext::unsandboxed("."),
            )
            .await;

        assert!(result.is_error);
        assert!(result.tool_call_id.is_empty());
        assert_eq!(
            result.content,
            "Invalid question arguments: questions[0]: options must contain at least one entry"
        );
    }
}
