#[test]
fn oauth_providers_mirrors_registered_names() {
    let names = super::oauth_providers();
    let registered = super::registered_providers();
    assert_eq!(names, registered);
}
