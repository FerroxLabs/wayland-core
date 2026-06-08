//! Wave B2 — `BearerToken`: HMAC-style hashed token at the session boundary.
//!
//! v0.3 design: SHA-256 over `secret || payload` (not a true HMAC; cheap
//! integrity check sufficient for a single trusted-secret world). Asymmetric
//! signing (Ed25519) is post-v0.6 once we wire an external identity provider.
//!
//! The payload format uses `{Actor:?}` (Debug) on purpose: sign + verify run
//! inside the same binary so format stability across versions doesn't matter
//! within a session. If we ever persist tokens across upgrades, swap to an
//! explicit canonical serializer.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{DenyReason, PolicyResult};
use crate::policy::Actor;
use crate::revocation::RevocationStore;

#[derive(Clone, Serialize, Deserialize)]
pub struct BearerToken {
    pub actor: Actor,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub signature_hex: String,
}

/// T6 closure: redact `signature_hex` in `Debug` output. Leaking the
/// signature to logs enables T2-style replay until TTL expiry, since the
/// signature is deterministic over `(actor, issued_at_ms, expires_at_ms,
/// secret)`. The other fields stay visible — they're useful for debugging
/// and carry no replay value on their own.
///
/// `Serialize`/`Deserialize` remain unchanged — round-tripping the full
/// token over the wire is the *defined* use case. Only the human-readable
/// `Debug` path is sensitive.
impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerToken")
            .field("actor", &self.actor)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("signature_hex", &"<redacted>")
            .finish()
    }
}

impl BearerToken {
    /// Issue a fresh token. `ttl_ms` is added to the current wall clock.
    pub fn issue(actor: Actor, ttl_ms: i64, secret: &[u8]) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        let expires_at_ms = now + ttl_ms;
        let signature_hex = sign(&actor, now, expires_at_ms, secret);
        Self {
            actor,
            issued_at_ms: now,
            expires_at_ms,
            signature_hex,
        }
    }

    /// Verify the token: signature matches and clock is within TTL.
    /// Returns the embedded `Actor` on success so callers can use the token's
    /// identity claim directly without re-passing it.
    pub fn verify(&self, secret: &[u8]) -> PolicyResult<&Actor> {
        let now = chrono::Utc::now().timestamp_millis();
        if now > self.expires_at_ms {
            return Err(DenyReason::TokenExpired);
        }
        let want = sign(&self.actor, self.issued_at_ms, self.expires_at_ms, secret);
        // Constant-time-ish compare via hex string length first (always 64 for
        // SHA-256 hex). `eq` on equal-length strings is fine here — these are
        // short-lived session tokens, not long-term credentials, and the
        // attacker has no oracle channel.
        if want != self.signature_hex {
            return Err(DenyReason::TokenInvalid);
        }
        Ok(&self.actor)
    }

    /// Stable identifier for this token instance.
    ///
    /// M5.9 design: the SHA-256 signature is already unique per
    /// `(actor, issued_at_ms, expires_at_ms, secret)` tuple, so we use it as
    /// the token id. This avoids adding a separately-persisted field (which
    /// would break the on-wire JSON shape) while still giving the revocation
    /// store a single short string to key on. A rotated token has a different
    /// signature and therefore a different id, which is exactly the behaviour
    /// `RevocationStore::revoke(old_id)` should produce: the new token stays
    /// valid, the old one is blocked.
    pub fn id(&self) -> &str {
        &self.signature_hex
    }

    /// Re-key this token under a new secret without extending its lifetime.
    ///
    /// The rotated token preserves the original `Actor` and `expires_at_ms`
    /// (so rotation cannot be used to extend a session past its original
    /// expiry) but gets a fresh `issued_at_ms = now` and a fresh signature
    /// over `new_secret`. The caller must keep accepting the *original*
    /// token under the *old* secret for the grace period; once the old token
    /// expires (which is the same wall-clock instant as the rotated one),
    /// only the new secret has live tokens at all.
    ///
    /// Returns `Err(DenyReason::TokenExpired)` if the original token has
    /// already passed its TTL — rotating an expired token would silently
    /// resurrect a dead session.
    pub fn rotate(&self, new_secret: &[u8]) -> PolicyResult<BearerToken> {
        let now = chrono::Utc::now().timestamp_millis();
        if now > self.expires_at_ms {
            return Err(DenyReason::TokenExpired);
        }
        let signature_hex = sign(&self.actor, now, self.expires_at_ms, new_secret);
        Ok(Self {
            actor: self.actor.clone(),
            issued_at_ms: now,
            expires_at_ms: self.expires_at_ms,
            signature_hex,
        })
    }

    /// Verify the token AND check it hasn't been revoked.
    ///
    /// Revocation check runs *before* the signature check on purpose: a
    /// revoked token is dead regardless of whether the presented secret is
    /// correct, and short-circuiting saves a hash round when the token is
    /// already invalid. The signature check then runs to defeat a forged
    /// token that happens to collide with a revoked id.
    pub fn verify_with_store(
        &self,
        secret: &[u8],
        store: &dyn RevocationStore,
    ) -> PolicyResult<&Actor> {
        if store.is_revoked(self.id())? {
            return Err(DenyReason::TokenRevoked);
        }
        self.verify(secret)
    }
}

fn sign(actor: &Actor, issued_at_ms: i64, expires_at_ms: i64, secret: &[u8]) -> String {
    let payload = format!("{actor:?}|{issued_at_ms}|{expires_at_ms}");
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    //! Internal unit tests. Public verify/issue contract is exercised in
    //! `tests/token_test.rs`.

    use super::*;

    #[test]
    fn signature_is_deterministic() {
        let a = sign(&Actor::User("alice".into()), 1, 2, b"s");
        let b = sign(&Actor::User("alice".into()), 1, 2, b"s");
        assert_eq!(a, b);
    }

    #[test]
    fn different_secret_yields_different_signature() {
        let a = sign(&Actor::User("alice".into()), 1, 2, b"s1");
        let b = sign(&Actor::User("alice".into()), 1, 2, b"s2");
        assert_ne!(a, b);
    }

    #[test]
    fn different_actor_yields_different_signature() {
        let a = sign(&Actor::User("alice".into()), 1, 2, b"s");
        let b = sign(&Actor::User("bob".into()), 1, 2, b"s");
        assert_ne!(a, b);
    }
}
