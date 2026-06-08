//! ACP auth schemes + verifier trait.
//!
//! Three schemes are wired:
//! - `ApiKey` — opaque-string header
//! - `Bearer` — RFC 6750 Authorization: Bearer <token>
//! - `OAuth` — scaffolded for device-flow; full impl is deferred
//!
//! All schemes pull their secret from the OS keychain (via
//! `wcore-config::keychain`) rather than process env or config files.
//! For local dev the keychain has fallback env-var lookup behavior in
//! `wcore-config::keychain` itself; here we just accept whatever
//! `keychain::get_secret` returns.

use serde::{Deserialize, Serialize};

use crate::error::AcpError;

/// Service name used to namespace ACP keychain entries.
pub const KEYCHAIN_SERVICE: &str = "acp";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum AuthScheme {
    ApiKey { account: String },
    Bearer { account: String },
    OAuth { config: OAuthConfig },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

/// Authenticated principal returned by a successful verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: String,
    pub scheme: AuthSchemeKind,
}

/// Lightweight tag for the scheme that produced a principal — avoids
/// carrying the full `AuthScheme` (with secret-bearing fields) around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthSchemeKind {
    ApiKey,
    Bearer,
    OAuth,
}

/// Trait implemented by anything that can verify headers + return a
/// principal. Server middleware composes this trait object.
pub trait Verifier: Send + Sync {
    /// Verify a header map. Returns `Ok(Principal)` on success, or an
    /// `AcpError::Auth(...)` on failure.
    fn verify(&self, headers: &[(String, String)]) -> Result<Principal, AcpError>;
}

/// API-key verifier — compares an `X-API-Key` (or `Authorization: ApiKey
/// <value>`) header against the secret stored under `(service, account)`
/// in the OS keychain.
pub struct ApiKeyVerifier {
    account: String,
}

impl ApiKeyVerifier {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
}

impl Verifier for ApiKeyVerifier {
    fn verify(&self, headers: &[(String, String)]) -> Result<Principal, AcpError> {
        let presented = headers
            .iter()
            .find_map(|(k, v)| {
                if k.eq_ignore_ascii_case("x-api-key") {
                    Some(v.clone())
                } else if k.eq_ignore_ascii_case("authorization") {
                    let v = v
                        .strip_prefix("ApiKey ")
                        .or_else(|| v.strip_prefix("apikey "));
                    v.map(|s| s.to_string())
                } else {
                    None
                }
            })
            .ok_or_else(|| AcpError::Auth("missing api-key header".to_string()))?;

        let expected = wcore_config::keychain::get_secret(KEYCHAIN_SERVICE, &self.account)
            .map_err(|e| AcpError::Auth(format!("keychain lookup failed: {e}")))?;

        if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
            Ok(Principal {
                id: self.account.clone(),
                scheme: AuthSchemeKind::ApiKey,
            })
        } else {
            Err(AcpError::Auth("api key mismatch".to_string()))
        }
    }
}

/// Bearer verifier — RFC 6750 `Authorization: Bearer <token>` against
/// keychain.
pub struct BearerVerifier {
    account: String,
}

impl BearerVerifier {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
}

impl Verifier for BearerVerifier {
    fn verify(&self, headers: &[(String, String)]) -> Result<Principal, AcpError> {
        let presented = headers
            .iter()
            .find_map(|(k, v)| {
                if k.eq_ignore_ascii_case("authorization") {
                    v.strip_prefix("Bearer ").map(|s| s.to_string())
                } else {
                    None
                }
            })
            .ok_or_else(|| AcpError::Auth("missing Bearer authorization header".to_string()))?;

        let expected = wcore_config::keychain::get_secret(KEYCHAIN_SERVICE, &self.account)
            .map_err(|e| AcpError::Auth(format!("keychain lookup failed: {e}")))?;

        if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
            Ok(Principal {
                id: self.account.clone(),
                scheme: AuthSchemeKind::Bearer,
            })
        } else {
            Err(AcpError::Auth("bearer token mismatch".to_string()))
        }
    }
}

/// Deny-by-default verifier used when no auth is configured. Always
/// returns `Err(AcpError::Auth(...))`. Production callers should
/// install a real verifier; this exists so the type plumbing always
/// has a default.
pub struct DenyAllVerifier;

impl Verifier for DenyAllVerifier {
    fn verify(&self, _headers: &[(String, String)]) -> Result<Principal, AcpError> {
        Err(AcpError::Auth(
            "no verifier configured (deny-all)".to_string(),
        ))
    }
}

/// Helper to store/retrieve an ACP API key in the OS keychain.
pub fn store_api_key(account: &str, key: &str) -> Result<(), AcpError> {
    wcore_config::keychain::store_secret(KEYCHAIN_SERVICE, account, key)
        .map_err(|e| AcpError::Auth(format!("keychain store failed: {e}")))
}

/// Constant-time byte comparison used for secret material.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_denies() {
        let v = DenyAllVerifier;
        let r = v.verify(&[]);
        assert!(matches!(r, Err(AcpError::Auth(_))));
    }

    #[test]
    fn api_key_missing_header() {
        let v = ApiKeyVerifier::new("test-acct");
        let r = v.verify(&[("content-type".to_string(), "application/json".to_string())]);
        assert!(matches!(r, Err(AcpError::Auth(_))));
    }

    #[test]
    fn bearer_missing_header() {
        let v = BearerVerifier::new("test-acct");
        let r = v.verify(&[]);
        assert!(matches!(r, Err(AcpError::Auth(_))));
    }

    #[test]
    fn bearer_wrong_scheme_rejected() {
        let v = BearerVerifier::new("test-acct");
        let r = v.verify(&[("authorization".to_string(), "Basic xxx".to_string())]);
        assert!(matches!(r, Err(AcpError::Auth(_))));
    }

    #[test]
    fn constant_time_eq_correctness() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn scheme_serialization_tags() {
        let s = AuthScheme::ApiKey {
            account: "foo".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"api_key\""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn api_key_roundtrip_macos() {
        let account = "test-1a8-roundtrip";
        let key = "super-secret-key";
        // Set up keychain entry.
        store_api_key(account, key).expect("store");

        let v = ApiKeyVerifier::new(account);
        let ok = v.verify(&[("X-API-Key".to_string(), key.to_string())]);
        assert!(ok.is_ok(), "expected ok, got {ok:?}");

        let bad = v.verify(&[("X-API-Key".to_string(), "wrong".to_string())]);
        assert!(matches!(bad, Err(AcpError::Auth(_))));

        // Cleanup.
        let _ = wcore_config::keychain::delete_secret(KEYCHAIN_SERVICE, account);
    }
}
