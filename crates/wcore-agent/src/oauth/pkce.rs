//! PKCE (Proof Key for Code Exchange) — RFC 7636.
//!
//! Default mode for every `OAuthFlow` is `S256`. Opt-out is explicit:
//! callers must invoke `OAuthFlow::without_pkce()` and the absence is
//! recorded in the flow descriptor so security audits can find it.

use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Mode for a flow's PKCE binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkceMode {
    /// Default. SHA-256 challenge — `code_challenge = base64url(sha256(verifier))`.
    S256,
    /// Explicit opt-out. Only use for legacy providers that reject PKCE.
    Disabled,
}

/// Generated PKCE pair. The `verifier` is sent on the token exchange,
/// the `challenge` is sent on the authorize URL. RFC 7636 requires the
/// verifier to be base64url-no-pad encoded.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub method: PkceMode,
}

impl PkceChallenge {
    /// Generate a fresh S256 PKCE pair from a CSPRNG.
    ///
    /// 32 random bytes → ~43 base64url chars, comfortably inside RFC
    /// 7636's 43-128 char range.
    pub fn new_s256() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier,
            challenge,
            method: PkceMode::S256,
        }
    }

    /// Returns the RFC 7636 `code_challenge_method` string.
    pub fn method_str(&self) -> &'static str {
        match self.method {
            PkceMode::S256 => "S256",
            PkceMode::Disabled => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_uses_s256_method() {
        let pkce = PkceChallenge::new_s256();
        assert_eq!(pkce.method, PkceMode::S256);
        assert_eq!(pkce.method_str(), "S256");
    }

    #[test]
    fn pkce_verifier_round_trips_through_authorize_and_token_exchange() {
        // The verifier we'd send on the token POST must derive a
        // challenge identical to the one the authorize URL carried.
        let pkce = PkceChallenge::new_s256();
        let recomputed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(pkce.verifier.as_bytes()));
        assert_eq!(recomputed, pkce.challenge);
    }

    #[test]
    fn pkce_verifier_length_within_rfc7636_bounds() {
        let pkce = PkceChallenge::new_s256();
        let len = pkce.verifier.len();
        assert!(
            (43..=128).contains(&len),
            "RFC 7636 verifier length must be 43-128 chars, got {len}"
        );
    }

    #[test]
    fn pkce_pairs_are_unique_across_calls() {
        let a = PkceChallenge::new_s256();
        let b = PkceChallenge::new_s256();
        assert_ne!(a.verifier, b.verifier, "verifier must be random per flow");
        assert_ne!(a.challenge, b.challenge);
    }
}
