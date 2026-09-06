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
    let t = SkillTool;
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
}
