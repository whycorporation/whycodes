//! PKCE (RFC 7636) helpers for the browser + localhost callback flow.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// A PKCE verifier/challenge pair plus the OAuth `state` nonce.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_urlsafe(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_urlsafe(32);
        Self {
            verifier,
            challenge,
            state,
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
