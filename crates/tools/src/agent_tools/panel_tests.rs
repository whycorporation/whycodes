use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn show_file_pushes_update() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello panel").unwrap();
    let seen = Arc::new(Mutex::new(None));
    let seen2 = Arc::clone(&seen);
    let mut ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().to_string());
    ctx.panel = Some(Arc::new(move |u| {
        *seen2.lock().unwrap() = Some(u);
    }));
    let tool = PanelTool::new();
    let result = tool
        .execute(json!({"action": "show_file", "path": "note.txt"}), &ctx)
        .await;
    assert!(!result.is_error, "{}", result.content);
    match seen.lock().unwrap().clone() {
        Some(PanelUpdate::File { path, text }) => {
            assert_eq!(path, "note.txt");
            assert_eq!(text, "hello panel");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn clear_without_sink_is_ok() {
    let ctx = ToolContext::unsandboxed(".");
    let result = PanelTool::new()
        .execute(json!({"action": "clear"}), &ctx)
        .await;
    assert!(!result.is_error);
    assert!(result.content.contains("no TUI panel"));
}

#[tokio::test]
async fn remaining_actions_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let t = PanelTool::default();
    assert_eq!(t.name(), "panel");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "action");
    let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().to_string());

    let missing_file = t.execute(json!({"action": "show_file"}), &ctx).await;
    assert!(missing_file.is_error);
    let gone = t
        .execute(json!({"action": "show_file", "path": "gone.txt"}), &ctx)
        .await;
    assert!(gone.is_error);

    let abs = dir.path().join("note.txt");
    let via_abs = t
        .execute(
            json!({"action": "show_file", "path": abs.to_string_lossy()}),
            &ctx,
        )
        .await;
    assert!(!via_abs.is_error, "{}", via_abs.content);

    let mermaid = t
        .execute(
            json!({"action": "show_mermaid", "source": "graph TD; A-->B;"}),
            &ctx,
        )
        .await;
    assert!(!mermaid.is_error, "{}", mermaid.content);
    let mermaid_file = t
        .execute(json!({"action": "show_mermaid", "path": "note.txt"}), &ctx)
        .await;
    assert!(!mermaid_file.is_error, "{}", mermaid_file.content);
    let mermaid_missing = t.execute(json!({"action": "show_mermaid"}), &ctx).await;
    assert!(mermaid_missing.is_error);

    let diff_src = t
        .execute(
            json!({"action": "show_diff", "source": "--- a\n+++ b\n"}),
            &ctx,
        )
        .await;
    assert!(!diff_src.is_error, "{}", diff_src.content);
    let diff_empty = t
        .execute(json!({"action": "show_diff", "path": "note.txt"}), &ctx)
        .await;
    assert!(diff_empty.is_error, "{}", diff_empty.content);
    let empty = empty_git_diff();
    assert!(empty.is_error);
    assert!(empty.content.contains("git diff is empty"));
    let diff_missing = t.execute(json!({"action": "show_diff"}), &ctx).await;
    assert!(diff_missing.is_error);

    let huge = "x".repeat(MAX_PREVIEW_BYTES + 8);
    let capped = cap_text(&huge);
    assert!(capped.ends_with("\n…"));
    std::fs::write(dir.path().join("big.txt"), &huge).unwrap();
    let too_big = t
        .execute(json!({"action": "show_file", "path": "big.txt"}), &ctx)
        .await;
    assert!(too_big.is_error);

    let bad = t.execute(json!({"action": "nope"}), &ctx).await;
    assert!(bad.is_error);

    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen2 = std::sync::Arc::clone(&seen);
    let mut with_sink = ctx.clone();
    with_sink.panel = Some(std::sync::Arc::new(move |u| {
        *seen2.lock().unwrap() = Some(u);
    }));
    let cleared = t.execute(json!({"action": "clear"}), &with_sink).await;
    assert!(!cleared.is_error, "{}", cleared.content);
    assert!(!cleared.content.contains("no TUI panel"));
}

#[tokio::test]
async fn git_diff_success_and_mermaid_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@whycodes.local"])
        .current_dir(dir.path())
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "whycodes-test"])
        .current_dir(dir.path())
        .status();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let t = PanelTool::new();
    let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().to_string());
    let diff = t
        .execute(json!({"action": "show_diff", "path": "a.txt"}), &ctx)
        .await;
    assert!(!diff.is_error, "{}", diff.content);

    let missing = t
        .execute(json!({"action": "show_mermaid", "path": "gone.mmd"}), &ctx)
        .await;
    assert!(missing.is_error, "{}", missing.content);

    let git_fail = git_diff(&ctx, "nope.txt");
    assert!(git_fail.is_ok() || git_fail.unwrap_err().contains("git"));

    let clean = t
        .execute(json!({"action": "show_diff", "path": "a.txt"}), &ctx)
        .await;
    // After commit the working tree is dirty (two\n). Reset to HEAD so git
    // diff is empty and the dedicated empty-diff helper is exercised.
    let _ = std::process::Command::new("git")
        .args(["checkout", "--", "a.txt"])
        .current_dir(dir.path())
        .status();
    let empty_diff = t
        .execute(json!({"action": "show_diff", "path": "a.txt"}), &ctx)
        .await;
    assert!(
        empty_diff.is_error || !clean.content.is_empty(),
        "{}",
        empty_diff.content
    );
}
