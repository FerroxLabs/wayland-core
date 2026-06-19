//! "Sign in with X (Grok)" — xAI OAuth token manager.
//!
//! Engine-native parity with [`super::chatgpt`]: owns load / refresh / persist
//! of the xAI OAuth tokens so a Grok session survives the ~6h access-token
//! lifetime without the host re-spawning. Bootstrap builds an async bearer
//! closure over [`XaiTokenManager::get`] and hands it to the OpenAI-compatible
//! provider (`wcore-providers` stays free of any OAuth dependency).
//!
//! Two differences from ChatGPT, both verified live (2026-06-18):
//! - xAI's refresh grant REQUIRES the `scope` form field (ChatGPT omits it).
//! - The access token works directly against `api.x.ai/v1` as a plain bearer
//!   (no account-id header, no Codex backend), so [`get`] returns just the
//!   token string.
//!
//! Token source. Two places hold xAI credentials, and the manager prefers
//! whichever is FRESHER so it rarely has to refresh itself (which avoids
//! racing the Grok CLI for the single-use, rotating refresh token):
//! - the engine's own store `~/.wayland/oauth/xai.json` (written by the Wayland
//!   app's "Sign in with X (Grok)" flow or by a prior refresh);
//! - the Grok CLI's `~/.grok/auth.json` (the CLI keeps it fresh), whose `key`
//!   field is the access token, nested under a `"https://auth.x.ai::<cid>"`
//!   wrapper.
//!
//! Rotation. xAI rotates the refresh token on every refresh (single-use), so a
//! refresh that succeeds but fails to persist is a HARD error (C4) — the old
//! token is already burned server-side.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use crate::oauth::{OAuthFlow, OAuthStorage, OAuthTokens, RefreshError, SingleFlightRefresh};

/// Provider name used by [`OAuthStorage`] when persisting tokens.
pub const PROVIDER: &str = "xai";

/// xAI OIDC token endpoint (refresh grant). Confirmed live from the discovery
/// doc at `https://auth.x.ai/.well-known/openid-configuration`.
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";

/// xAI OIDC authorize endpoint (browser PKCE login). Carried on the flow for
/// completeness; engine-side we only run the refresh grant — interactive Grok
/// login lives in the Wayland app's "Sign in with X (Grok)", not a CLI verb.
const XAI_AUTH_URL: &str = "https://auth.x.ai/oauth2/authorize";

/// Public desktop PKCE client_id (no secret). Verified live: this id refreshes
/// the SuperGrok / Grok-CLI token against `auth.x.ai`. Override at runtime with
/// `WAYLAND_XAI_OAUTH_CLIENT_ID` so a corrected value needs no rebuild.
const XAI_CLIENT_ID_DEFAULT: &str = "b1a00492-073a-47ea-816f-4c329264a828";

/// Scopes requested. `grok-cli:access` + `api:access` are the SuperGrok
/// entitlements; `offline_access` yields the refresh token. xAI's refresh
/// grant echoes these in the form body (verified live).
const XAI_SCOPES: &str = "openid profile email offline_access grok-cli:access api:access";

/// Refresh `REFRESH_LEAD_SECS` before the token actually expires so a turn
/// never starts on a token that lapses mid-flight.
const REFRESH_LEAD_SECS: u64 = 120;

/// Per network call ceiling for the refresh round-trip.
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Sentinel marking a `429` refresh (rate limit, not auth failure) so the
/// caller can keep using a still-valid current token (C3).
const RATE_LIMIT_SENTINEL: &str = "__xai_refresh_rate_limited__";

/// Resolve the client_id: env override wins over the pinned default.
fn xai_client_id() -> String {
    std::env::var("WAYLAND_XAI_OAUTH_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| XAI_CLIENT_ID_DEFAULT.to_string())
}

/// Build the xAI OAuth flow descriptor. Only `token_url` + `client_id` matter
/// for the refresh-only path; the authorize fields are for the login flow.
pub fn build_xai_flow() -> OAuthFlow {
    OAuthFlow::new(
        xai_client_id(),
        None,
        XAI_AUTH_URL,
        XAI_TOKEN_URL,
        XAI_SCOPES.split(' ').map(str::to_string).collect(),
    )
}

/// Decode a JWT's `exp` (epoch seconds) without verifying the signature — used
/// to read the access-token expiry out of the Grok CLI's `key`. Returns `None`
/// when the segment is absent / not base64url / not JSON / lacks `exp`.
fn decode_jwt_exp(token: &str) -> Option<u64> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let seg = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(seg).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp").and_then(|e| e.as_u64())
}

/// Path to the Grok CLI credential file: `$GROK_HOME/auth.json`, default
/// `~/.grok/auth.json`. Mirrors `chatgpt::codex_home_dir`.
fn grok_auth_json_path() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("GROK_HOME")
        && !dir.trim().is_empty()
    {
        return Some(std::path::PathBuf::from(dir).join("auth.json"));
    }
    Some(dirs::home_dir()?.join(".grok").join("auth.json"))
}

/// Read the Grok CLI's `~/.grok/auth.json` into our token shape. The file nests
/// the bundle under a single `"https://auth.x.ai::<client_id>"` wrapper whose
/// `key` is the access token; the expiry comes from the JWT's `exp` (the file's
/// own `expires_at` string has been observed stale). Returns `None` when the
/// file is absent / malformed / carries no `key`.
pub fn read_grok_cli_tokens() -> Option<OAuthTokens> {
    let path = grok_auth_json_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let doc: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    // Find the nested entry that carries a `key` (the host-prefixed wrapper).
    let entry = doc
        .as_object()?
        .values()
        .find_map(|v| v.as_object().filter(|o| o.contains_key("key")))?;
    let key = entry.get("key")?.as_str()?.to_string();
    let refresh_token = entry
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let expires_at_unix_secs = decode_jwt_exp(&key);
    Some(OAuthTokens {
        access_token: key,
        refresh_token,
        expires_at_unix_secs,
        token_type: "Bearer".to_string(),
        scope: Some(XAI_SCOPES.to_string()),
        id_token: None,
    })
}

/// Owns load / refresh / persist of the xAI OAuth tokens. Built by bootstrap;
/// the async bearer closure handed to the provider calls [`XaiTokenManager::get`].
pub struct XaiTokenManager {
    flow: Arc<OAuthFlow>,
    single_flight: Arc<SingleFlightRefresh>,
    client: wcore_egress::EgressClient,
    storage: OAuthStorage,
    cached: Mutex<Option<OAuthTokens>>,
}

impl XaiTokenManager {
    pub fn new(storage: OAuthStorage) -> Self {
        Self {
            flow: Arc::new(build_xai_flow()),
            single_flight: Arc::new(SingleFlightRefresh::new()),
            client: wcore_egress::EgressClient::tool(),
            storage,
            cached: Mutex::new(None),
        }
    }

    /// Construct with an explicit flow descriptor (out-of-crate tests point the
    /// refresh round-trip at a local mock token server).
    #[doc(hidden)]
    pub fn new_with_flow(storage: OAuthStorage, flow: OAuthFlow) -> Self {
        Self {
            flow: Arc::new(flow),
            single_flight: Arc::new(SingleFlightRefresh::new()),
            client: wcore_egress::EgressClient::tool(),
            storage,
            cached: Mutex::new(None),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Valid for at least `REFRESH_LEAD_SECS` more seconds. A missing expiry is
    /// treated as stale (forces a refresh) rather than fresh.
    fn token_is_fresh(t: &OAuthTokens) -> bool {
        let Some(exp) = t.expires_at_unix_secs else {
            return false;
        };
        exp.saturating_sub(REFRESH_LEAD_SECS) > Self::now_secs()
    }

    /// Past actual expiry (no lead window). Used by the 429 path.
    fn token_is_hard_expired(t: &OAuthTokens) -> bool {
        let Some(exp) = t.expires_at_unix_secs else {
            return true;
        };
        exp <= Self::now_secs()
    }

    /// Pick the token that expires later (None expiry sorts earliest).
    fn fresher(a: Option<OAuthTokens>, b: Option<OAuthTokens>) -> Option<OAuthTokens> {
        match (a, b) {
            (Some(a), Some(b)) => {
                let ea = a.expires_at_unix_secs.unwrap_or(0);
                let eb = b.expires_at_unix_secs.unwrap_or(0);
                Some(if eb > ea { b } else { a })
            }
            (a, None) => a,
            (None, b) => b,
        }
    }

    /// Load the active token: in-memory cache, else the FRESHER of the engine
    /// store and the Grok CLI file. Preferring the fresher source means that
    /// when the Grok CLI has already refreshed, we ride its token instead of
    /// doing our own (avoids racing it for the single-use refresh token).
    /// A miss returns `Ok(None)` so the caller can surface login guidance.
    async fn load_active(&self) -> Result<Option<OAuthTokens>, String> {
        let mut guard = self.cached.lock().await;
        if guard.is_some() {
            return Ok(guard.clone());
        }
        let from_store = self
            .storage
            .load(PROVIDER)
            .map_err(|e| format!("oauth storage load failed: {e}"))?;
        let from_cli = read_grok_cli_tokens();
        let chosen = Self::fresher(from_store, from_cli);
        *guard = chosen.clone();
        Ok(chosen)
    }

    /// Clear the in-memory token cache (logout calls this).
    pub async fn clear_cache(&self) {
        *self.cached.lock().await = None;
    }

    /// Return the access token, refreshing if near expiry.
    pub async fn get(&self) -> Result<String, String> {
        let tokens = self.load_active().await?.ok_or_else(|| {
            "not signed in to Grok — use \"Sign in with X (Grok)\" in the Wayland app, \
             sign in with the Grok CLI (creates ~/.grok/auth.json), or set XAI_API_KEY"
                .to_string()
        })?;
        let tokens = if Self::token_is_fresh(&tokens) {
            tokens
        } else {
            self.refresh(tokens).await?
        };
        Ok(tokens.access_token)
    }

    /// Refresh `current` via the rotating-refresh-token grant.
    ///
    /// C3: a `429` keeps the still-valid current token. C4: a refresh that
    /// rotated the refresh token but failed to persist is a HARD error.
    async fn refresh(&self, current: OAuthTokens) -> Result<OAuthTokens, String> {
        let refresh_token = current.refresh_token.clone().ok_or(
            "no refresh_token for Grok — sign in with X (Grok) again in the Wayland app or via the Grok CLI",
        )?;
        let client = self.client.clone();
        let token_url = self.flow.token_url.clone();
        let client_id = self.flow.client_id.clone();

        let refreshed = self
            .single_flight
            .refresh(move || async move {
                // xAI REQUIRES the scope field on refresh (unlike ChatGPT).
                let form: Vec<(&str, String)> = vec![
                    ("grant_type", "refresh_token".into()),
                    ("refresh_token", refresh_token),
                    ("client_id", client_id),
                    ("scope", XAI_SCOPES.into()),
                ];
                let res = tokio::time::timeout(
                    PER_CALL_TIMEOUT,
                    client.post(&token_url).form(&form).send(),
                )
                .await
                .map_err(|_| RefreshError::Transport("refresh timed out".into()))?
                .map_err(|e| RefreshError::Transport(e.to_string()))?;

                let status = res.status();
                let body = res
                    .text()
                    .await
                    .map_err(|e| RefreshError::Transport(e.to_string()))?;

                if status.as_u16() == 429 {
                    return Err(RefreshError::Transport(RATE_LIMIT_SENTINEL.into()));
                }
                if !status.is_success() {
                    // Surface only the status, never the token-endpoint body.
                    return Err(RefreshError::ProviderRejected(format!(
                        "token endpoint rejected refresh: HTTP {}",
                        status.as_u16()
                    )));
                }
                let raw: serde_json::Value = serde_json::from_str(&body)
                    .map_err(|e| RefreshError::Transport(format!("malformed token JSON: {e}")))?;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Ok(OAuthTokens {
                    access_token: raw
                        .get("access_token")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            RefreshError::ProviderRejected("missing access_token".into())
                        })?
                        .to_string(),
                    refresh_token: raw
                        .get("refresh_token")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    expires_at_unix_secs: raw
                        .get("expires_in")
                        .and_then(|v| v.as_u64())
                        .map(|s| now + s),
                    token_type: raw
                        .get("token_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Bearer")
                        .to_string(),
                    scope: raw
                        .get("scope")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    id_token: raw
                        .get("id_token")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                })
            })
            .await;

        let refreshed = match refreshed {
            Ok(t) => t,
            Err(RefreshError::Transport(msg)) if msg == RATE_LIMIT_SENTINEL => {
                if Self::token_is_hard_expired(&current) {
                    return Err(
                        "Grok refresh is rate limited (429) and the access token has expired — \
                         try again shortly."
                            .to_string(),
                    );
                }
                *self.cached.lock().await = Some(current.clone());
                return Ok(current);
            }
            Err(e) => return Err(format!("refresh failed: {e}")),
        };

        // C4: rotation vs server-omitted refresh token.
        let rotated = refreshed.refresh_token.is_some();
        let mut to_store = refreshed;
        if to_store.refresh_token.is_none() {
            to_store.refresh_token = current.refresh_token.clone();
        }
        if let Err(e) = self.storage.store(PROVIDER, &to_store) {
            if rotated {
                return Err(format!(
                    "Grok refresh rotated the refresh token but persisting it failed ({e}); \
                     sign in with X again to re-authenticate"
                ));
            }
            tracing::warn!(error = %e, "failed to persist refreshed Grok access token");
        }
        *self.cached.lock().await = Some(to_store.clone());
        Ok(to_store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn token(access: &str, refresh: Option<&str>, exp: Option<u64>) -> OAuthTokens {
        OAuthTokens {
            access_token: access.into(),
            refresh_token: refresh.map(str::to_string),
            expires_at_unix_secs: exp,
            token_type: "Bearer".into(),
            scope: None,
            id_token: None,
        }
    }

    fn far_future() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 3600
    }

    #[test]
    fn flow_uses_xai_endpoints_and_default_client() {
        let f = build_xai_flow();
        assert_eq!(f.token_url, XAI_TOKEN_URL);
        assert_eq!(f.client_id, XAI_CLIENT_ID_DEFAULT);
        assert!(f.scopes.iter().any(|s| s == "api:access"));
        assert!(f.scopes.iter().any(|s| s == "offline_access"));
    }

    #[test]
    fn fresher_prefers_the_later_expiry() {
        let a = token("a", None, Some(100));
        let b = token("b", None, Some(200));
        assert_eq!(
            XaiTokenManager::fresher(Some(a.clone()), Some(b.clone()))
                .unwrap()
                .access_token,
            "b"
        );
        // None expiry loses to a real one; a single Some wins over None.
        assert_eq!(
            XaiTokenManager::fresher(Some(token("x", None, None)), Some(a.clone()))
                .unwrap()
                .access_token,
            "a"
        );
        assert_eq!(
            XaiTokenManager::fresher(Some(a), None)
                .unwrap()
                .access_token,
            "a"
        );
    }

    #[test]
    fn freshness_window_respects_lead_secs() {
        assert!(XaiTokenManager::token_is_fresh(&token(
            "a",
            Some("r"),
            Some(far_future())
        )));
        // Inside the 120s lead window → stale.
        let soon = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 30;
        assert!(!XaiTokenManager::token_is_fresh(&token(
            "a",
            Some("r"),
            Some(soon)
        )));
        // Unknown expiry → stale (forces refresh).
        assert!(!XaiTokenManager::token_is_fresh(&token(
            "a",
            Some("r"),
            None
        )));
    }

    #[tokio::test]
    async fn get_returns_a_fresh_stored_token_without_refreshing() {
        // Isolate from any real ~/.grok/auth.json so the test is deterministic.
        unsafe { std::env::set_var("GROK_HOME", "/nonexistent-grok-home-for-test") };
        let tmp = TempDir::new().unwrap();
        let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        storage
            .store(PROVIDER, &token("at-fresh", Some("rt"), Some(far_future())))
            .unwrap();
        let mgr = XaiTokenManager::new(storage);
        assert_eq!(mgr.get().await.unwrap(), "at-fresh");
        unsafe { std::env::remove_var("GROK_HOME") };
    }

    #[test]
    fn reads_real_grok_auth_json_shape() {
        // The host-prefixed wrapper with a `key` is extracted; a JWT `key`
        // yields an expiry from its `exp` claim.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        // {"exp": 9999999999} payload, unsigned, base64url.
        let jwt = "aGVhZA.eyJleHAiOiA5OTk5OTk5OTk5fQ.sig";
        let doc = format!(
            r#"{{"https://auth.x.ai::cid":{{"key":"{jwt}","refresh_token":"rt-xyz","email":"x@y.z"}}}}"#
        );
        std::fs::write(home.join(".grok").join("auth.json"), doc).unwrap();
        unsafe { std::env::set_var("GROK_HOME", home.join(".grok")) };
        let t = read_grok_cli_tokens().expect("parses the real shape");
        assert_eq!(t.access_token, jwt);
        assert_eq!(t.refresh_token.as_deref(), Some("rt-xyz"));
        assert_eq!(t.expires_at_unix_secs, Some(9_999_999_999));
        unsafe { std::env::remove_var("GROK_HOME") };
    }
}
