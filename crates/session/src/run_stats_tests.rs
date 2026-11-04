use super::*;
use whycodes_core::types::{ContentBlock, ImageSource, MessageContent};

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({}),
    }
}

fn result(id: &str, is_error: bool) -> ToolResult {
    ToolResult {
        tool_call_id: id.into(),
        content: "out".into(),
        is_error,
    }
}

fn todo(status: TodoStatus) -> TodoItem {
    TodoItem {
        id: String::new(),
        content: "x".into(),
        status,
    }
}

#[test]
fn records_requests_per_model_and_tools_with_failures() {
    let mut run = RunStats::default();
    let usage = Usage {
        input_tokens: 1_500,
        output_tokens: 20,
        ..Default::default()
    };
    run.record_request("cf", "glm", &usage);
    run.record_request("cf", "glm", &Usage::default());
    run.record_request("cf", "kimi", &usage);
    run.record_tools(
        &[call("a", "bash"), call("b", "bash"), call("c", "read")],
        &[result("a", true), result("b", false), result("c", false)],
    );

    assert_eq!(run.requests(), 3);
    assert_eq!(
        run.models["cf/glm"],
        ModelTally {
            requests: 2,
            input_tokens: 1_500,
            output_tokens: 20
        }
    );
    assert_eq!(run.tool_calls(), 3);
    assert_eq!(run.tool_errors(), 1);
    assert_eq!(
        run.tools["bash"],
        ToolTally {
            calls: 2,
            errors: 1
        }
    );
}

#[test]
fn prompt_count_skips_notes_summaries_and_tool_carriers() {
    let mut s = Session::new("/p".into(), "sys".into());
    s.add_user_message("fix it");
    s.add_user_message("   ");
    s.add_user_message("[Compacted] 1. Primary Request: x");
    s.add_user_message("note");
    if let Some(m) = s.messages.last_mut() {
        m.name = Some(whycodes_core::types::AGENT_NOTE_NAME.into());
    }
    s.add_user_message("blocks");
    if let Some(m) = s.messages.last_mut() {
        m.content = MessageContent::Blocks(vec![ContentBlock::Image {
            source: ImageSource::Url {
                url: "a.png".into(),
            },
        }]);
    }
    s.add_user_message("carrier");
    if let Some(m) = s.messages.last_mut() {
        m.content = MessageContent::Blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "t".into(),
            content: "r".into(),
            is_error: None,
        }]);
    }
    s.add_assistant_message(vec![ContentBlock::Text { text: "ok".into() }]);

    assert_eq!(s.prompt_count(), 2);
}

#[test]
fn report_lists_models_tools_subagents_and_todos() {
    let mut s = Session::new("/p".into(), "sys".into());
    s.add_user_message("go");
    s.usage.input_tokens = 2_500_000;
    s.usage.output_tokens = 999;
    s.run_stats.record_request("cf", "glm", &s.usage.clone());
    s.run_stats.record_tools(
        &[call("a", "task"), call("b", "bash"), call("c", "bash")],
        &[result("a", false), result("b", true), result("c", false)],
    );
    let todos = [
        todo(TodoStatus::Completed),
        todo(TodoStatus::InProgress),
        todo(TodoStatus::Pending),
        todo(TodoStatus::Cancelled),
    ];
    let now = s.created_at + chrono::Duration::minutes(75);

    let out = stats_report(&s, &todos, now);

    assert!(out.contains("duration:  1h 15m"), "{out}");
    assert!(out.contains("prompts:   1"), "{out}");
    assert!(out.contains("requests:  1 this run"), "{out}");
    assert!(
        out.contains("cf/glm: 1 request · 2.5M in / 999 out"),
        "{out}"
    );
    assert!(out.contains("tools:     3 calls · 1 failed"), "{out}");
    assert!(out.contains("bash 2 (1 failed) · task 1"), "{out}");
    assert!(out.contains("subagents: 1 launched"), "{out}");
    assert!(
        out.contains("todos:     1/4 done · 1 in progress · 1 pending · 1 cancelled"),
        "{out}"
    );
    assert!(out.contains("tokens:    2.5M in / 999 out"), "{out}");
}

#[test]
fn report_on_a_fresh_session_shows_zeroes() {
    let s = Session::new("/p".into(), "sys".into());
    let out = stats_report(&s, &[], s.created_at + chrono::Duration::seconds(30));

    assert!(out.contains("duration:  0m"), "{out}");
    assert!(out.contains("requests:  0 this run"), "{out}");
    assert!(out.contains("tools:     0 calls · 0 failed"), "{out}");
    assert!(out.contains("subagents: 0 launched"), "{out}");
    assert!(out.contains("todos:     0/0 done"), "{out}");
}

#[test]
fn compact_scales_thousands() {
    assert_eq!(compact(12_300), "12.3k");
    assert_eq!(compact(7), "7");
    assert_eq!(compact(3_000_000), "3.0M");
}
