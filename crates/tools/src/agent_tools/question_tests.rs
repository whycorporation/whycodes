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
    assert!(!q[0].important);
}

#[test]
fn parse_important_flag() {
    let q = parse_questions(&json!({
        "questions": [{
            "question": "Delete prod?",
            "options": [{"label": "Yes"}, {"label": "No"}],
            "important": true
        }]
    }))
    .unwrap();
    assert!(q[0].important);
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
            r#"{"questions":[{"question":"Pick","options":[{"description":"none"}]}]}"#,
            "questions[0]: options[0]: missing label",
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
            auto_picked: false,
        },
        QuestionAnswer {
            selected: vec!["Search".into(), "Export".into()],
            free_text: Some("  Audit log  ".into()),
            auto_picked: false,
        },
        QuestionAnswer {
            selected: vec![],
            free_text: Some("  ".into()),
            auto_picked: false,
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
            auto_picked: false,
        }
    );
    assert_eq!(
        resolve_stdin_answer(&questions[0], "sqlite", 3),
        QuestionAnswer {
            selected: vec!["SQLite".into()],
            free_text: None,
            auto_picked: false,
        }
    );
    assert_eq!(
        resolve_stdin_answer(&questions[0], "custom", 3),
        QuestionAnswer {
            selected: vec![],
            free_text: Some("custom".into()),
            auto_picked: false,
        }
    );
    assert_eq!(
        resolve_stdin_answer(&questions[1], "1, 2 extra", 3),
        QuestionAnswer {
            selected: vec!["Search".into(), "Export".into()],
            free_text: Some("extra".into()),
            auto_picked: false,
        }
    );
    assert_eq!(
        resolve_stdin_answer(&questions[1], "99", 3),
        QuestionAnswer {
            selected: vec![],
            free_text: Some("99".into()),
            auto_picked: false,
        }
    );
}

#[tokio::test]
async fn execute_reports_invalid_state_without_reading_stdin() {
    let result = QuestionTool::new()
        .execute(
            fixture(r#"{"questions":[{"question":"Pick","options":[{"description":"none"}]}]}"#),
            &ToolContext::unsandboxed("."),
        )
        .await;

    assert!(result.is_error);
    assert!(result.tool_call_id.is_empty());
    assert_eq!(
        result.content,
        "Invalid question arguments: questions[0]: options[0]: missing label"
    );
}

#[test]
fn empty_options_array_is_free_form() {
    let q = parse_questions(&json!({
        "questions": [{"question": "Notes?", "options": []}]
    }))
    .unwrap();
    assert!(q[0].options.is_empty());
}

#[test]
fn parse_rejects_too_many_questions() {
    let items: Vec<_> = (0..9)
        .map(|i| json!({"question": format!("Q{i}"), "choices": ["a"]}))
        .collect();
    let err = parse_questions(&json!({"questions": items})).unwrap_err();
    assert!(err.contains("at most 8"), "{err}");
}

#[test]
fn validate_answers_rejects_length_and_unknown_labels() {
    let q = parse_questions(&json!({
        "question": "Pick",
        "choices": ["A", "B"]
    }))
    .unwrap();
    assert!(validate_answers(&q, &[]).is_err());
    assert!(
        validate_answers(
            &q,
            &[QuestionAnswer {
                selected: vec!["Z".into()],
                free_text: None,
                auto_picked: false,
            }]
        )
        .unwrap_err()
        .contains("unknown option")
    );
    assert!(
        validate_answers(
            &q,
            &[QuestionAnswer {
                selected: vec!["A".into()],
                free_text: None,
                auto_picked: false,
            }]
        )
        .is_ok()
    );
}

#[test]
fn format_stamps_auto_picked_answers() {
    let q = parse_questions(&json!({"question": "Pick", "choices": ["A"]})).unwrap();
    let a = [QuestionAnswer {
        selected: vec!["A".into()],
        free_text: None,
        auto_picked: true,
    }];
    let body = format_question_result(&q, &a);
    assert!(body.contains("auto-picked"), "{body}");
    assert!(body.contains("approval_mode=auto"), "{body}");
}

static STDIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn queue_stdin(lines: &[&str]) {
    let mut q = TEST_STDIN.lock().unwrap_or_else(|e| e.into_inner());
    q.clear();
    for line in lines {
        q.push_back((*line).to_string());
    }
}

#[test]
fn default_and_metadata() {
    let tool = QuestionTool::default();
    assert_eq!(tool.name(), "question");
    assert!(!tool.description().is_empty());
}

#[test]
fn validate_answers_covers_remaining_rules() {
    let multi = parse_questions(&json!({
        "question": "Pick many",
        "choices": ["A", "B"],
        "multi_select": true
    }))
    .unwrap();
    assert!(
        validate_answers(
            &multi,
            &[QuestionAnswer {
                selected: vec![],
                free_text: None,
                auto_picked: false,
            }]
        )
        .unwrap_err()
        .contains("at least one option")
    );

    let single = parse_questions(&json!({
        "question": "Pick one",
        "choices": ["A", "B"]
    }))
    .unwrap();
    assert!(
        validate_answers(
            &single,
            &[QuestionAnswer {
                selected: vec!["A".into(), "B".into()],
                free_text: None,
                auto_picked: false,
            }]
        )
        .unwrap_err()
        .contains("single-select")
    );
    assert!(too_many_single_select(&["A".into(), "B".into()]));
    assert!(!too_many_single_select(&["A".into()]));
    assert!(
        validate_answers(
            &multi,
            &[QuestionAnswer {
                selected: vec!["A".into()],
                free_text: None,
                auto_picked: false,
            }]
        )
        .is_ok()
    );
    assert_eq!(
        resolve_stdin_answer(
            &parse_questions(&json!({"question": "Pick", "choices": ["A", "B"]})).unwrap()[0],
            "99",
            3
        )
        .free_text
        .as_deref(),
        Some("99")
    );
    assert!(
        validate_answers(
            &single,
            &[QuestionAnswer {
                selected: vec![],
                free_text: None,
                auto_picked: false,
            }]
        )
        .unwrap_err()
        .contains("empty selection")
    );

    let free = parse_questions(&json!({"question": "Notes?"})).unwrap();
    assert!(
        validate_answers(
            &free,
            &[QuestionAnswer {
                selected: vec![],
                free_text: Some("  ".into()),
                auto_picked: false,
            }]
        )
        .unwrap_err()
        .contains("free-form")
    );
}

#[test]
fn stdin_questionnaire_covers_free_form_and_options() {
    let _g = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let questions = parse_questions(&fixture(WAVE4_QUESTIONNAIRE)).unwrap();
    queue_stdin(&["1", "1,2", "more notes"]);
    let answers = stdin_questionnaire(&questions).unwrap();
    assert_eq!(answers[0].selected, vec!["SQLite".to_string()]);
    assert_eq!(
        answers[1].selected,
        vec!["Search".to_string(), "Export".to_string()]
    );
    assert_eq!(answers[2].free_text.as_deref(), Some("more notes"));

    queue_stdin(&[""]);
    let err = stdin_questionnaire(&questions[2..]).unwrap_err();
    assert!(err.contains("empty input"), "{err}");

    let labelled = parse_questions(&json!({
        "question": "Pick",
        "options": [{"label": "Yes", "description": "do it"}]
    }))
    .unwrap();
    queue_stdin(&[""]);
    let err = stdin_questionnaire(&labelled).unwrap_err();
    assert!(err.contains("empty input"), "{err}");

    queue_stdin(&["2", "typed other"]);
    let other = stdin_questionnaire(&labelled).unwrap();
    assert_eq!(other[0].free_text.as_deref(), Some("typed other"));

    queue_stdin(&["2", ""]);
    let empty_other = stdin_questionnaire(&labelled).unwrap();
    assert!(empty_other[0].free_text.is_none());

    queue_stdin(&["1 extra"]);
    let multi = stdin_questionnaire(&questions[1..2]).unwrap();
    assert!(
        multi[0].selected.contains(&"Search".to_string()),
        "{:?}",
        multi[0]
    );
    assert_eq!(multi[0].free_text.as_deref(), Some("extra"));
}

#[test]
fn remaining_stdin_and_parse_arms() {
    let _g = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let q = parse_questions(&json!({
        "questions": [{
            "question": "Pick",
            "options": ["", "A", {"label": "B", "description": "bee"}]
        }]
    }))
    .unwrap();
    assert_eq!(q[0].options.len(), 2);

    let multi = parse_questions(&json!({
        "question": "Many",
        "choices": ["A", "B"],
        "multi_select": true
    }))
    .unwrap();
    queue_stdin(&["3", "typed-other"]);
    let ans = stdin_questionnaire(&multi).unwrap();
    assert_eq!(ans[0].free_text.as_deref(), Some("typed-other"));

    queue_stdin(&["3", ""]);
    let empty_other = stdin_questionnaire(&multi).unwrap();
    assert!(
        empty_other[0].free_text.is_none() || empty_other[0].selected.is_empty(),
        "{:?}",
        empty_other[0]
    );

    let mut cursor = std::io::Cursor::new("hello\n");
    assert_eq!(read_line_from(&mut cursor).unwrap(), "hello");
    let mut failing = FailingReader;
    assert!(
        read_line_from(&mut failing)
            .unwrap_err()
            .contains("Failed to read input")
    );
    TEST_STDIN.lock().unwrap_or_else(|e| e.into_inner()).clear();
    let _ = read_line_stdin();
}

struct FailingReader;

impl std::io::Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("nope"))
    }
}

impl std::io::BufRead for FailingReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("nope"))
    }
    fn consume(&mut self, _amt: usize) {}
}

#[tokio::test]
async fn execute_reads_queued_stdin() {
    let _g = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    queue_stdin(&["hello from stdin"]);
    let result = QuestionTool::new()
        .execute(
            json!({"question": "Notes?"}),
            &ToolContext::unsandboxed("."),
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("hello from stdin"),
        "{}",
        result.content
    );

    queue_stdin(&[""]);
    let empty = QuestionTool::new()
        .execute(
            json!({"question": "Notes?"}),
            &ToolContext::unsandboxed("."),
        )
        .await;
    assert!(empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("empty input"), "{}", empty.content);
}
