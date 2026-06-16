//! "Sign in with ChatGPT" — Codex OAuth token manager.
//!
//! Public PKCE client (no client secret); refresh tokens ROTATE (single-use)
//! so every refresh must re-persist the new `refresh_token`; the ChatGPT
//! account id is read from the access-token JWT, not a separate API call.
//!
//! Layering: this manager owns `OAuthStorage` + refresh + JWT decode and
//! lives in `wcore-agent`. `wcore-providers` stays free of any OAuth
//! dependency — bootstrap builds an async bearer closure over a
//! [`ChatGptTokenManager`] and hands it to the provider.
//!
//! Cross-audit revisions baked in:
//! - C3: a `429` on refresh is a rate-limit, not an auth failure. When the
//!   current access token is not hard-expired we return it unchanged rather
//!   than failing the whole turn.
//! - C4: a failed persist of a ROTATED refresh token is a HARD error (a
//!   silent persist failure burns the old single-use token server-side and
//!   locks the user out next process start). A server that simply omits the
//!   refresh token (genuine non-rotation) keeps the old token and is safe.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use crate::oauth::{
    OAuthFlow, OAuthStorage, OAuthTokens, RedirectStrategy, RefreshError, SingleFlightRefresh,
};

/// Provider name used by [`OAuthStorage`] when persisting tokens.
pub const PROVIDER: &str = "chatgpt";
/// OpenAI's published Codex public client (no client secret).
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Fixed port registered against the Codex client's redirect_uri.
pub const CALLBACK_PORT: u16 = 1455;
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const CALLBACK_HOST: &str = "localhost";
pub const SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];
/// Our honest attribution sent as the `originator` authorize param + header.
pub const ORIGINATOR: &str = "wayland";

/// Refresh this many seconds before expiry to absorb clock skew.
const REFRESH_LEAD_SECS: u64 = 120;
/// Outer wall-clock cap on the refresh round-trip.
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Sentinel embedded in the refresh error when the token endpoint returns a
/// `429`. Lets [`ChatGptTokenManager::refresh`] distinguish a rate-limit
/// (token still valid) from a genuine auth rejection after the single-flight
/// gate has flattened everything into a `RefreshError`.
const RATE_LIMIT_SENTINEL: &str = "__chatgpt_refresh_rate_limited__";

/// Build the ChatGPT Codex OAuth flow: fixed port 1455, `localhost` redirect
/// host, `/auth/callback` path, and the three Codex authorize extras.
pub fn build_chatgpt_flow() -> OAuthFlow {
    OAuthFlow::new(
        CLIENT_ID,
        None,
        AUTHORIZE_URL,
        TOKEN_URL,
        SCOPES.iter().map(|s| s.to_string()).collect(),
    )
    .with_redirect_strategy(RedirectStrategy::FixedPort(CALLBACK_PORT))
    .with_redirect_uri_parts(CALLBACK_HOST, CALLBACK_PATH)
    .with_extra_auth_params(vec![
        ("id_token_add_organizations".into(), "true".into()),
        ("codex_cli_simplified_flow".into(), "true".into()),
        ("originator".into(), ORIGINATOR.into()),
    ])
}

/// Claims extracted from the access-token JWT's
/// `https://api.openai.com/auth` namespace.
#[derive(Debug, Clone)]
pub struct CodexClaims {
    pub account_id: String,
    pub plan_type: Option<String>,
}

/// Decode the JWT payload (segment `[1]`, base64url, NO signature
/// verification — the token is already trusted; we only read claims) and pull
/// the ChatGPT account id + plan from the `https://api.openai.com/auth`
/// namespace claim. Errors if the segment is absent, not base64url, not JSON,
/// or carries no `chatgpt_account_id`.
pub fn decode_codex_claims(access_token: &str) -> Result<CodexClaims, String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let seg = access_token.split('.').nth(1).ok_or("not a JWT")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(seg)
        .map_err(|e| format!("b64: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("json: {e}"))?;
    let auth = v.get("https://api.openai.com/auth").ok_or("no auth claim")?;
    let account_id = auth
        .get("chatgpt_account_id")
        .and_then(|x| x.as_str())
        .ok_or("no chatgpt_account_id")?
        .to_string();
    let plan_type = auth
        .get("chatgpt_plan_type")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    Ok(CodexClaims {
        account_id,
        plan_type,
    })
}

/// Owns load / refresh / persist of the ChatGPT Codex OAuth tokens plus the
/// access-token JWT decode. Built by bootstrap; the async bearer closure
/// handed to the provider calls [`ChatGptTokenManager::get`].
pub struct ChatGptTokenManager {
    flow: Arc<OAuthFlow>,
    single_flight: Arc<SingleFlightRefresh>,
    client: wcore_egress::EgressClient,
    storage: OAuthStorage,
    cached: Mutex<Option<OAuthTokens>>,
}

impl ChatGptTokenManager {
    pub fn new(storage: OAuthStorage) -> Self {
        Self {
            flow: Arc::new(build_chatgpt_flow()),
            single_flight: Arc::new(SingleFlightRefresh::new()),
            client: wcore_egress::EgressClient::tool(),
            storage,
            cached: Mutex::new(None),
        }
    }

    /// Whether the token is valid for at least `REFRESH_LEAD_SECS` more
    /// seconds. ChatGPT always sets `expires_in`, so a MISSING expiry is
    /// treated as stale (forces a refresh) rather than fresh.
    fn token_is_fresh(t: &OAuthTokens) -> bool {
        let Some(exp) = t.expires_at_unix_secs else {
            return false;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        exp.saturating_sub(REFRESH_LEAD_SECS) > now
    }

    /// Whether the token is past its actual expiry (no lead window). Used by
    /// the 429 path: if a rate-limited refresh returns and the current token
    /// is NOT hard-expired, we can keep using it.
    fn token_is_hard_expired(t: &OAuthTokens) -> bool {
        let Some(exp) = t.expires_at_unix_secs else {
            // Unknown expiry → cannot prove still-valid; treat as expired so
            // a 429 doesn't hand back a possibly-dead token.
            return true;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        exp <= now
    }

    /// Load the cached token from disk on first call, then keep it in memory.
    /// A cache miss returns `Ok(None)` so the caller can surface login
    /// guidance rather than an opaque error.
    async fn load_cached(&self) -> Result<Option<OAuthTokens>, String> {
        let mut guard = self.cached.lock().await;
        if guard.is_some() {
            return Ok(guard.clone());
        }
        let from_disk = self
            .storage
            .load(PROVIDER)
            .map_err(|e| format!("oauth storage load failed: {e}"))?;
        *guard = from_disk.clone();
        Ok(from_disk)
    }

    /// Clear the in-memory token cache. Logout calls this so a live manager
    /// can't re-persist a token after the on-disk file is removed (C5).
    pub async fn clear_cache(&self) {
        *self.cached.lock().await = None;
    }

    /// Return `(access_token, account_id)`, refreshing if near expiry.
    pub async fn get(&self) -> Result<(String, String), String> {
        let tokens = self.load_cached().await?.ok_or_else(|| {
            "not signed in to ChatGPT — run `wayland auth login chatgpt`".to_string()
        })?;
        let tokens = if Self::token_is_fresh(&tokens) {
            tokens
        } else {
            self.refresh(tokens).await?
        };
        let claims = decode_codex_claims(&tokens.access_token)?;
        Ok((tokens.access_token, claims.account_id))
    }

    /// Refresh `current` via the rotating-refresh-token grant.
    ///
    /// C3: on a `429` (rate limit), if `current` is not hard-expired we return
    /// it unchanged instead of failing the turn. C4: a successful refresh that
    /// ROTATED the refresh token but failed to persist is a HARD error.
    async fn refresh(&self, current: OAuthTokens) -> Result<OAuthTokens, String> {
        let refresh_token = current
            .refresh_token
            .clone()
            .ok_or("no refresh_token — run `wayland auth login chatgpt`")?;
        let client = self.client.clone();
        let token_url = self.flow.token_url.clone();
        let client_id = self.flow.client_id.clone();

        let refreshed = self
            .single_flight
            .refresh(move || async move {
                let form: Vec<(&str, String)> = vec![
                    ("grant_type", "refresh_token".into()),
                    ("refresh_token", refresh_token),
                    ("client_id", client_id),
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

                // A 429 is a rate limit, NOT an auth failure — surface it as a
                // recognizable sentinel so the caller can keep using the still
                // -valid current token (C3). Do NOT include the response body
                // (C7 — token-endpoint bodies are never logged).
                if status.as_u16() == 429 {
                    return Err(RefreshError::Transport(RATE_LIMIT_SENTINEL.into()));
                }
                if !status.is_success() {
                    // C7: cap + scrub — surface only the status, never the body.
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
                    // ROTATES — single-use. None here means the server omitted
                    // it (genuine non-rotation); merged forward below.
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
                    scope: raw.get("scope").and_then(|v| v.as_str()).map(str::to_string),
                    id_token: raw.get("id_token").and_then(|v| v.as_str()).map(str::to_string),
                })
            })
            .await;

        let refreshed = match refreshed {
            Ok(t) => t,
            Err(RefreshError::Transport(msg)) if msg == RATE_LIMIT_SENTINEL => {
                // C3: rate limited. Keep using the current token if it has not
                // actually expired; only error when it's truly dead.
                if Self::token_is_hard_expired(&current) {
                    return Err(
                        "ChatGPT refresh is rate limited (429) and the access token has \
                         expired — try again shortly."
                            .to_string(),
                    );
                }
                *self.cached.lock().await = Some(current.clone());
                return Ok(current);
            }
            Err(e) => return Err(format!("refresh failed: {e}")),
        };

        // C4: distinguish "server omitted refresh_token" (non-rotation, keep
        // old, safe) from "got a new one, persist failed" (hard error).
        let rotated = refreshed.refresh_token.is_some();
        let mut to_store = refreshed;
        if to_store.refresh_token.is_none() {
            to_store.refresh_token = current.refresh_token.clone();
        }
        if let Err(e) = self.storage.store(PROVIDER, &to_store) {
            if rotated {
                // The old refresh token is now burned server-side and the new
                // one was NOT saved — fail loudly so the user re-logs in now
                // rather than hitting a dead token next process start.
                return Err(format!(
                    "ChatGPT refresh rotated the refresh token but persisting it failed \
                     ({e}); run `wayland auth login chatgpt` to re-authenticate"
                ));
            }
            // Non-rotation: the on-disk token is unchanged and still valid;
            // a persist failure of identical data is not fatal.
            tracing::warn!(error = %e, "failed to persist refreshed ChatGPT access token");
        }
        *self.cached.lock().await = Some(to_store.clone());
        Ok(to_store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Task 2.2: JWT account-id decode ──────────────────────────────

    #[test]
    fn extracts_chatgpt_account_id_from_access_token() {
        // payload = {"https://api.openai.com/auth":{"chatgpt_account_id":"acct_123","chatgpt_plan_type":"pro"}}
        let payload = "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdF8xMjMiLCJjaGF0Z3B0X3BsYW5fdHlwZSI6InBybyJ9fQ";
        let jwt = format!("hdr.{payload}.sig");
        let claims = decode_codex_claims(&jwt).expect("decode");
        assert_eq!(claims.account_id, "acct_123");
        assert_eq!(claims.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn rejects_token_without_account_id() {
        let jwt = "hdr.eyJmb28iOiJiYXIifQ.sig"; // {"foo":"bar"}
        assert!(decode_codex_claims(jwt).is_err());
    }

    #[test]
    fn rejects_non_jwt_string() {
        assert!(decode_codex_claims("not-a-jwt").is_err());
    }

    // ── flow descriptor (Task 2.1) ───────────────────────────────────

    #[test]
    fn chatgpt_flow_uses_codex_redirect_and_extras() {
        let flow = build_chatgpt_flow();
        assert_eq!(flow.client_id, CLIENT_ID);
        assert_eq!(flow.redirect_host, "localhost");
        assert_eq!(flow.callback_path, "/auth/callback");
        assert!(matches!(
            flow.redirect_strategy,
            RedirectStrategy::FixedPort(1455)
        ));
        let (url, _state, _pkce) =
            flow.build_authorize_url("http://localhost:1455/auth/callback");
        assert!(url.contains("id_token_add_organizations=true"), "url={url}");
        assert!(url.contains("codex_cli_simplified_flow=true"), "url={url}");
        assert!(url.contains("originator=wayland"), "url={url}");
    }

    // ── Task 2.3: token manager — fresh / rotate / errors / 429 ──────

    /// A 3-segment JWT whose payload decodes to the given account id. Built
    /// from a JSON string base64url-encoded so the fixtures stay readable.
    fn jwt_with_account(account_id: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let payload = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
        });
        let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("hdr.{seg}.sig")
    }

    fn manager_at(root: std::path::PathBuf) -> ChatGptTokenManager {
        ChatGptTokenManager::new(OAuthStorage::at_root(root).expect("storage"))
    }

    fn token(access: &str, refresh: Option<&str>, expires_at: Option<u64>) -> OAuthTokens {
        OAuthTokens {
            access_token: access.to_string(),
            refresh_token: refresh.map(str::to_string),
            expires_at_unix_secs: expires_at,
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

    #[tokio::test]
    async fn returns_fresh_token_without_refreshing() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_at(tmp.path().join("oauth"));
        let at = jwt_with_account("acct_fresh");
        // Point token_url at an address that would fail if hit, proving no
        // refresh occurs for a fresh token.
        mgr.flow = Arc::new(build_chatgpt_flow_with_token_url("http://127.0.0.1:1/never"));
        mgr.storage
            .store(PROVIDER, &token(&at, Some("rt"), Some(far_future())))
            .unwrap();
        let (access, account) = mgr.get().await.expect("get");
        assert_eq!(access, at);
        assert_eq!(account, "acct_fresh");
    }

    #[tokio::test]
    async fn rotates_and_restores_refresh_token() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let new_at = jwt_with_account("acct_rotated");
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": new_at,
                "refresh_token": "rt-NEW",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_at(tmp.path().join("oauth"));
        mgr.flow = Arc::new(build_chatgpt_flow_with_token_url(&format!(
            "{}/oauth/token",
            server.uri()
        )));
        // Stored token already expired → forces refresh.
        mgr.storage
            .store(PROVIDER, &token(&jwt_with_account("acct_old"), Some("rt-OLD"), Some(0)))
            .unwrap();

        let (access, account) = mgr.get().await.expect("get");
        assert_eq!(access, new_at);
        assert_eq!(account, "acct_rotated");

        // The rotated refresh token must be persisted to disk.
        let on_disk = mgr.storage.load(PROVIDER).unwrap().expect("present");
        assert_eq!(on_disk.refresh_token.as_deref(), Some("rt-NEW"));
        assert_eq!(on_disk.access_token, new_at);
    }

    #[tokio::test]
    async fn keeps_old_refresh_token_when_server_omits_it() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let new_at = jwt_with_account("acct_norot");
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": new_at,
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_at(tmp.path().join("oauth"));
        mgr.flow = Arc::new(build_chatgpt_flow_with_token_url(&format!(
            "{}/oauth/token",
            server.uri()
        )));
        mgr.storage
            .store(PROVIDER, &token(&jwt_with_account("acct_old"), Some("rt-KEEP"), Some(0)))
            .unwrap();

        let (_access, _account) = mgr.get().await.expect("get");
        let on_disk = mgr.storage.load(PROVIDER).unwrap().expect("present");
        // Server omitted refresh_token → old one carried forward.
        assert_eq!(on_disk.refresh_token.as_deref(), Some("rt-KEEP"));
    }

    #[tokio::test]
    async fn rate_limit_returns_current_token_when_not_expired() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_at(tmp.path().join("oauth"));
        mgr.flow = Arc::new(build_chatgpt_flow_with_token_url(&format!(
            "{}/oauth/token",
            server.uri()
        )));
        let at = jwt_with_account("acct_429");
        // Inside the lead window (not fresh) but NOT hard-expired: 30s out,
        // lead is 120s → refresh attempted, 429 → keep current.
        let soon = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 30;
        mgr.storage
            .store(PROVIDER, &token(&at, Some("rt"), Some(soon)))
            .unwrap();

        let (access, account) = mgr.get().await.expect("get");
        assert_eq!(access, at, "429 must return the still-valid current token");
        assert_eq!(account, "acct_429");
    }

    #[tokio::test]
    async fn rate_limit_errors_when_token_already_expired() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let mut mgr = manager_at(tmp.path().join("oauth"));
        mgr.flow = Arc::new(build_chatgpt_flow_with_token_url(&format!(
            "{}/oauth/token",
            server.uri()
        )));
        // Hard-expired (exp = 0) + 429 → must error, never hand back a dead token.
        mgr.storage
            .store(PROVIDER, &token(&jwt_with_account("acct_dead"), Some("rt"), Some(0)))
            .unwrap();

        let err = mgr.get().await.unwrap_err();
        assert!(err.contains("rate limited"), "err={err}");
    }

    #[tokio::test]
    async fn errors_when_no_tokens_stored() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_at(tmp.path().join("oauth"));
        let err = mgr.get().await.unwrap_err();
        assert!(err.contains("not signed in"), "err={err}");
    }

    #[tokio::test]
    async fn clear_cache_drops_in_memory_tokens() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_at(tmp.path().join("oauth"));
        mgr.storage
            .store(PROVIDER, &token(&jwt_with_account("acct_c"), Some("rt"), Some(far_future())))
            .unwrap();
        // Prime the in-memory cache.
        let _ = mgr.get().await.expect("get");
        // Remove the backing file and clear the cache: a subsequent load must
        // miss, proving the cache was dropped.
        std::fs::remove_file(mgr.storage.path_for(PROVIDER)).unwrap();
        mgr.clear_cache().await;
        let err = mgr.get().await.unwrap_err();
        assert!(err.contains("not signed in"), "err={err}");
    }

    /// Test helper: a ChatGPT flow with the token URL overridden so the
    /// refresh round-trip can be pointed at a mock server. Mirrors
    /// [`build_chatgpt_flow`] otherwise.
    fn build_chatgpt_flow_with_token_url(token_url: &str) -> OAuthFlow {
        OAuthFlow::new(
            CLIENT_ID,
            None,
            AUTHORIZE_URL,
            token_url,
            SCOPES.iter().map(|s| s.to_string()).collect(),
        )
        .with_redirect_strategy(RedirectStrategy::FixedPort(CALLBACK_PORT))
        .with_redirect_uri_parts(CALLBACK_HOST, CALLBACK_PATH)
    }
}
