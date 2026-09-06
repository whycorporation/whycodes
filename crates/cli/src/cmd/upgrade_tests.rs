use super::*;

#[test]
fn format_outcome_covers_arms() {
    assert!(crate::upgrade::format_upgrade_outcome("1", Ok(None)).contains("latest"));
    assert!(crate::upgrade::format_upgrade_outcome("1", Ok(Some("2".into()))).contains("2"));
    assert!(crate::upgrade::format_upgrade_outcome("1", Err("offline".into())).contains("offline"));
    assert!(
        crate::upgrade::format_upgrade_outcome("1", Err("brew upgrade".into())).contains("brew")
    );
}

#[test]
fn upgrade_printer_helpers() {
    assert!(upgrade_header_line().contains("Upgrade"));
    assert!(upgrade_current_line("0.1.0").contains("0.1.0"));
    assert!(upgrade_checking_line().contains("Checking"));
    assert!(upgrade_ok_line("1", Some("2".into())).contains("2"));
    assert!(upgrade_ok_line("1", None).contains("latest"));
    assert!(upgrade_err_line("1", "offline").contains("offline"));
    let src = upgrade_source_build_lines();
    assert!(src.iter().any(|l| l.contains("git clone")));
    assert!(should_print_source_build("offline"));
    assert!(!should_print_source_build("brew upgrade whycodes"));
}
