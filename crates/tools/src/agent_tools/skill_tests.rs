use super::*;
use crate::tool::ToolContext;

#[tokio::test]
async fn list_and_load_use_project_working_dir() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".skills");
    std::fs::create_dir(&skills).unwrap();
    std::fs::write(
        skills.join("demo.skill.md"),
        "---\nname: Demo\ndescription: d\n---\n\nTHE BODY\n",
    )
    .unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let tool = SkillTool::new();
    let listed = tool.execute(json!({"action": "list"}), &ctx).await;
    assert!(!listed.is_error, "{}", listed.content);
    assert!(listed.content.contains("Demo"), "{}", listed.content);
    let loaded = tool
        .execute(json!({"action": "load", "name": "demo"}), &ctx)
        .await;
    assert!(!loaded.is_error, "{}", loaded.content);
    assert!(loaded.content.contains("THE BODY"), "{}", loaded.content);
    let missing = tool
        .execute(json!({"action": "load", "name": "nope"}), &ctx)
        .await;
    assert!(missing.is_error);

    let empty_dir = tempfile::tempdir().unwrap();
    let empty_ctx = ToolContext::new(empty_dir.path().to_string_lossy());
    let none = tool.execute(json!({"action": "list"}), &empty_ctx).await;
    assert!(!none.is_error, "{}", none.content);
    assert!(none.content.contains("No skills found"), "{}", none.content);

    let bad = tool.execute(json!({"action": "nope"}), &ctx).await;
    assert!(bad.is_error, "{}", bad.content);
    assert!(bad.content.contains("Unknown action"), "{}", bad.content);
}

#[tokio::test]
async fn load_requires_name_and_empty_description() {
    let t = SkillTool::default();
    assert_eq!(t.name(), "skill");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".skills");
    std::fs::create_dir(&skills).unwrap();
    std::fs::write(
        skills.join("bare.skill.md"),
        "---\nname: Bare\n---\n\nBODY\n",
    )
    .unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let listed = t.execute(json!({"action": "list"}), &ctx).await;
    assert!(listed.content.contains("(no description)") || listed.content.contains("Bare"));
    let missing_name = t.execute(json!({"action": "load"}), &ctx).await;
    assert!(missing_name.is_error, "{}", missing_name.content);
    let bad_project = t
        .execute(
            json!({"action": "list"}),
            &ToolContext::new("/nonexistent-xyz-project"),
        )
        .await;
    assert!(
        bad_project.is_error || !bad_project.content.is_empty(),
        "{}",
        bad_project.content
    );
    let bad_load = t
        .execute(
            json!({"action": "load", "name": "demo"}),
            &ToolContext::new("/nonexistent-xyz-project"),
        )
        .await;
    assert!(
        bad_load.is_error || !bad_load.content.is_empty(),
        "{}",
        bad_load.content
    );
}

#[test]
fn skill_format_helpers_cover_empty_and_loaded() {
    assert_eq!(format_skill_list(Vec::new()), "No skills found.");
    let listed = format_skill_list(vec![("Demo", "d"), ("Bare", "")]);
    assert!(listed.contains("Available skills (2)"), "{listed}");
    assert!(listed.contains("Demo: d"), "{listed}");
    assert!(listed.contains("Bare: (no description)"), "{listed}");
    assert_eq!(skill_description_label(""), "(no description)");
    assert_eq!(skill_description_label("hi"), "hi");
    assert_eq!(
        format_loaded_skill("Demo", "d", "BODY"),
        "Loaded skill 'Demo':\n\nd\n\nBODY"
    );
    assert!(skill_not_found("nope").contains("nope"));
    assert!(unknown_skill_action("x").contains("Unknown action 'x'"));
    let load_err = skill_load_error("boom");
    assert!(load_err.is_error);
    assert!(
        load_err.content.contains("Error loading skills: boom"),
        "{}",
        load_err.content
    );
    let blocked = tempfile::tempdir().unwrap();
    let as_file = blocked.path().join(".skills");
    std::fs::write(&as_file, "not-a-dir").unwrap();
    match load_skill_registry(blocked.path()) {
        Ok(_) => {}
        Err(e) => assert!(e.is_error),
    }
    assert!(take_skill_registry(Err(skill_load_error("boom"))).is_err());
    assert!(registry_load_failed(skill_load_error("boom")).is_error);
    assert!(skill_registry_or_err(Err(skill_load_error("boom"))).is_err());
    assert!(skill_registry_or_err(Ok(whycodes_skill::SkillRegistry::new())).is_ok());
    assert!(listed_skills(Err(skill_load_error("boom"))).is_error);
    assert!(loaded_skill(Err(skill_load_error("boom")), "demo").is_error);
    let listed = listed_skills(Ok(whycodes_skill::SkillRegistry::new()));
    assert!(!listed.is_error);
    assert!(listed.content.contains("No skills found"));
}
