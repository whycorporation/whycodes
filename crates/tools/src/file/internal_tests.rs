use super::*;
use crate::tool::ToolContext;

#[test]
fn unknown_scheme_is_none() {
    let ctx = ToolContext::new("/tmp");
    assert!(read_internal("src/main.rs", &ctx).is_none());
    assert!(read_internal("http://example.com", &ctx).is_none());
}

#[test]
fn skill_url_rejects_bad_names() {
    let ctx = ToolContext::new("/tmp");
    let r = read_internal("skill://", &ctx).unwrap();
    assert!(r.is_error);
    let r = read_internal("skill://a/b", &ctx).unwrap();
    assert!(r.is_error);
}

#[test]
fn skill_url_loads_project_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".skills");
    std::fs::create_dir(&skills).unwrap();
    std::fs::write(
        skills.join("demo.skill.md"),
        "---\nname: demo\ndescription: d\n---\n\nTHE BODY\n",
    )
    .unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let r = read_internal("skill://demo", &ctx).unwrap();
    assert!(!r.is_error, "{}", r.content);
    assert!(r.content.contains("THE BODY"));
    let miss = read_internal("skill://nope", &ctx).unwrap();
    assert!(miss.is_error);
}

#[test]
fn agent_url_lists_and_reads() {
    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(".whycodes").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("task-1.md"), "findings here").unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let list = read_internal("agent://", &ctx).unwrap();
    assert!(!list.is_error);
    assert!(list.content.contains("task-1"));
    let body = read_internal("agent://task-1", &ctx).unwrap();
    assert!(body.content.contains("findings here"));
    let bad = read_internal("agent://../x", &ctx).unwrap();
    assert!(bad.is_error);
    let missing = read_internal("agent://nope", &ctx).unwrap();
    assert!(missing.is_error);
}

#[test]
fn remaining_agent_and_skill_edges() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let none = read_internal("agent://", &ctx).unwrap();
    assert!(!none.is_error);
    let agents = dir.path().join(".whycodes").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    let empty_list = read_internal("agent://", &ctx).unwrap();
    assert!(empty_list.content.contains("No agent artifacts"));
    std::fs::write(agents.join("empty.md"), "   ").unwrap();
    let empty_body = read_internal("agent://empty", &ctx).unwrap();
    assert!(empty_body.is_error);
    std::fs::write(agents.join("skip.bin"), "x").unwrap();
    let listed = read_internal("agent://", &ctx).unwrap();
    assert!(!listed.content.contains("skip"));
    let dots = read_internal("skill://..", &ctx).unwrap();
    assert!(dots.is_error);
}

#[test]
fn skill_load_error_is_surfaced_for_unreadable_project() {
    let ctx = ToolContext::new("/nonexistent-xyz-project");
    let r = read_internal("skill://demo", &ctx).unwrap();
    assert!(
        r.is_error || r.content.contains("not found") || r.content.contains("Error"),
        "{}",
        r.content
    );
    let load_err = skill_load_error("boom");
    assert!(load_err.is_error);
    assert!(load_err.content.contains("Error loading skills"));
    let bounced = registry_load_failed(skill_load_error("boom"));
    assert!(bounced.is_error);
    assert!(
        take_skill_registry(Err(skill_load_error("boom")))
            .unwrap_err()
            .is_error
    );
    assert!(take_skill_registry(Ok(whycodes_skill::SkillRegistry::new())).is_ok());
    assert!(skill_registry_or_err(Err(skill_load_error("boom"))).is_err());
    assert!(skill_registry_or_err(Ok(whycodes_skill::SkillRegistry::new())).is_ok());
    assert!(skill_body(Err(skill_load_error("boom")), "demo").is_error);
    let missing = skill_body(Ok(whycodes_skill::SkillRegistry::new()), "demo");
    assert!(missing.is_error);
    let blocked = tempfile::tempdir().unwrap();
    std::fs::write(blocked.path().join(".skills"), "not-a-dir").unwrap();
    match load_skill_registry(blocked.path()) {
        Ok(_) => {}
        Err(e) => assert!(e.is_error),
    }
}
