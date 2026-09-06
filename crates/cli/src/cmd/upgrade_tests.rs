#[test]
fn format_outcome_covers_arms() {
    assert!(crate::upgrade::format_upgrade_outcome("1", Ok(None)).contains("latest"));
    assert!(crate::upgrade::format_upgrade_outcome("1", Ok(Some("2".into()))).contains("2"));
    assert!(crate::upgrade::format_upgrade_outcome("1", Err("offline".into())).contains("offline"));
    assert!(
        crate::upgrade::format_upgrade_outcome("1", Err("brew upgrade".into())).contains("brew")
    );
}
