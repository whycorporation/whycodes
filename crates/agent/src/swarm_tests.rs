use super::*;
use serde_json::json;

#[test]
fn parse_two_tasks() {
    let args = json!({
        "tasks": [
            {"goal": "audit A", "paths": ["a.rs"]},
            {"goal": "audit B", "subagent_type": "explore"}
        ]
    });
    let specs = parse_swarm_tasks(&args).unwrap();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].paths, vec!["a.rs"]);
    assert_eq!(specs[1].subagent_type, "explore");
}

#[test]
fn tldr_injected_for_long_body() {
    let body = "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 50);
    let out = ensure_tldr(&body);
    assert!(out.starts_with("TLDR:"), "{out}");
}

#[test]
fn short_body_unchanged() {
    assert_eq!(ensure_tldr("done"), "done");
}

#[test]
fn parse_rejects_empty_array_too_many_and_empty_goal() {
    assert!(parse_swarm_tasks(&json!({"tasks": []})).is_err());
    assert!(parse_swarm_tasks(&json!({})).is_err());
    let too_many = json!({
        "tasks": (0..9).map(|i| json!({"goal": format!("g{i}")})).collect::<Vec<_>>()
    });
    let err = parse_swarm_tasks(&too_many).unwrap_err();
    assert!(err.contains("at most"), "{err}");
    let empty_goal = json!({"tasks": [{"goal": "  "}]});
    let err = parse_swarm_tasks(&empty_goal).unwrap_err();
    assert!(err.contains("goal is required"), "{err}");
}

#[test]
fn ensure_tldr_empty_existing_and_truncate() {
    assert!(ensure_tldr("   ").is_empty());
    let existing = format!(
        "TLDR: already\n{}",
        "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 20)
    );
    let out = ensure_tldr(&existing);
    assert!(out.starts_with("TLDR:"), "{out}");
    let huge = format!(
        "TLDR: keep\n{}",
        "y".repeat(MAX_SWARM_COMPLETION_REPORT_CHARS + 50)
    );
    let truncated = ensure_tldr(&huge);
    assert!(
        truncated.contains("truncated")
            || truncated.chars().count() <= MAX_SWARM_COMPLETION_REPORT_CHARS + 20
    );
    let report = format_worker_report("w0", "explore", true, 1.5, "goal", "done");
    assert!(report.contains("w0") && report.contains("✓"));
    let fail = format_worker_report("w1", "general", false, 0.2, "goal", "");
    assert!(fail.contains("✕"));
    assert!(format_swarm_header(2, 1, 3.0).contains("2 agents"));
    assert_eq!(truncate_chars("ab", 10), "ab");
    let t = truncate_chars("abcdefghij", 4);
    assert!(t.starts_with("abcd"));
    assert!(t.contains("truncated"));
}
