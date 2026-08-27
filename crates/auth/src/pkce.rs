//! PKCE (RFC 7636) helpers for the browser + localhost callback flow.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use sha2::{Digest, Sha256};

/// A PKCE verifier/challenge pair plus the OAuth `state` nonce.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
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
#[path = "pkce_tests.rs"]
mod tests;
