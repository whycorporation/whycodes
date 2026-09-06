use super::*;

#[test]
fn parse_shell_risk_detail() {
    let p = parse_permission_detail("Command:\nrm -rf /tmp/x\n\nRisk: destructive delete");
    assert_eq!(p.body, "rm -rf /tmp/x");
    assert_eq!(p.risk.as_deref(), Some("destructive delete"));
    assert!(p.is_command);
}

#[test]
fn parse_bare_command() {
    let p = parse_permission_detail("ls -la");
    assert_eq!(p.body, "ls -la");
    assert!(p.is_command);
    assert!(p.risk.is_none());
}

#[test]
fn parse_key_value_detail() {
    let p = parse_permission_detail("path: src/main.rs\noffset: 10");
    assert!(!p.is_command);
    assert!(p.body.contains("path: src/main.rs"));
    assert!(p.risk.is_none());
}
