use super::*;
use crate::app::{AgentState, TuiApp};
use crate::config::TuiAppConfig;

#[test]
fn strip_status_chrome_drops_cancel_and_spinner() {
    assert_eq!(
        strip_status_chrome("⠋ Generating…  [Esc cancel]"),
        "Generating…"
    );
    assert_eq!(
        strip_status_chrome("LLM request (step 1)…  [Esc cancel]"),
        "LLM request (step 1)…"
    );
    assert_eq!(strip_status_chrome("tool: bash  Esc cancel"), "tool: bash");
}

#[test]
fn turn_status_detail_skips_generic_generating() {
    assert_eq!(turn_status_detail("Generating…", "generating 1.2s"), "");
    assert_eq!(
        turn_status_detail("LLM request (step 1)…  [Esc cancel]", "generating 1.2s"),
        "LLM request (step 1)…"
    );
    assert_eq!(
        turn_status_detail("Running tool `read`…", "generating 3s"),
        "tool: read"
    );
}

#[test]
fn turn_status_detail_handles_generating_variants_and_empty() {
    // All "generating…" spellings collapse to nothing.
    assert_eq!(turn_status_detail("Generating", "generating 1.2s"), "");
    assert_eq!(turn_status_detail("Generating...", "generating"), "");
    assert_eq!(turn_status_detail("Generating… 3 steps", "generating"), "");
    // Empty / chrome-only status → no detail.
    assert_eq!(turn_status_detail("", "generating"), "");
    assert_eq!(turn_status_detail("  [Esc cancel]  ", "generating"), "");
}

#[test]
fn turn_status_detail_dedupes_label_head() {
    // "thinking · thinking…" style double is dropped.
    assert_eq!(turn_status_detail("thinking…", "thinking 1.4s"), "");
    assert_eq!(turn_status_detail("thinking", "thinking 1.4s"), "");
    // A longer detail that merely starts with the label survives.
    assert_eq!(
        turn_status_detail("thinking about next step", "thinking 1.4s"),
        "thinking about next step"
    );
}

#[test]
fn turn_status_detail_cleans_tool_names() {
    // Tool name keeps only safe identifier chars.
    assert_eq!(
        turn_status_detail("Running tool `git status`…", "generating 3s"),
        "tool: gitstatus"
    );
    assert_eq!(
        turn_status_detail("Running tool `read`…", "generating 3s"),
        "tool: read"
    );
    // Backtick content with nothing usable → original text kept.
    assert_eq!(
        turn_status_detail("Running tool ``…", "generating 3s"),
        "Running tool ``…"
    );
}

#[test]
fn turn_status_detail_truncates_mid() {
    let long = "a".repeat(100);
    let detail = turn_status_detail(&long, "generating");
    assert!(detail.ends_with('…'), "{detail}");
    assert_eq!(detail.chars().count(), 40, "{detail}");
}

#[test]
fn strip_status_chrome_extra_cases() {
    // Multiple leading spinners + noise + separators.
    assert_eq!(
        strip_status_chrome("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏⠋⠋  hello · world  ·"),
        "hello · world"
    );
    assert_eq!(strip_status_chrome("  plain text  "), "plain text");
    assert_eq!(strip_status_chrome("Esc cancel · ready"), "ready");
    assert_eq!(strip_status_chrome(""), "");
}

#[test]
fn truncate_mid_keeps_short_and_ellipsizes_long() {
    assert_eq!(truncate_mid("short", 40), "short");
    assert_eq!(truncate_mid("", 5), "");
    assert_eq!(truncate_mid("abcdef", 3), "ab…");
    assert_eq!(truncate_mid("abcdef", 6), "abcdef");
    // Multibyte chars counted, not bytes (keep = max - 1).
    assert_eq!(truncate_mid("çok uzun bir başlık", 5), "çok …");
}

#[test]
fn turn_status_height_tracks_busy_and_clears_stop_hit() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.turn_stop_hit.set_rect(Some(Rect::new(10, 10, 6, 1)));
    assert_eq!(turn_status_height(&mut app), 0);
    assert!(app.turn_stop_hit.rect.is_none(), "idle clears the stop hit");

    app.current_agent_state = AgentState::Generating;
    assert_eq!(turn_status_height(&mut app), 1);
    app.current_agent_state = AgentState::Thinking;
    assert_eq!(turn_status_height(&mut app), 1);
}
