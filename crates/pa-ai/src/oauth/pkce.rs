//! PKCE pair generation: a random 43-character verifier and its S256 challenge, plus the
//! independent random CSRF `state`. The Codex flow keeps its own PKCE copy (TS duplicates the
//! helper per module the same way).

use base64::Engine as _;
use rand::Rng;
use sha2::{Digest, Sha256};

/// A PKCE pair: the verifier is the token-exchange secret, the
/// challenge travels in the authorization URL.
pub fn generate_pkce() -> (String, String) {
    let verifier = base64url(&random_bytes(32));
    let challenge = base64url(Sha256::digest(verifier.as_bytes()).as_slice());
    (verifier, challenge)
}

/// A random, URL-safe CSRF `state`, independent of the PKCE verifier: the verifier is the
/// token-exchange secret, `state` is the public token echoed on the redirect URL. Anthropic's own
/// Claude Code client mints its `state` the same way (`base64url(randomBytes(32))`).
pub fn generate_state() -> String {
    base64url(&random_bytes(32))
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill(&mut bytes[..]);
    bytes
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verifier_and_challenge_have_the_ts_shape() {
        let (verifier, challenge) = generate_pkce();
        assert_eq!(verifier.len(), 43, "32 bytes base64url-encoded");
        assert!(!verifier.contains(['+', '/', '=']));
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()).as_slice());
        assert_eq!(challenge, expected);
        // A fresh pair is a fresh secret.
        assert_ne!(verifier, generate_pkce().0);
    }

    #[test]
    fn the_state_is_an_independent_url_safe_secret() {
        let state = generate_state();
        assert_eq!(state.len(), 43, "32 bytes base64url-encoded");
        assert!(!state.contains(['+', '/', '=']));
        // The state is a fresh CSRF token, never a reuse of the verifier.
        let (verifier, _) = generate_pkce();
        assert_ne!(state, verifier);
        assert_ne!(state, generate_state());
    }
}
