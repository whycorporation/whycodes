use super::*;

#[test]
fn core_excludes_web_and_github() {
    assert!(ToolProfile::Core.includes("read"));
    assert!(ToolProfile::Core.includes("bash"));
    assert!(!ToolProfile::Core.includes("webfetch"));
    assert!(!ToolProfile::Core.includes("browser"));
    assert!(!ToolProfile::Core.includes("github_pr"));
    assert!(!ToolProfile::Core.includes("lsp"));
    assert!(ToolProfile::Full.includes("webfetch"));
}

#[test]
fn core_includes_todo_tools_under_real_names() {
    assert!(ToolProfile::Core.includes("todowrite"));
    assert!(ToolProfile::Core.includes("todoread"));
    assert!(ToolProfile::Core.includes("todo"));
    // Wrong snake_case names must not be the filter keys
    assert!(!ToolProfile::Core.includes("todo_write"));
    assert!(!ToolProfile::Core.includes("todo_read"));
}

#[test]
fn core_includes_question() {
    assert!(ToolProfile::Core.includes("question"));
}

#[test]
fn core_includes_repomap() {
    assert!(ToolProfile::Core.includes("repomap"));
}

#[test]
fn core_includes_tool_search_for_deferred_activation() {
    assert!(ToolProfile::Core.includes("tool_search"));
    assert!(!ToolProfile::Core.includes("worktree"));
    assert!(!ToolProfile::Core.includes("webfetch"));
}

#[test]
fn core_defers_specialized_tools() {
    assert!(ToolProfile::Core.includes("bg"));
    for name in [
        "apply_patch",
        "memory",
        "schedule",
        "shell",
        "swarm",
        "swarm_msg",
    ] {
        assert!(
            !ToolProfile::Core.includes(name),
            "{name} should be deferred"
        );
        assert!(ToolProfile::Full.includes(name), "{name} stays in Full");
    }
    assert!(
        CORE_TOOL_NAMES.len() <= 15,
        "core set grew: {}",
        CORE_TOOL_NAMES.len()
    );
}

#[test]
fn parse_and_as_str_cover_aliases() {
    assert_eq!(ToolProfile::parse("full"), ToolProfile::Full);
    assert_eq!(ToolProfile::parse("ALL"), ToolProfile::Full);
    assert_eq!(ToolProfile::parse("core"), ToolProfile::Core);
    assert_eq!(ToolProfile::parse(" nope "), ToolProfile::Core);
    assert_eq!(ToolProfile::Core.as_str(), "core");
    assert_eq!(ToolProfile::Full.as_str(), "full");
    assert_eq!(ToolProfile::core_names(), CORE_TOOL_NAMES);
    assert_eq!(ToolProfile::default(), ToolProfile::Core);
}
