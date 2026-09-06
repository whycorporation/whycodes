#[test]
fn missing_database_is_not_a_generic_error() {
    let err = anyhow::Error::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
    assert!(crate::cmd::helpers::is_missing_database(&err));
}
