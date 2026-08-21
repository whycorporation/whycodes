use super::*;

#[test]
fn challenge_is_s256_of_verifier() {
    let p = Pkce::new();
    let expect = URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
    assert_eq!(p.challenge, expect);
    assert!(!p.state.is_empty());
    assert_ne!(p.verifier, p.challenge);
}

#[test]
fn default_generates_a_new_pair() {
    let first = Pkce::default();
    let second = Pkce::default();
    assert_ne!(first.verifier, second.verifier);
    assert_ne!(first.state, second.state);
}
