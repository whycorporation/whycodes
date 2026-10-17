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
    let guide = read_internal("agent://tools/bash", &ctx).unwrap();
    assert!(!guide.is_error, "{}", guide.content);
    assert!(guide.content.contains("auto-detach") || guide.content.contains("background"));
    let index = read_internal("agent://tools", &ctx).unwrap();
    assert!(index.content.contains("bash"));
    let unknown = read_internal("agent://tools/nope", &ctx).unwrap();
    assert!(unknown.is_error);
    for name in ["read", "edit", "write", "grep", "glob", "shell"] {
        let g = read_internal(&format!("agent://tools/{name}"), &ctx).unwrap();
        assert!(!g.is_error, "{name}: {}", g.content);
        assert!(
            g.content.contains(&format!("agent://tools/{name}"))
                || g.content.to_lowercase().contains(name),
            "{name}: {}",
            g.content
        );
    }
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
    assert!(skill_registry_error("boom").is_error);
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

#[test]
fn proc_url_lists_and_reads_job_logs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let none = read_internal("proc://", &ctx).unwrap();
    assert!(!none.is_error);
    assert!(
        none.content.contains("No background job logs"),
        "{}",
        none.content
    );

    let scratch = dir.path().join(".whycodes").join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let empty = read_internal("proc://", &ctx).unwrap();
    assert_eq!(empty.content, "No background job logs yet.");

    std::fs::write(scratch.join("bg-1.log"), "line one\nline two\n").unwrap();
    std::fs::write(scratch.join("notes.txt"), "skip").unwrap();
    std::fs::write(scratch.join("bg-2.log"), vec![b'x'; 2048]).unwrap();
    let list = read_internal("proc://", &ctx).unwrap();
    assert!(!list.is_error);
    assert!(
        list.content.contains("- bg-1  18 B  modified"),
        "{}",
        list.content
    );
    assert!(
        list.content.contains("- bg-2  2.0 KB  modified"),
        "{}",
        list.content
    );
    assert!(!list.content.contains("notes"), "{}", list.content);

    let body = read_internal("proc://bg-1", &ctx).unwrap();
    assert!(!body.is_error);
    assert!(
        body.content.starts_with("# proc://bg-1\n\nline one"),
        "{}",
        body.content
    );
    let slash = read_internal("proc:///bg-1", &ctx).unwrap();
    assert_eq!(slash.content, body.content);

    let bad = read_internal("proc://../x", &ctx).unwrap();
    assert!(bad.is_error);
    let missing = read_internal("proc://bg-9", &ctx).unwrap();
    assert!(missing.is_error);
    assert!(missing.content.contains("`bg`"), "{}", missing.content);
    assert!(missing.content.contains("bg-9"), "{}", missing.content);
}

#[test]
fn proc_helpers_format_tail_size_and_age() {
    assert_eq!(tail_chars("abc", 5), "abc");
    let long = "é".repeat(20);
    let tail = tail_chars(&long, 5);
    assert!(tail.starts_with("…(15 earlier chars omitted)\n"), "{tail}");
    assert!(tail.ends_with(&"é".repeat(5)), "{tail}");

    assert_eq!(size_label(512), "512 B");
    assert_eq!(size_label(1536), "1.5 KB");
    assert_eq!(size_label(3 * 1024 * 1024), "3.0 MB");

    let now = std::time::SystemTime::now();
    let ago = |secs: u64| Some(now - std::time::Duration::from_secs(secs));
    assert_eq!(age_label(None), "at an unknown time");
    assert!(
        age_label(ago(5)).ends_with("s ago"),
        "{}",
        age_label(ago(5))
    );
    assert_eq!(age_label(ago(120)), "2m ago");
    assert_eq!(age_label(ago(7200)), "2h ago");
    assert_eq!(age_label(ago(3 * 86_400)), "3d ago");
    let future = Some(now + std::time::Duration::from_secs(600));
    assert_eq!(age_label(future), "0s ago");
}

#[test]
fn rule_url_lists_and_reads_project_rules() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let none = read_internal("rule://", &ctx).unwrap();
    assert!(!none.is_error);
    assert!(
        none.content.contains("No project rules"),
        "{}",
        none.content
    );
    assert!(
        none.content.contains(".whycodes/rules/"),
        "{}",
        none.content
    );

    let rules = dir.path().join(".whycodes").join("rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(rules.join("Style.md"), "no unwrap\n").unwrap();
    std::fs::write(rules.join("ignored.txt"), "x").unwrap();
    std::fs::create_dir_all(rules.join("nested.md")).unwrap();
    let cursor = dir.path().join(".cursor").join("rules");
    std::fs::create_dir_all(&cursor).unwrap();
    std::fs::write(cursor.join("rust.MDC"), "cargo fmt").unwrap();
    std::fs::write(cursor.join("style.mdc"), "cursor style").unwrap();

    let list = read_internal("rule://", &ctx).unwrap();
    assert!(!list.is_error);
    assert!(
        list.content.contains("- Style — .whycodes/rules/Style.md"),
        "{}",
        list.content
    );
    assert!(
        list.content.contains("- rust — .cursor/rules/rust.MDC"),
        "{}",
        list.content
    );
    assert!(
        list.content.contains("- style — .cursor/rules/style.mdc"),
        "{}",
        list.content
    );
    assert!(!list.content.contains("ignored"), "{}", list.content);
    assert!(!list.content.contains("nested"), "{}", list.content);

    let body = read_internal("rule://style", &ctx).unwrap();
    assert!(!body.is_error, "{}", body.content);
    assert_eq!(
        body.content,
        "# rule://Style (.whycodes/rules/Style.md)\n\nno unwrap\n"
    );
    let cursor_body = read_internal("rule:///rust", &ctx).unwrap();
    assert!(
        cursor_body.content.contains("cargo fmt"),
        "{}",
        cursor_body.content
    );

    let bad = read_internal("rule://a/b", &ctx).unwrap();
    assert!(bad.is_error);
    assert!(read_internal("rule://..", &ctx).unwrap().is_error);
    assert!(read_internal("rule://a\\b", &ctx).unwrap().is_error);
    let missing = read_internal("rule://nope", &ctx).unwrap();
    assert!(missing.is_error);
    assert!(
        missing.content.contains("No project rule 'nope'"),
        "{}",
        missing.content
    );

    let gone = rule_body("gone", &rules.join("gone.md"), dir.path());
    assert!(gone.is_error);
    assert!(
        gone.content
            .contains("Cannot read rule 'gone' at .whycodes/rules/gone.md"),
        "{}",
        gone.content
    );
    let outside = rule_label(std::path::Path::new("/elsewhere/x.md"), dir.path());
    assert_eq!(outside, "/elsewhere/x.md");
}

#[test]
fn mcp_url_outside_the_agent_loop_is_an_error() {
    let ctx = ToolContext::new("/tmp");
    let r = read_internal("mcp://docs/doc://guide", &ctx).unwrap();
    assert!(r.is_error);
    assert!(r.content.contains("mcp_read_resource"), "{}", r.content);
}

#[tokio::test]
async fn async_wrapper_delegates_sync_schemes_and_plain_paths() {
    let ctx = ToolContext::new("/tmp");
    assert!(read_internal_async("src/main.rs", &ctx).await.is_none());
    let guide = read_internal_async("agent://tools/read", &ctx)
        .await
        .unwrap();
    assert!(!guide.is_error);
    for scheme in [
        "skill://", "agent://", "proc://", "rule://", "pr://", "issue://", "mcp://",
    ] {
        assert!(
            guide.content.contains(scheme),
            "{scheme} missing: {}",
            guide.content
        );
    }
    let index = read_internal_async("agent://tools", &ctx).await.unwrap();
    assert!(index.content.contains("pr://"), "{}", index.content);
    for (url, needle) in [
        ("pr://abc", "pr://<number>"),
        ("issue://x", "issue://<number>"),
    ] {
        let r = read_internal_async(url, &ctx).await.unwrap();
        assert!(r.is_error && r.content.contains(needle), "{}", r.content);
    }
}
