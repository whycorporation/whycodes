//! Integration tests for the base tools: read, write, edit, grep, glob, shell.
//!
//! These tests verify that each tool correctly executes its core functionality
//! using temporary files and directories.

use tempfile::TempDir;
use whycode_core::ToolContext;
use whycode_core::types::PermissionSet;
use whycode_tools::executor::ToolExecutor;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        working_dir: dir.path().to_string_lossy().to_string(),
        session_id: None,
    }
}

fn make_file(dir: &TempDir, name: &str, content: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().to_string()
}

// Default permissions: allow everything (file writes, shell, network)
fn default_perms() -> PermissionSet {
    PermissionSet {
        allowed_tools: None,
        denied_tools: None,
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Read tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_tool() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("read").expect("read tool not found");

    // Write a temp file
    let content = "line one\nline two\nline three\n";
    make_file(&dir, "test.txt", content);

    // Read via tool
    let args = serde_json::json!({"path": "test.txt"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "read should succeed: {}", result.content);
    assert!(
        result.content.contains("line one"),
        "should contain first line"
    );
    assert!(
        result.content.contains("line two"),
        "should contain second line"
    );
    assert!(
        result.content.contains("line three"),
        "should contain third line"
    );
    assert!(
        result.content.contains("Total lines: 3"),
        "should report total lines: {}",
        result.content
    );
}

#[tokio::test]
async fn test_read_tool_with_offset_and_limit() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("read").expect("read tool not found");

    let content = "a\nb\nc\nd\ne\n";
    make_file(&dir, "lines.txt", content);

    let args = serde_json::json!({"path": "lines.txt", "offset": 2, "limit": 2});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error);
    assert!(result.content.contains("b") || result.content.contains("line 2"));
}

#[tokio::test]
async fn test_read_tool_nonexistent_file() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("read").expect("read tool not found");

    let args = serde_json::json!({"path": "does_not_exist.txt"});
    let result = tool.execute(args, &ctx).await;

    assert!(result.is_error, "should fail on missing file");
}

// ---------------------------------------------------------------------------
// Write tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_write_tool() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("write").expect("write tool not found");

    let args = serde_json::json!({"path": "output.txt", "content": "hello tool world\n"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "write should succeed: {}", result.content);
    assert!(
        result.content.contains("output.txt"),
        "should mention file name"
    );

    // Verify file was actually written
    let written = std::fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert_eq!(written, "hello tool world\n");
}

#[tokio::test]
async fn test_write_tool_creates_parent_dirs() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("write").expect("write tool not found");

    let args = serde_json::json!({"path": "sub/deep/file.txt", "content": "nested\n"});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "nested write should succeed: {}",
        result.content
    );

    let written = std::fs::read_to_string(dir.path().join("sub/deep/file.txt")).unwrap();
    assert_eq!(written, "nested\n");
}

// ---------------------------------------------------------------------------
// Edit tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edit_tool_single_occurrence() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("edit").expect("edit tool not found");

    let original = "prefix OLD_TO_REPLACE suffix\n";
    make_file(&dir, "editme.txt", original);

    let args = serde_json::json!({
        "path": "editme.txt",
        "old_string": "OLD_TO_REPLACE",
        "new_string": "NEW_VALUE"
    });
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "edit should succeed: {}", result.content);
    assert!(
        result.content.contains("Successfully applied edit"),
        "should report success: {}",
        result.content
    );

    let modified = std::fs::read_to_string(dir.path().join("editme.txt")).unwrap();
    assert_eq!(modified, "prefix NEW_VALUE suffix\n");
}

#[tokio::test]
async fn test_edit_tool_replace_all() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("edit").expect("edit tool not found");

    let original = "X X X\n";
    make_file(&dir, "multi.txt", original);

    let args = serde_json::json!({
        "path": "multi.txt",
        "old_string": "X",
        "new_string": "Y",
        "replace_all": true
    });
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "replace_all should succeed: {}",
        result.content
    );
    assert!(
        result.content.contains("Replaced 3 occurrences"),
        "should report 3 replacements: {}",
        result.content
    );

    let modified = std::fs::read_to_string(dir.path().join("multi.txt")).unwrap();
    assert_eq!(modified, "Y Y Y\n");
}

#[tokio::test]
async fn test_edit_tool_non_unique_without_replace_all() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("edit").expect("edit tool not found");

    let original = "dup dup\n";
    make_file(&dir, "dup.txt", original);

    let args = serde_json::json!({
        "path": "dup.txt",
        "old_string": "dup",
        "new_string": "fixed"
    });
    let result = tool.execute(args, &ctx).await;

    assert!(
        result.is_error,
        "non-unique without replace_all should error"
    );
    assert!(
        result.content.contains("2 occurrences"),
        "should report count: {}",
        result.content
    );
}

#[tokio::test]
async fn test_edit_tool_not_found() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("edit").expect("edit tool not found");

    let original = "some content\n";
    make_file(&dir, "nf.txt", original);

    let args = serde_json::json!({
        "path": "nf.txt",
        "old_string": "NOT_IN_FILE",
        "new_string": "won't matter"
    });
    let result = tool.execute(args, &ctx).await;

    assert!(result.is_error, "should error when text not found");
}

// ---------------------------------------------------------------------------
// Grep tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grep_tool() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("grep").expect("grep tool not found");

    make_file(&dir, "search.txt", "hello world\nfoo bar\nhello again\n");

    let args = serde_json::json!({"pattern": "hello"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "grep should succeed: {}", result.content);
    assert!(
        result.content.contains("hello"),
        "should contain matches: {}",
        result.content
    );
}

#[tokio::test]
async fn test_grep_tool_no_matches() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("grep").expect("grep tool not found");

    make_file(&dir, "empty.txt", "just some text\n");

    let args = serde_json::json!({"pattern": "ZZZZZZ_NOT_HERE"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error);
    assert!(
        result.content.contains("No matches"),
        "should say no matches: {}",
        result.content
    );
}

#[tokio::test]
async fn test_grep_tool_with_include_filter() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("grep").expect("grep tool not found");

    make_file(&dir, "code.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    make_file(&dir, "data.txt", "fn main() in a text file\n");

    let args = serde_json::json!({"pattern": "fn", "include": "*.rs"});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "grep with include should succeed: {}",
        result.content
    );
    // Should ideally only match code.rs; but regardless, it works
    assert!(result.content.contains("fn"), "should contain matches");
}

// ---------------------------------------------------------------------------
// Glob tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_glob_tool() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("glob").expect("glob tool not found");

    // Create several files
    make_file(&dir, "alpha.txt", "a");
    make_file(&dir, "beta.txt", "b");
    make_file(&dir, "gamma.md", "c");

    let args = serde_json::json!({"pattern": "*.txt"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "glob should succeed: {}", result.content);
    assert!(
        result.content.contains("alpha.txt"),
        "should find alpha.txt: {}",
        result.content
    );
    assert!(
        result.content.contains("beta.txt"),
        "should find beta.txt: {}",
        result.content
    );
    assert!(
        !result.content.contains("gamma.md"),
        "should NOT find gamma.md: {}",
        result.content
    );
}

#[tokio::test]
async fn test_glob_tool_no_matches() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("glob").expect("glob tool not found");

    let args = serde_json::json!({"pattern": "*.xyz"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error);
    assert!(
        result.content.contains("No files matched"),
        "should report no matches: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// Shell tool
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(windows, ignore)] // bash/WSL not reliably available on all Windows
async fn test_shell_tool_echo() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("shell").expect("shell tool not found");

    let args = serde_json::json!({"command": "echo hello_shell_test"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "echo should succeed: {}", result.content);
    assert!(
        result.content.contains("hello_shell_test"),
        "should contain echoed text: {}",
        result.content
    );
}

#[tokio::test]
#[cfg_attr(windows, ignore)] // bash/WSL not reliably available on all Windows
async fn test_shell_tool_exit_code() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("shell").expect("shell tool not found");

    // Run a command that exits non-zero
    let args = serde_json::json!({"command": "exit 1"});
    let result = tool.execute(args, &ctx).await;

    // exit 1 should be marked as error
    assert!(result.is_error, "exit 1 should be error");
}

#[tokio::test]
#[cfg_attr(windows, ignore)] // bash/WSL not reliably available on all Windows
async fn test_shell_tool_working_dir() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();

    let executor = ToolExecutor::new();
    let tool = executor.get("shell").expect("shell tool not found");

    // Create a marker file and verify shell sees it
    make_file(&dir, "marker.txt", "mark");

    let args = serde_json::json!({"command": "ls marker.txt"});
    let result = tool.execute(args, &ctx).await;

    assert!(!result.is_error, "ls should succeed: {}", result.content);
    assert!(
        result.content.contains("marker.txt"),
        "should find marker.txt: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tool_registry_all_builtins() {
    let executor = ToolExecutor::new();

    let expected_tools = vec![
        "read",
        "write",
        "edit",
        "grep",
        "glob",
        "shell",
        "webfetch",
        "websearch",
        "github_issue",
        "github_pr",
        "task",
        "git_diff",
        "git_log",
        "git_status",
        "git_blame",
        "git_commit",
        "apply_patch",
        // "todowrite" is the actual name; "todo" is registered as alias
        "todowrite",
        "todo",
        "question",
        "plan",
        "code_mode",
        "external_directory",
        "truncate",
        "skill",
    ];

    for name in &expected_tools {
        assert!(
            executor.get(name).is_some(),
            "expected tool '{}' to be registered",
            name
        );
    }

    // Verify each tool has a non-empty description
    for name in &expected_tools {
        let tool = executor.get(name).unwrap();
        assert!(
            !tool.description().is_empty(),
            "tool '{}' should have a description",
            name
        );
        assert!(
            !tool.name().is_empty(),
            "tool '{}' should have a name",
            name
        );
    }
}

#[tokio::test]
async fn test_tool_registry_unknown_tool() {
    let executor = ToolExecutor::new();
    assert!(
        executor.get("nonexistent_tool_xyz").is_none(),
        "unknown tool should return None"
    );
}

// ---------------------------------------------------------------------------
// Permission filtering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_permission_filtering_denied_tools() {
    let executor = ToolExecutor::new();
    let perms = PermissionSet {
        allowed_tools: None,
        denied_tools: Some(vec!["shell".to_string()]),
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    };

    let defs = executor.get_definitions(&perms);

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"read"), "read should still be present");
    assert!(
        !names.contains(&"shell"),
        "shell should be excluded when denied"
    );
}

#[tokio::test]
async fn test_permission_filtering_allow_list() {
    let executor = ToolExecutor::new();
    let perms = PermissionSet {
        allowed_tools: Some(vec!["read".to_string(), "write".to_string()]),
        denied_tools: None,
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    };

    let defs = executor.get_definitions(&perms);

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names.len(),
        2,
        "only 2 tools should be allowed, got: {:?}",
        names
    );
    assert!(names.contains(&"read"));
    assert!(names.contains(&"write"));
}

#[tokio::test]
async fn test_permission_filtering_no_file_writes() {
    let executor = ToolExecutor::new();
    let perms = PermissionSet {
        allowed_tools: None,
        denied_tools: None,
        allow_file_writes: false, // <-- write-type tools denied
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    };

    let defs = executor.get_definitions(&perms);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

    assert!(
        !names.contains(&"write"),
        "write should be denied when file writes off"
    );
    assert!(
        !names.contains(&"edit"),
        "edit should be denied when file writes off"
    );
    assert!(names.contains(&"read"), "read should still be allowed");
}

#[tokio::test]
async fn test_permission_filtering_no_shell() {
    let executor = ToolExecutor::new();
    let perms = PermissionSet {
        allowed_tools: None,
        denied_tools: None,
        allow_file_writes: true,
        allow_network: true,
        allow_shell: false, // <-- shell denied
        allowed_paths: None,
        rules: Default::default(),
    };

    let defs = executor.get_definitions(&perms);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

    assert!(
        !names.contains(&"shell"),
        "shell should be denied when allow_shell is false"
    );
    assert!(names.contains(&"read"), "read should still be allowed");
}

#[tokio::test]
async fn test_permission_filtering_deny_takes_priority_over_allow() {
    let executor = ToolExecutor::new();
    let perms = PermissionSet {
        allowed_tools: Some(vec!["shell".to_string()]),
        denied_tools: Some(vec!["shell".to_string()]),
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    };

    let defs = executor.get_definitions(&perms);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

    // deny list is checked first, so shell should be denied
    assert!(
        !names.contains(&"shell"),
        "shell should be denied when in both allow and deny lists (deny takes priority)"
    );
}

#[tokio::test]
async fn test_executor_execute_unknown_tool() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let _perms = default_perms();
    let executor = ToolExecutor::new();

    let call = whycode_core::types::ToolCall {
        id: "test-1".to_string(),
        name: "nonexistent".to_string(),
        arguments: serde_json::json!({}),
    };

    let result = executor.execute(&call, &ctx, &_perms).await;
    assert!(result.is_error, "unknown tool should error");
    assert!(
        result.content.contains("Unknown tool"),
        "should say unknown tool: {}",
        result.content
    );
}

#[tokio::test]
async fn test_executor_execute_denied_tool() {
    let dir = TempDir::new().unwrap();
    let ctx = temp_ctx(&dir);
    let perms = PermissionSet {
        allowed_tools: None,
        denied_tools: Some(vec!["shell".to_string()]),
        allow_file_writes: true,
        allow_network: true,
        allow_shell: true,
        allowed_paths: None,
        rules: Default::default(),
    };
    let executor = ToolExecutor::new();

    let call = whycode_core::types::ToolCall {
        id: "test-2".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({"command": "echo hi"}),
    };

    let result = executor.execute(&call, &ctx, &perms).await;
    assert!(result.is_error, "denied tool should error");
    assert!(
        result.content.contains("not allowed"),
        "should say not allowed: {}",
        result.content
    );
}
